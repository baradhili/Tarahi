//! The connector-crossing math: the page's resolved jump radii, the
//! stored-jump classifier, the crossing set (`line_jumps`), and the
//! page walk that feeds it (`page_jumps`).

use super::connector::is_connector;
use super::hitframe::hit_frame;
use super::segments::geometry_hit_segments_straight;

#[derive(Clone)]
pub struct JumpLine {
    pub id: i64,
    pub z: usize,
    pub weight: f64,
    /// (r, g, b) — the stroke color both overlays repaint.
    pub color: (u8, u8, u8),
    /// The line's resolved hop style (`ConLineJumpStyle` over the
    /// page's `LineJumpStyle`, #324 phase 4).
    pub style: JumpStyle,
    pub pts: Vec<(f64, f64)>,
}

/// A computed hop (#108): where the jumping line crosses the under
/// line, both directions (unit), and both strokes. Visio's default
/// crossing treatment — the more-horizontal line hops (ties: the
/// later in z), arc style, never persisted.
#[derive(Debug, Clone, Copy)]
pub struct Jump {
    pub at: (f64, f64),
    /// Unit vector along the JUMPING line.
    pub dir: (f64, f64),
    /// Unit vector along the under line.
    pub under: (f64, f64),
    /// The JUMPING line's shape id (the stored-jump reconciliation,
    /// #303: hops are written into the jumper's own Geometry rows).
    pub jumper: i64,
    pub weight: f64,
    pub under_weight: f64,
    pub color: (u8, u8, u8),
    pub under_color: (u8, u8, u8),
    /// The JUMPER's resolved hop style (#324 phase 4 — the row writer
    /// and both painters read it).
    pub style: JumpStyle,
}

/// Visio's jump radius (#294): `LineToLineX × LineJumpFactorX / 2`,
/// both page-layout cells resolved through the page's style
/// inheritance ([`vsdx::Document::page_line_to_line_x`]/[
/// `vsdx::Document::page_line_jump_factor`]). The #294 repro pins the
/// identity: its stored jump `ArcTo` carries `A = −0.04166667` —
/// exactly 0.125 in × 0.66667 / 2, the "No Style" values Visio
/// resolves for that page. (The old constant 0.12 was ~3× too large
/// and ignored the page cells entirely.)
pub fn jump_radius_in(doc: &vsdx::Document, page: &vsdx::Page) -> f64 {
    doc.page_line_to_line_x(page) * doc.page_line_jump_factor(page) / 2.0
}

/// The Y-axis twin of [`jump_radius_in`]: `LineToLineY ×
/// LineJumpFactorY / 2` (PR #299 review — Visio authors a stored jump
/// on a VERTICAL run at the Y spacing, so the classifier must compare
/// against the arc's own axis).
pub fn jump_radius_y_in(doc: &vsdx::Document, page: &vsdx::Page) -> f64 {
    doc.page_line_to_line_y(page) * doc.page_line_jump_factor_y(page) / 2.0
}

/// Both resolved jump radii, X then Y.
pub fn jump_radii_in(doc: &vsdx::Document, page: &vsdx::Page) -> (f64, f64) {
    (jump_radius_in(doc, page), jump_radius_y_in(doc, page))
}

/// Visio's line-jump styles (page `LineJumpStyle` / connector
/// `ConLineJumpStyle`, #324 phase 4): the hop's shape inside its
/// 2r × r box over the slot. Value table and row encodings are
/// recorded in `Analysis/line-jump-styles.md` — [MS-Learn]'s
/// enumeration, MisterDiagram's Visio-2021 witnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpStyle {
    /// Values 0/1 — the semicircle (Default = Arc).
    Arc,
    /// Value 2 — the line breaks; nothing bridges the slot.
    Gap,
    /// Value 3 — a rectangular hump: risers at the slot ends, flat
    /// top at r (the "Square" name notwithstanding, the box is 2r
    /// wide — the witnessed form).
    Square,
    /// Values 4–9 — "2 sides".."7 sides": an N-segment hump
    /// inscribed in the semicircle (N = 2 is the witnessed triangle;
    /// N ≥ 3 is the natural reading of the display names,
    /// UNWITNESSED — see the Analysis record).
    Sides(u8),
}

impl JumpStyle {
    /// The cell value → style: 0 and 1 are both Arc, 2 Gap, 3 Square,
    /// 4–9 the 2–7-sided humps; anything else falls back to Arc (the
    /// safe default family).
    pub fn from_cell(v: f64) -> JumpStyle {
        match v.round() as i64 {
            2 => JumpStyle::Gap,
            3 => JumpStyle::Square,
            n @ 4..=9 => JumpStyle::Sides((n - 2) as u8),
            _ => JumpStyle::Arc,
        }
    }
}

/// A connector's resolved jump style: its own `ConLineJumpStyle`
/// (chain-resolved; 0 = the page default) over the page's
/// `LineJumpStyle`.
fn line_jump_style(
    doc: &vsdx::Document,
    chain: &[&vsdx::model::Shape],
    s: &vsdx::model::Shape,
    page_style: JumpStyle,
) -> JumpStyle {
    match doc
        .resolve_cell(s, chain, "ConLineJumpStyle")
        .and_then(|c| c.numeric())
    {
        Some(v) if v != 0.0 => JumpStyle::from_cell(v),
        _ => page_style,
    }
}

/// The page's own `LineJumpStyle`, style-resolved like the radius
/// cells; absent = Arc, Visio's default.
pub fn page_jump_style(doc: &vsdx::Document, page: &vsdx::Page) -> JumpStyle {
    doc.resolve_page_layout_cell(page, "LineJumpStyle")
        .and_then(|c| c.numeric())
        .map(JumpStyle::from_cell)
        .unwrap_or(JumpStyle::Arc)
}

/// The run continues along the chord: the neighboring segment exists,
/// is collinear with it, and points the same way (Visio writes jumps
/// MID-RUN — an isolated authored semicircle has no such context and
/// stays a curve, #294 review).
fn run_continues(dir: Option<(f64, f64)>, chord: (f64, f64)) -> bool {
    let Some(d) = dir else {
        return false;
    };
    let dl = d.0.hypot(d.1);
    let cl = chord.0.hypot(chord.1);
    dl > 1e-9
        && (d.0 * chord.1 - d.1 * chord.0).abs() <= 0.02 * cl * dl
        && d.0 * chord.0 + d.1 * chord.1 > 0.0
}

/// Parse a `POLYLINE(xType, yType, x1, y1, …)` control string into
/// its type flags and vertex pairs. Anything malformed (or not a
/// percentage-typed polyline — the stored-hop form is always
/// `POLYLINE(0, 0, …)`) is `None`.
pub type PolylineControls = ((f64, f64), Vec<(f64, f64)>);

pub fn parse_polyline_controls(a: &str) -> Option<PolylineControls> {
    let inner = a.trim().strip_prefix("POLYLINE(")?.strip_suffix(')')?;
    let nums: Vec<f64> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if nums.len() < 4 || !nums.len().is_multiple_of(2) {
        return None;
    }
    let types = (nums[0], nums[1]);
    if types != (0.0, 0.0) {
        return None; // local-coordinate controls are not the stored form
    }
    let pairs: Vec<(f64, f64)> = nums[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0], c[1]))
        .collect();
    Some((types, pairs))
}

/// A stored SQUARE / TRIANGLE / N-SIDES hop row (#324 phase 4): a
/// `PolylineTo` whose `A` controls are percentages of the shape's
/// `size` that all sit inside the 2r × r box over the row's chord,
/// with the route continuing collinear on both sides — the same
/// evidence family as [`is_stored_jump_arc`] (the chord is the slot:
/// ≈ 2·radius, the arc's exact twin). `Analysis/line-jump-styles.md`.
#[allow(clippy::too_many_arguments)] // the stored-hop evidence contract, one fact per argument
pub fn is_stored_jump_polyline(
    a_text: &str,
    start: (f64, f64),
    chord: (f64, f64),
    prev_dir: Option<(f64, f64)>,
    next_dir: Option<(f64, f64)>,
    size: (f64, f64),
    radius_x: f64,
    radius_y: f64,
) -> bool {
    let Some((_, controls)) = parse_polyline_controls(a_text) else {
        return false;
    };
    if controls.is_empty() || size.0.abs() <= 1e-9 || size.1.abs() <= 1e-9 {
        return false;
    }
    let len = chord.0.hypot(chord.1);
    if len <= 1e-9 || !run_continues(prev_dir, chord) || !run_continues(next_dir, chord) {
        return false;
    }
    // The chord IS the slot: ≈ 2· the radius, the axis picked by the
    // chord's class (the arc row's rule). A NEAR-DIAGONAL chord takes
    // whichever axis's radius matches — the >= tie flips across
    // six-decimal row rounding exactly as the arc classifier's does
    // (a 45° run saved at the X radius was once re-read against Y and
    // rejected, CodeRabbit round on #324 phase 5).
    let near_diagonal = (chord.0.abs() - chord.1.abs()).abs() <= len * 0.02;
    let band =
        |radius: f64| radius > 0.0 && (len - 2.0 * radius).abs() <= (radius * 0.05).max(1e-6);
    let radius = if near_diagonal {
        if band(radius_x) {
            radius_x
        } else {
            radius_y
        }
    } else if chord.0.abs() >= chord.1.abs() {
        radius_x
    } else {
        radius_y
    };
    if !band(radius) {
        return false;
    }
    // Every control's ALONG inside the slot (a hair of writer rounding
    // on each side). The fractions are of the WHOLE SHAPE, so each
    // denormalized control is read relative to the chord's own start.
    let (ux, uy) = (chord.0 / len, chord.1 / len);
    let tol = (radius * 0.02).max(1e-6);
    let rel = |fx: f64, fy: f64| {
        let (cx, cy) = (fx * size.0 - start.0, fy * size.1 - start.1);
        (cx * ux + cy * uy, cy * ux - cx * uy)
    };
    let in_box = controls
        .iter()
        .all(|&(fx, fy)| rel(fx, fy).0 >= -radius * 0.1 && rel(fx, fy).0 <= len + radius * 0.1);
    if !in_box {
        return false;
    }
    // Match the control SEQUENCE against a shape this engine
    // generates — the single apex at the midpoint (Sides ≥ 2), or the
    // flat top's two corners at the slot ends. Arbitrary in-box
    // polylines that double back match no generated hump and stay
    // authored art (#324 phase-5 review).
    let near = |v: f64, want: f64| (v - want).abs() <= (tol * 4.0).max(1e-6);
    // Square: both corners AT the slot ends, a full r off the run,
    // ordered along the run and on the SAME side (either bulge
    // direction). Two controls can ALSO be Sides(3)'s inscribed pair
    // — the square check failing falls THROUGH to the polygonal
    // check, not out (PR #339 review caught the early return
    // stranding 3-sided hops).
    if controls.len() == 2 {
        let (aa, ao) = rel(controls[0].0, controls[0].1);
        let (ba, bo) = rel(controls[1].0, controls[1].1);
        if near(ao.abs(), radius)
            && near(bo.abs(), radius)
            && aa <= tol
            && ba >= len - tol
            && ao.signum() == bo.signum()
        {
            return true;
        }
    }
    // Sides(n): n−1 = controls.len() interior inscribed vertices at
    // along = len/2 − r·cos(πk/n), off = ±r·sin(πk/n) — the sign
    // consistent across the sequence (the bump sits on ONE side).
    let n = controls.len() as i64 + 1;
    if !(2..=9).contains(&n) {
        return false; // more controls than any style emits
    }
    let mut side = 0.0f64;
    for (k, (fx, fy)) in controls.iter().enumerate() {
        let ang = std::f64::consts::PI * (k + 1) as f64 / n as f64;
        let (along, off) = rel(*fx, *fy);
        let want_off = radius * ang.sin();
        if !near(off.abs(), want_off) || !near(along, len / 2.0 - radius * ang.cos()) {
            return false;
        }
        let s = off.signum();
        if s != 0.0 {
            if side == 0.0 {
                side = s;
            } else if side != s {
                return false; // zig-zags between the sides
            }
        }
    }
    true
}

/// A stored GAP hop (#324 phase 4): a mid-route `MoveTo` whose skip
/// is collinear with the runs on both sides and ≈ 2·radius long —
/// `LineTo(at−r) · MoveTo(at+r) · LineTo`, the witnessed gap form.
pub fn is_stored_jump_gap(
    skip: (f64, f64),
    prev_dir: Option<(f64, f64)>,
    next_dir: Option<(f64, f64)>,
    radius_x: f64,
    radius_y: f64,
) -> bool {
    let len = skip.0.hypot(skip.1);
    if len <= 1e-9 || !run_continues(prev_dir, skip) || !run_continues(next_dir, skip) {
        return false;
    }
    let radius = if skip.0.abs() >= skip.1.abs() {
        radius_x
    } else {
        radius_y
    };
    radius > 0.0 && (len - 2.0 * radius).abs() <= (radius * 0.05).max(1e-6)
}

/// A stored line-jump arc (#294): Visio saves a connector's jump as
/// an `ArcTo` row ON a straight run — the chord lies ALONG the run
/// (orthogonal or diagonal: b01's 18° link carries its hop on a
/// diagonal run, live-measured Visio behavior per mk-git 2026-11-25),
/// the arc is a SEMICIRCLE over the gap (`|A| = radius = chord / 2`
/// exactly; the #294 repro's row: chord 0.0833, A −0.0416667 — the
/// band admits writer rounding), and the route continues collinear on
/// both sides (`prev_dir`/`next_dir`: the neighboring segments'
/// directions; `None` = no such run). Corner arcs cut ACROSS their
/// runs (the route turns there, so a side fails the collinear
/// context), authored shallow bumps fall far below the band, and
/// whole-route semicircles have no continuing runs — all stay
/// curves. The callers additionally gate on connector-ness so
/// artwork arcs never strip. Once stripped, the route is straight at
/// the old arc and the jump overlay draws the ONE arc at the current
/// crossing with the page's own radius — a stored arc stops lingering
/// after a move (#294).
///
/// `radius_x`/`radius_y` are the page's own jump radii
/// ([`jump_radii_in`]), passed UNCONVERTED: every geometry frame is
/// inches, and the stored arc's A was authored at the page's radius
/// (both callers — the geom walk and the svg ink path — pass the
/// resolved page radii as they are). The
/// shape checks alone cannot tell an AUTHORED semicircle between two
/// straight runs from a stored jump (a 1-inch arc with A=−0.5 over
/// collinear runs matches every geometric test — CodeRabbit rounds 1–2
/// on #121), so the radius is the deciding evidence: the arc strips
/// only when its sagitta matches the page's resolved radius within
/// writer rounding. An authored arc that happens to sit exactly at the
/// page radius is indistinguishable from a stored jump and is
/// deliberately preserved (the safer direction — never destroy
/// authored geometry on a guess).
pub fn is_stored_jump_arc(
    sagitta: f64,
    chord: (f64, f64),
    prev_dir: Option<(f64, f64)>,
    next_dir: Option<(f64, f64)>,
    radius_x: f64,
    radius_y: f64,
) -> bool {
    let len = chord.0.hypot(chord.1);
    let a = sagitta.abs();
    // No absolute size cap: the radius is LineToLineX ×
    // LineJumpFactorX / 2 and LineJumpFactorX=10 is documented-valid
    // (a 0.625 in semicircle at the default spacing) — the semicircle
    // band, the straight-run context, the connector gate and the
    // page-radius evidence already separate jumps from artwork (#294
    // review). No axis-aligned-chord requirement either: Visio stores
    // jumps on DIAGONAL straight connectors too — `LineTo · ArcTo(A =
    // radius) · LineTo` along the run (b01's 18° link); the
    // collinear runs on BOTH sides are the evidence, and a rounded
    // corner's chord cuts across its runs and fails it (#324).
    len > 1e-9
        && a >= 0.4 * len
        && a <= 0.6 * len
        && run_continues(prev_dir, chord)
        && run_continues(next_dir, chord)
        // The arc's own axis picks the evidence radius: Visio authors
        // a jump on a horizontal run at the X cells, on a vertical run
        // at the Y cells (PR #299 review — one shared radius fails a
        // vertical stored jump whenever the page's Y spacing differs).
        // Only the CHOSEN axis's radius must be positive: a FactorY=0
        // page still recognizes horizontal stored jumps (PR #299
        // round-3 review); a zero on the chosen axis never strips.
        // A NEAR-DIAGONAL chord tests BOTH radii: the writer and this
        // classifier pick the axis on an >= tie that six-decimal row
        // rounding can flip across (the dead branch's #324 phase-5
        // review — unwitnessed robustness, not invented behavior).
        && {
            let near_diagonal = (chord.0.abs() - chord.1.abs()).abs() <= len * 0.02;
            let pick = |radius: f64| {
                radius > 0.0 && (a - radius).abs() <= (radius * 0.05).max(1e-6)
            };
            if near_diagonal {
                pick(radius_x) || pick(radius_y)
            } else if chord.0.abs() >= chord.1.abs() {
                pick(radius_x)
            } else {
                pick(radius_y)
            }
        }
}

/// Proper segment intersection (interior of both).
pub fn seg_cross(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> Option<(f64, f64)> {
    seg_cross_open(a0, a1, b0, b1)
        .filter(|(_, t, u)| (1e-9..=1.0 - 1e-9).contains(t) && (1e-9..=1.0 - 1e-9).contains(u))
        .map(|(p, _, _)| p)
}

/// Open segment intersection: the point plus both window parameters —
/// the caller decides which endpoint coincidences count.
fn seg_cross_open(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> Option<((f64, f64), f64, f64)> {
    let (r, s) = ((a1.0 - a0.0, a1.1 - a0.1), (b1.0 - b0.0, b1.1 - b0.1));
    let denom = r.0 * s.1 - r.1 * s.0;
    if denom.abs() < 1e-12 {
        return None; // parallel
    }
    let qp = (b0.0 - a0.0, b0.1 - a0.1);
    let t = (qp.0 * s.1 - qp.1 * s.0) / denom;
    let u = (qp.0 * r.1 - qp.1 * r.0) / denom;
    Some(((a0.0 + t * r.0, a0.1 + t * r.1), t, u))
}

/// The two routes' runs at a SHARED contact vertex alternate around
/// it (r1, r2, r1, r2 in angular order) — they cross; grouped-by-
/// route rays mean the routes merely touch (#294 review). A run one
/// route SHARES with the other's is a merge onto a shared run —
/// reject it before the angle sort, where the coincident pair's tie
/// reads as alternation by sort luck alone (#334: the join heading
/// east hopped, its mirror heading west did not). Each of a's runs
/// is tested against b's ACTUAL runs — not a ray fabricated from a's
/// vertex: the two contact vertices need not coincide inside
/// `hugged_vertex`'s window tolerances (CodeRabbit, on the re-land) —
/// and RECIPROCALLY: both runs must agree they lie on one line, or a
/// short run hides a long one's divergence (CodeRabbit round 2: a
/// 0.001-in run sits within 5e-13 of a piercing line whose own
/// endpoint is 5e-10 out — still a crossing).
fn alternating(vi: usize, a: &[(f64, f64)], vj: usize, b: &[(f64, f64)]) -> bool {
    for pa in [a[vi - 1], a[vi + 1]] {
        for qb in [b[vj - 1], b[vj + 1]] {
            if run_lies_along(a[vi], pa, b[vj], qb, true)
                && run_lies_along(b[vj], qb, a[vi], pa, true)
            {
                return false;
            }
        }
    }
    let (vx, vy) = a[vi];
    let ray = |p: (f64, f64)| (p.0 - vx).atan2(p.1 - vy);
    let mut angs = [
        (ray(a[vi - 1]), 0u8),
        (ray(a[vi + 1]), 0u8),
        (ray(b[vj - 1]), 1u8),
        (ray(b[vj + 1]), 1u8),
    ];
    angs.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let seq: Vec<u8> = angs.iter().map(|(_, r)| *r).collect();
    seq.windows(2).all(|w| w[0] != w[1]) && seq[0] != seq[3]
}

/// The turning route's two runs at the INTERIOR vertex `vx` merely
/// touch the other segment — both sit on the same side of its line
/// (a tangency that bounces back, #294 review), or one lies ALONG it
/// (a merge onto the other's run, #334). Neither is a crossing.
fn merely_touches(vx: usize, pts: &[(f64, f64)], other: &[(f64, f64)]) -> bool {
    let (a, b) = (other[0], other[1]);
    let dir = (b.0 - a.0, b.1 - a.1);
    let side = |p: (f64, f64)| dir.0 * (p.1 - a.1) - dir.1 * (p.0 - a.0);
    let (s1, s2) = (side(pts[vx - 1]), side(pts[vx + 1]));
    // Pure sign comparison — a magnitude cutoff would wave through
    // shallow same-side contacts on short segments (#294 review).
    if s1 * s2 > 0.0 {
        return true;
    }
    // A run lying along the OTHER SEGMENT's line — the one-route-turn
    // half of the shared-run merge, measured against the segment's
    // own line, either heading (the segment is crossed at its
    // interior, so it extends both ways from the contact).
    let v = pts[vx];
    [pts[vx - 1], pts[vx + 1]]
        .iter()
        .any(|&p| run_lies_along(v, p, other[0], other[1], false))
}

/// The polyline index a crossing hugs, when it hugs a segment
/// endpoint at all (`t` ≈ 0 or 1).
fn hugged_vertex(t: f64, window_index: usize) -> Option<usize> {
    if t <= 1e-9 {
        Some(window_index)
    } else if t >= 1.0 - 1e-9 {
        Some(window_index + 1)
    } else {
        None
    }
}

/// Which line hops at a crossing — Visio's page `LineJumpCode` and
/// the connectors' `ConLineJumpCode` (live-measured in Visio 2021,
/// mk-git 2026-11-25, #324; the Visio-authored b01/c02 fixtures
/// agree):
/// - page 1 "horizontal lines": the FLATTER crossing segment hops —
///   steepness, not class: between two steep lines the more
///   horizontal one still hops (29° over 61°, 75° over 85°, 5° over
///   15° — measured live), and two EQUALLY steep segments (an X of
///   mirrored diagonals, b01's untouched 4/5 pair) draw no hop;
/// - page 2 "vertical lines": the mirror image — the steeper hops;
/// - page 3/4 "last routed/displayed": the later line in z hops;
///   page 5 "first displayed": the earlier one;
/// - connector 1 "never" — it never hops; 2 "always" — it hops over
///   the other; 3 "other line" — the other one hops over it;
///   4 "neither" — no hop at its crossings. `ConFixedCode` is inert
///   (c02's whole top row: identical hops under every value).
#[derive(Debug, Clone)]
pub struct JumpRules {
    pub page_code: i64,
    /// `ConLineJumpCode` per connector id (absent = 0, the page rule).
    pub con_codes: std::collections::HashMap<i64, i64>,
}

impl Default for JumpRules {
    fn default() -> Self {
        Self {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        }
    }
}

/// Two crossing segments within this |Δ sin θ| count as EQUALLY
/// steep — the mirrored-X no-hop case. The band absorbs the writer
/// rounding of coordinates that reached the crossing through
/// different frames; genuinely different steepness (60° vs 84° is
/// |Δ sin| ≈ 0.13) stays far above it. Only EXACT ties are witnessed
/// (the mirrored pairs); the band width is a judgment call recorded
/// in `Analysis/line-jump-selection-witnesses.md`.
const STEEPNESS_TIE: f64 = 1e-3;

/// A run counts as lying along another when it runs parallel within
/// this |sin θ| (#334). The direction gate is deliberately loose —
/// it only nominates; the offset gate below decides.
const SHARED_RUN_SIN: f64 = 1e-9;
/// …and sits on ONE line: BOTH of the run's endpoints within this
/// many inches of the other line. The endpoint pair separates the
/// genuine merge from its two near-misses — the route that PIERCES
/// the line and runs on 5e-10 in behind it, and the vertex 5e-10
/// ABOVE the line whose departure merely aims along it (CodeRabbit,
/// on the re-land): each keeps an endpoint outside the offset. Only
/// genuinely shared runs are merges.
const SHARED_RUN_OFF_IN: f64 = 1e-11;

/// Does the run `v → p` lie ALONG the line of `q0 → q1` — a
/// shared-run merge contact (#334)? Parallel within
/// [`SHARED_RUN_SIN`] and both endpoints within
/// [`SHARED_RUN_OFF_IN`] of the other line. `same_heading` adds the
/// half-ray overlap the both-turn branch needs: opposite half-rays
/// meet only AT the vertex (the interleaved corner passes through),
/// while a segment crossed at its interior extends both ways, so
/// either heading overlaps it. The test is origin-independent — the
/// run is measured against the OTHER line itself, never against a
/// ray fabricated from a shared vertex the two routes may not
/// actually share.
fn run_lies_along(
    v: (f64, f64),
    p: (f64, f64),
    q0: (f64, f64),
    q1: (f64, f64),
    same_heading: bool,
) -> bool {
    let (d, r) = ((q1.0 - q0.0, q1.1 - q0.1), (p.0 - v.0, p.1 - v.1));
    let (ld, lr) = (d.0.hypot(d.1), r.0.hypot(r.1));
    if ld < 1e-12 || lr < 1e-12 {
        return false;
    }
    let cross = d.0 * r.1 - d.1 * r.0;
    if (cross / (ld * lr)).abs() >= SHARED_RUN_SIN {
        return false;
    }
    if same_heading && d.0 * r.0 + d.1 * r.1 <= 0.0 {
        return false;
    }
    let off = |x: (f64, f64)| (d.0 * (x.1 - q0.1) - d.1 * (x.0 - q0.0)).abs() / ld;
    off(v) < SHARED_RUN_OFF_IN && off(p) < SHARED_RUN_OFF_IN
}

impl JumpRules {
    /// `Some(true)` when line `a` (its crossing segment running `da`)
    /// hops over `b`, `Some(false)` when `b` hops, `None` for no hop.
    fn jumper(&self, a: &JumpLine, da: (f64, f64), b: &JumpLine, db: (f64, f64)) -> Option<bool> {
        let code = |l: &JumpLine| self.con_codes.get(&l.id).copied().unwrap_or(0);
        let (ca, cb) = (code(a), code(b));
        // 4 on either side: no hop at this crossing at all.
        if ca == 4 || cb == 4 {
            return None;
        }
        // The connector codes that FORCE the answer regardless of the
        // page rule: 2 "always" hops, 3 demands the OTHER line hop.
        let forced = match (ca, cb) {
            (2, c) if c != 2 => Some(true),
            (c, 2) if c != 2 => Some(false),
            (3, c) if c != 3 => Some(false),
            (c, 3) if c != 3 => Some(true),
            _ => None,
        };
        let a_jumps = forced.or_else(|| match self.page_code {
            // The steepness rule: |sin θ| is the unit direction's
            // |dy|. Code 1 hops the flatter segment, code 2 the
            // steeper; equally steep segments never hop.
            1 | 2 => {
                let (sa, sb) = (da.1.abs(), db.1.abs());
                if (sa - sb).abs() < STEEPNESS_TIE {
                    None
                } else if self.page_code == 1 {
                    Some(sa < sb)
                } else {
                    Some(sa > sb)
                }
            }
            3 | 4 => Some(a.z > b.z),
            5 => Some(a.z < b.z),
            // 0 is the off switch and 6 the legacy off (the read side
            // of #251); anything else unknown — never hop.
            _ => None,
        })?;
        // 1 "never": the chosen jumper's own veto (the under line's
        // code 1 is silent — the other line may still hop over it).
        let jumper_code = if a_jumps { ca } else { cb };
        (jumper_code != 1).then_some(a_jumps)
    }
}

/// Every crossing hop among the lines (#108) under the page-1
/// defaults; pairwise proper segment intersections, the flatter
/// segment jumping. `radius` is the page's resolved jump radius
/// (inches — [`jump_radius_in`]): it sizes the hop arc and bounds
/// what counts as one crossing REGION (#251) when converging
/// crossings cluster.
pub fn line_jumps(lines: &[JumpLine], (radius, radius_y): (f64, f64)) -> Vec<Jump> {
    line_jumps_with(lines, (radius, radius_y), &JumpRules::default())
}

/// [`line_jumps`] under explicit page/connector rules ([`JumpRules`])
/// — what [`page_jumps_stripping`] crosses with once it has read the
/// page's own cells through [`page_jump_rules`].
pub fn line_jumps_with(
    lines: &[JumpLine],
    (radius, radius_y): (f64, f64),
    rules: &JumpRules,
) -> Vec<Jump> {
    // Degenerate rows (a straight route's elbow LineTos collapse onto
    // the end) repeat points; the endpoint classification below reads
    // indices, so normalize first or the connector's geometric END
    // looks like an interior vertex (#294 review).
    let pts: Vec<Vec<(f64, f64)>> = lines
        .iter()
        .map(|l| {
            let mut v: Vec<(f64, f64)> = Vec::with_capacity(l.pts.len());
            for p in &l.pts {
                if v.last().is_none_or(|q| (q.0 - p.0).hypot(q.1 - p.1) > 1e-9) {
                    v.push(*p);
                }
            }
            v
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..lines.len() {
        for j in (i + 1)..lines.len() {
            // Per-PAIR crossings (#251): one crossing REGION hops once.
            // Flattened curves produce entry+exit sample windows whose
            // crossings converge onto the same pixels — hopping each
            // drew overlapping arcs on one intersection (the #238
            // screenshot). Clusters within the hop radius merge.
            let mut pair: Vec<Jump> = Vec::new();
            for (k1, w1) in pts[i].windows(2).enumerate() {
                for (k2, w2) in pts[j].windows(2).enumerate() {
                    // #294 review: a crossing ON an interior vertex of
                    // either polyline (an elbow corner exactly on the
                    // other line) is a real crossing — the strict-
                    // interior test used to drop it and, with stored
                    // jump arcs straightened, the replacement hop
                    // vanished with it. A crossing at a polyline's
                    // first/last point stays excluded: that is the
                    // connector's glue end, not a crossing.
                    let Some((at, t, u)) = seg_cross_open(w1[0], w1[1], w2[0], w2[1]) else {
                        continue;
                    };
                    // Off-segment hits (the crossing sits on the
                    // segment's EXTENSION) are not crossings.
                    if !(-1e-9..=1.0 + 1e-9).contains(&t) || !(-1e-9..=1.0 + 1e-9).contains(&u) {
                        continue;
                    }
                    let at_end_i =
                        hugged_vertex(t, k1).is_some_and(|ix| ix == 0 || ix == pts[i].len() - 1);
                    let at_end_j =
                        hugged_vertex(u, k2).is_some_and(|ix| ix == 0 || ix == pts[j].len() - 1);
                    if at_end_i || at_end_j {
                        continue;
                    }
                    // An interior-vertex contact is a crossing only
                    // when the vertex's two runs straddle the other
                    // segment's line — same-side runs TOUCH the line
                    // and bounce back (a tangency), and a run ALONG
                    // the other segment MERGES onto it; neither is a
                    // hop (#294 review, #334). When BOTH routes turn
                    // at the contact, the four incident runs must
                    // alternate by route around the vertex; grouped-
                    // by-route rays — or any ray the routes SHARE —
                    // mean the routes merely touch.
                    let vi = hugged_vertex(t, k1).filter(|&ix| ix > 0 && ix + 1 < pts[i].len());
                    let vj = hugged_vertex(u, k2).filter(|&ix| ix > 0 && ix + 1 < pts[j].len());
                    match (vi, vj) {
                        (Some(vi), Some(vj)) => {
                            if !alternating(vi, &pts[i], vj, &pts[j]) {
                                continue;
                            }
                        }
                        (Some(vi), None) => {
                            if merely_touches(vi, &pts[i], w2) {
                                continue;
                            }
                        }
                        (None, Some(vj)) => {
                            if merely_touches(vj, &pts[j], w1) {
                                continue;
                            }
                        }
                        (None, None) => {}
                    }
                    let unit = |a: (f64, f64), b: (f64, f64)| {
                        let d = (b.0 - a.0, b.1 - a.1);
                        let l = d.0.hypot(d.1).max(1e-9);
                        (d.0 / l, d.1 / l)
                    };
                    let (d1, d2) = (unit(w1[0], w1[1]), unit(w2[0], w2[1]));
                    // Visio's selection rule ([`JumpRules`]): which
                    // line hops — or neither (equally steep segments,
                    // a connector's veto).
                    let Some(i_jumps) = rules.jumper(&lines[i], d1, &lines[j], d2) else {
                        continue;
                    };
                    let (jmp, und) = if i_jumps {
                        (&lines[i], &lines[j])
                    } else {
                        (&lines[j], &lines[i])
                    };
                    let (d, du) = if i_jumps { (d1, d2) } else { (d2, d1) };
                    pair.push(Jump {
                        at,
                        dir: d,
                        under: du,
                        jumper: jmp.id,
                        weight: jmp.weight,
                        under_weight: und.weight,
                        color: jmp.color,
                        under_color: und.color,
                        style: jmp.style,
                    });
                }
            }
            // Merge the pair's jumps inside a hop radius: the arc's
            // own reach (2× the page's jump radius) bounds what "one
            // region" means visually. Distinct crossings beyond it
            // stay.
            let mut clustered: Vec<Jump> = Vec::new();
            for jmp in pair {
                // The reach is the hop's OWN axis radius — the unrelated
                // axis's would merge crossings the drawn arcs never
                // overlap (PR #299 round-3 review).
                let jr = if jmp.dir.0.abs() >= jmp.dir.1.abs() {
                    radius
                } else {
                    radius_y
                };
                // A hop on a zero-radius axis cannot draw — and must
                // not ENTER a cluster either: its stroke-width reach
                // would absorb a valid hop on the other axis and the
                // merged direction would draw nothing (CodeRabbit
                // round on #324 phase 2). The strip side already holds
                // the same rule — a zero on the chosen axis never
                // strips (`is_stored_jump_arc`, PR #299 round-3).
                if jr <= 0.0 {
                    continue;
                }
                let reach = (jr * 2.0).max((jmp.weight + jmp.under_weight) * 2.0);
                // Compare EVERY existing cluster (#218 review): the
                // last-only check (under the old x-sort) let a jump
                // near an earlier cluster open a duplicate instead of
                // merging. First hit wins; the centroid keeps the arc
                // on the intersection instead of stacking arcs.
                let mut hit = false;
                for c in clustered.iter_mut() {
                    let dd = (c.at.0 - jmp.at.0).hypot(c.at.1 - jmp.at.1);
                    if dd < reach {
                        c.at = ((c.at.0 + jmp.at.0) / 2.0, (c.at.1 + jmp.at.1) / 2.0);
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    clustered.push(jmp);
                }
            }
            out.extend(clustered);
        }
    }
    out
}

/// Parse a `#rrggbb` line-color cell into the overlay's (r, g, b).
fn parse_hex_rgb(v: &str) -> Option<(u8, u8, u8)> {
    let hex = v.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

/// Jump candidacy (#324 phase 2, the #322 witness): the crossing set
/// hops only true connectors — `ObjType = 2`, local or
/// chain-resolved through the master. A plain glued 1D line —
/// `ObjType 0`, static `PAR(PNT` glue — never joins the crossing
/// set. The wider glue-ownership gate [`is_connector_in`] stays what
/// reroute and glue use; jump candidacy is deliberately narrower.
pub fn is_jump_candidate(
    doc: &vsdx::Document,
    chain: &[&vsdx::model::Shape],
    s: &vsdx::model::Shape,
) -> bool {
    is_connector(s)
        || doc
            .resolve_cell(s, chain, "ObjType")
            .and_then(|c| c.numeric())
            .is_some_and(|v| v == 2.0)
}

/// The page's jump rules (#324 phase 2): the PageSheet's
/// `LineJumpCode` — resolved through the page's style inheritance
/// like the radius cells, absent meaning Visio's "horizontal lines"
/// default 1 — plus every jump candidate's resolved
/// `ConLineJumpCode`. One resolver behind the crossing set: the
/// export, the canvas overlay and the stored-jump reconciliation all
/// cross through [`page_jumps_stripping`], which reads its rules
/// here.
pub fn page_jump_rules(doc: &vsdx::Document, page: &vsdx::Page) -> JumpRules {
    let page_code = doc
        .resolve_page_layout_cell(page, "LineJumpCode")
        .and_then(|c| c.numeric())
        .map_or(1, |v| v.round() as i64);
    let mut rules = JumpRules {
        page_code,
        ..JumpRules::default()
    };
    for s in &page.shapes {
        let mut chain = vec![s];
        if let Some(root) = s
            .master
            .and_then(|mid| doc.master(mid))
            .and_then(|m| m.shapes.first())
        {
            chain.push(root);
        }
        if !is_jump_candidate(doc, &chain, s) {
            continue;
        }
        if let Some(code) = doc
            .resolve_cell(s, &chain, "ConLineJumpCode")
            .and_then(|c| c.numeric())
        {
            rules.con_codes.insert(s.id, code.round() as i64);
        }
    }
    rules
}

/// The page's connector crossings (#187 export parity): every
/// top-level connector's polyline — the same `hit_frame` +
/// [`geometry_hit_segments_straight`] walk the canvas jump overlay
/// uses, formula-preferred, restricted to connectors whose geometry
/// is entirely straight runs (#324 step 1: Visio never hops curved
/// connectors and never hops over one) — crossed under the page's
/// own jump rules ([`page_jump_rules`]). Hidden-layer members and
/// guides skip like the render walk.
pub fn page_jumps(doc: &vsdx::Document, page: &vsdx::Page, hidden: &[i64]) -> Vec<Jump> {
    page_jumps_stripping(doc, page, hidden, None)
}

/// [`page_jumps`] with an explicit STRIP-radius override (the #303
/// reconciliation): the stored-jump classification runs at `strip` —
/// the PRE-edit radius when a jump-size edit just moved the page
/// radius (#301) — while the hops themselves still compute at the
/// page's CURRENT radius. `None` strips at the current radius, the
/// plain [`page_jumps`] behavior.
pub fn page_jumps_stripping(
    doc: &vsdx::Document,
    page: &vsdx::Page,
    hidden: &[i64],
    strip: Option<(f64, f64)>,
) -> Vec<Jump> {
    // #294: the hop radii are the page's own LineToLineX/Y ×
    // LineJumpFactorX/Y / 2, resolved through the style chain — the
    // X pair for a horizontal hop, the Y pair for a vertical one
    // (PR #299 review).
    let radius = jump_radius_in(doc, page);
    let radius_y = jump_radius_y_in(doc, page);
    let (strip_r, strip_ry) = strip.unwrap_or((radius, radius_y));
    let rules = page_jump_rules(doc, page);
    let page_style = page_jump_style(doc, page);
    let mut lines: Vec<JumpLine> = Vec::new();
    for (z, s) in page.shapes.iter().enumerate() {
        if s.sheet
            .cell("LayerMember")
            .and_then(|c| c.numeric())
            .is_some_and(|m| hidden.contains(&(m as i64)))
        {
            continue;
        }
        if s.type_.as_deref() == Some("Guide") {
            continue;
        }
        // Top-level chain: the shape plus its master root.
        let mut chain = vec![s];
        let msize = s
            .master
            .and_then(|mid| doc.master(mid))
            .and_then(|m| m.shapes.first())
            .map(|r| {
                chain.push(r);
                let g = |n: &str| {
                    doc.resolve_cell(r, &[r], n)
                        .and_then(|c| c.numeric())
                        .unwrap_or(1.0)
                };
                (g("Width"), g("Height"))
            })
            .unwrap_or((1.0, 1.0));
        // Jump candidacy (#324 phase 2), deliberately narrower than
        // the glue-ownership gate: only ObjType-2 connectors join the
        // crossing set — a plain glued 1D line never hops and is
        // never hopped over (#322's witness).
        if !is_jump_candidate(doc, &chain, s) {
            continue;
        }
        let frame = hit_frame(doc, &chain, s, msize);
        let master_root = chain.get(1).copied();
        // Everything reaching here is a connector (the gate above), so
        // stored jump arcs strip from the polyline (#294) — the hop
        // computes on the straight run, never on a stale arc. The
        // evidence is THIS page's resolved radius (the same value the
        // hop draws with) — or the caller's override, the #301
        // pre-edit radius when a jump-size edit just landed.
        let (segs, straight) =
            geometry_hit_segments_straight(s, master_root, &frame, Some((strip_r, strip_ry)));
        // Straight-run eligibility (#324 step 1): a connector joins
        // the crossing set only when its geometry is entirely straight
        // runs — Visio never hops a curved connector and never hops
        // over one (live-measured, mk-git 2026-11-25). Recognized
        // stored-hop arcs straightened above and keep a straight
        // connector eligible; every curve row — authored arcs,
        // splines, NURBS, polylines — drops the connector whole. The
        // ink and the hit-testing keep their curve-aware walks.
        if !straight {
            continue;
        }
        if segs.is_empty() {
            continue;
        }
        // The polyline the crossing math walks: the segments' endpoints.
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(segs.len() + 1);
        pts.push(segs[0].0);
        for (_, b) in &segs {
            pts.push(*b);
        }
        let weight = doc
            .resolve_cell(s, &chain, "LineWeight")
            .and_then(|c| c.numeric())
            .unwrap_or(0.01);
        // #235 corollary (CodeRabbit, #278): an explicit-zero
        // LineWeight paints nothing since the fallback pen went, so
        // an invisible shaft must not make visible connectors hop
        // over it (and it has no hop of its own to draw). Drop it
        // from the crossing set entirely.
        if weight == 0.0 {
            continue;
        }
        // Resolve like the renderer (#218 review): a legacy palette
        // index (`LineColor V='0'`) is common, and the hex-only read
        // drew every such crossing's under line and hop arc in gray.
        let color = parse_hex_rgb(&crate::svg::themed_color_string(
            doc,
            s,
            &chain,
            "LineColor",
        ))
        .unwrap_or((0x42, 0x42, 0x42));
        lines.push(JumpLine {
            id: s.id,
            z,
            weight,
            color,
            style: line_jump_style(doc, &chain, s, page_style),
            pts,
        });
    }
    line_jumps_with(&lines, (radius, radius_y), &rules)
}
