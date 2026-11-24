//! The line-jump engine (#108, #187, #251, #294, #301, #303): the hop
//! radius from the page's layout cells, the stored-jump classifier,
//! the crossing selection among connector polylines, and the page's
//! crossing set. The canvas overlay, the `dc::svg` export and the
//! stored-jump reconciliation all draw their hops from THIS code —
//! one crossing set, [`page_jumps_stripping`] — so none of them can
//! drift from the others (#336: the canvas used to build its own).
//!
//! Extracted from `geom.rs` as a pure move (#324 phase 1 — Visio's
//! selection codes and jump styles grow here in later phases, and the
//! complexity deserves its own file); `geom` re-exports every public
//! item, so existing `dc::geom::` paths keep resolving. The hit walk
//! stays in `geom`: `geometry_hit_segments` straightens a stored jump
//! arc through [`is_stored_jump_arc`] here, and [`page_jumps_stripping`]
//! walks the page's connectors through that same hit walk.

use crate::geom::{geometry_hit_segments_marked, hit_frame, is_connector};

#[derive(Debug, Clone)]
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
    /// UNBRIDGED stroke breaks: pts indices a pen move OPENED (an
    /// authored second stroke — the window pairing pts[k-1]→pts[k]
    /// with k in breaks draws no ink and never crosses). A
    /// RECOGNIZED stored gap slot bridges instead (#324 phase 5).
    pub breaks: Vec<usize>,
    /// The PAGE-AXIS radii (unswapped, X then Y) plus the frame
    /// rotation that maps the connector's LOCAL axes onto the page
    /// (CodeRabbit rounds 6–7 on PR #339): one radius policy — the
    /// stored rows and the row-local classifiers pick the axis in the
    /// connector's OWN frame, so each CROSSING's hop carries the page
    /// pair pre-swapped when the run's page-dominance disagrees with
    /// its local dominance (a diagonal run under a rotated frame; a
    /// binary per-connector swap once got those wrong — Greptile).
    /// Every page-space consumer (the cluster reach, the painters,
    /// the overlay's spacing rule) then selects by the page direction
    /// and lands on the LOCAL axis's radius exactly like the writer.
    pub radii: (f64, f64),
    /// The frame's rotation in radians (page image of the local +X
    /// axis); 0.0 for unrotated lines.
    pub local_rot: f64,
}

/// A computed hop (#108): where the jumping line crosses the under
/// line, both directions (unit), and both strokes. Visio's default
/// crossing treatment (page `LineJumpCode` 1, [`JumpRules`]): the
/// flatter segment hops, equally steep segments never do; arc style.
/// The overlay itself persists nothing; dc-ui's `ReconcileLineJumps`
/// is what writes these hops into the jumper's Geometry rows.
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
    /// The straight-run room AROUND the crossing on the jumper's own
    /// polyline, page inches — the distance from the crossing to the
    /// nearer end of its continuing run (a turn, an unbridged stroke
    /// break, or the route's end). The cut slot, the bridge and the
    /// canvas gap may not reach past it: a hop against a corner once
    /// clipped into the connector's NEXT leg and disconnected the
    /// route (PR #339 round 5). Painters shrink the hop to the room;
    /// a room smaller than a pen-width drops the hop to a plain
    /// crossing (the row writer's own room check, `reconciled_rows`).
    pub cap: f64,
    /// The jumper line's PAGE-AXIS radii (already local-axis-corrected
    /// — see [`JumpLine::radii`]); the painters size the hop with
    /// these, matching the row writer's own axis pick.
    pub radii: (f64, f64),
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
    if types != (0.0, 0.0) && types != (1.0, 1.0) {
        return None; // mixed/unknown type flags are not a stored form
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
    let Some((types, controls)) = parse_polyline_controls(a_text) else {
        return false;
    };
    // [MS-Learn] POLYLINE type flags: (0, 0) percentages of the shape's
    // Width/Height, (1, 1) local coordinates. The LOCAL form is the
    // degenerate-frame encoding — the Connector tool's own horizontal
    // and vertical shapes carry Height/Width = 0 and cannot fraction
    // (PR #339 round 5: such hops once drew on canvas but never
    // saved). Only the FRACTION form divides by the shape's dims, so
    // only it reads them (absolute — the fractions denormalize against
    // the SIGNED values the writer divided by; a reversed connector
    // carries Width = −5 and its saved square must re-classify, #324
    // phase-5 review).
    let local_coords = types == (1.0, 1.0);
    if controls.is_empty() {
        return false;
    }
    if !local_coords && (size.0.abs() <= 1e-9 || size.1.abs() <= 1e-9) {
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
    // (PR #339 round 5: a 45° run saved at the X radius was once
    // re-read against Y and rejected).
    let near_diagonal = (chord.0.abs() - chord.1.abs()).abs() <= len * 0.02;
    let band = |radius: f64| {
        radius > 0.0 && (len - 2.0 * radius).abs() <= (radius * 0.05).max(1e-6)
    };
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
    // Every control inside the 2r × r box over the chord AND the
    // sequence matching a shape this engine generates: the single
    // apex at the midpoint (Sides ≥ 2), or the flat top's two corners
    // at the slot ends (Square). Arbitrary in-box polylines that
    // double back match no generated hump and stay authored art
    // (#324 phase-5 review). The fractions are of the WHOLE SHAPE, so
    // each denormalized control is read relative to the chord's own
    // start.
    let (ux, uy) = (chord.0 / len, chord.1 / len);
    let tol = (radius * 0.02).max(1e-6);
    let rel = |fx: f64, fy: f64| {
        let (cx, cy) = if local_coords {
            (fx - start.0, fy - start.1)
        } else {
            (fx * size.0 - start.0, fy * size.1 - start.1)
        };
        (cx * ux + cy * uy, cy * ux - cx * uy)
    };
    let in_box = controls
        .iter()
        .all(|&(fx, fy)| rel(fx, fy).0 >= -radius * 0.1 && rel(fx, fy).0 <= len + radius * 0.1);
    if !in_box {
        return false;
    }
    // Match the control sequence against a shape this engine
    // generates. The writer emits, for one hop over a run traveling
    // +along with the bump on +off: Sides(n) → n−1 interior inscribed
    // vertices at angles πk/n (k = 1..n−1), the apex form n = 2 being
    // the single midpoint control; Square → the two top corners at
    // the slot ends. Anything else (a doubled-back in-box polyline)
    // is authored art.
    let near = |v: f64, want: f64| (v - want).abs() <= (tol * 4.0).max(1e-6);
    // Square: both corners AT the slot ends, a full r off the run.
    // (Two controls can ALSO be Sides(3)'s inscribed pair — the
    // square check failing falls THROUGH to the polygonal check, not
    // out; PR #339 review caught the early return stranding 3-sided
    // hops.)
    if controls.len() == 2 {
        // The corners FOLLOW the run (first near the start, second
        // near the end — the writer's emission order) and sit on the
        // SAME side (either bulge direction). A reversed pair or a
        // straddle is not a hop the writer emits (PR #339 round 3).
        // (Two controls can ALSO be Sides(3)'s inscribed pair — the
        // square check failing falls THROUGH to the polygonal check.)
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
        let off_mag = off.abs();
        if !near(off_mag, want_off) || !near(along, len / 2.0 - radius * ang.cos()) {
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
    // The skip's own axis picks the evidence radius — and a
    // NEAR-DIAGONAL skip tests BOTH, the arc classifier's rule: the
    // >= tie flips across six-decimal row rounding and a 45° run
    // saved at one axis's radius was once re-read against the other
    // (PR #339 round 5).
    let near_diagonal = (skip.0.abs() - skip.1.abs()).abs() <= len * 0.02;
    let pick = |radius: f64| {
        radius > 0.0 && (len - 2.0 * radius).abs() <= (radius * 0.05).max(1e-6)
    };
    if near_diagonal {
        pick(radius_x) || pick(radius_y)
    } else if skip.0.abs() >= skip.1.abs() {
        pick(radius_x)
    } else {
        pick(radius_y)
    }
}

/// A stored line-jump arc (#294): Visio saves a connector's jump as an
/// `ArcTo` row ON the run — the chord lies along the run, AT ANY
/// ANGLE (a `ShapeRouteStyle 16` network link stores its hop on a
/// 60° run: `LineTo · ArcTo(A = radius) · LineTo`, the d04 witness),
/// the arc is a SEMICIRCLE over the gap (`|A| = radius = chord / 2`
/// exactly; the #294 repro's row: chord 0.0833, A −0.0416667 — the
/// band admits writer rounding), and the route continues collinear on
/// both sides (`prev_dir`/`next_dir`: the neighboring segments'
/// directions; `None` = no such run). The collinear runs on BOTH
/// sides are the evidence against everything else: a corner arc's
/// chord cuts ACROSS its runs and fails, authored shallow bumps fall
/// far below the band, and whole-route semicircles have no continuing
/// runs — all stay curves. The callers additionally gate on
/// connector-ness so artwork arcs never strip. Once stripped, the
/// route is straight at the old arc and the jump overlay draws the
/// ONE arc at the current crossing with the page's own radius — a
/// stored arc stops lingering after a move (#294).
///
/// `radius_x`/`radius_y` are the page's own jump radii
/// ([`jump_radii_in`]), passed UNCONVERTED. Every geometry frame is
/// inches — master-local, instance-local, page; the instance/master
/// ratios are rescals applied after authoring — and the stored arc's
/// A was authored at the page's radius, so the sagitta is compared
/// with the page radius in whatever frame the rows are read. Both
/// callers hand over the resolved page radii as they are.
///
/// The shape checks alone cannot tell an AUTHORED semicircle between
/// two straight runs from a stored jump (a 1-inch arc with A=−0.5 over
/// collinear runs matches every geometric test — CodeRabbit rounds 1–2
/// on #121), so the radius is the deciding evidence: the arc strips
/// only when its sagitta matches the page's resolved radius within
/// writer rounding. Every authored arc OFF the page radius is
/// preserved (the safer direction — never destroy authored geometry on
/// a guess); one that sits exactly AT it is indistinguishable from a
/// stored jump and is straightened like one — the unit test pins that
/// case.
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
    // jumps on DIAGONAL straight connectors too (ShapeRouteStyle 16
    // network links — `LineTo · ArcTo(A = radius) · LineTo` along a
    // 60° run, the d04 witness); the collinear runs on BOTH sides are
    // the evidence, and a rounded corner's chord cuts across its runs
    // and fails it (#324 phase 3).
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
        // rounding can flip across (a 45° run with unequal X/Y —
        // #324 phase-5 review).
        && {
            let near_diagonal =
                (chord.0.abs() - chord.1.abs()).abs() <= len * 0.02;
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

/// A SHARED run — two routes running on along one line — is a merge,
/// never a crossing (#334). It counts as shared only when the runs
/// really are on ONE line, to within the floating-point noise of
/// coordinates that reached the contact through two different frames:
/// the sine of the angle between them is at most `SHARED_RUN_SIN`
/// (noise is about 1e-12 even on a 0.01-in run), and they lie at most
/// `SHARED_RUN_OFF_IN` apart across the line (about 1e-13 on a 100-in
/// page). Both sit far under the contact tolerance, 1e-9 of a segment
/// on each side of a vertex: a route that pierces the other line and
/// runs on 5e-10 in beside it is a crossing, as it always was. Only a
/// genuinely shared run is new.
const SHARED_RUN_SIN: f64 = 1e-9;
const SHARED_RUN_OFF_IN: f64 = 1e-11;

/// The sine of the angle between two directions — 1 when either is
/// degenerate, so a degenerate direction never matches.
fn sin_between(p: (f64, f64), q: (f64, f64)) -> f64 {
    let (lp, lq) = (p.0.hypot(p.1), q.0.hypot(q.1));
    if lp > 0.0 && lq > 0.0 {
        (p.0 * q.1 - p.1 * q.0) / (lp * lq)
    } else {
        1.0
    }
}

/// How far `p` lies from the line through `o` along `dir` (inches;
/// infinite for a degenerate direction).
fn off_line(dir: (f64, f64), o: (f64, f64), p: (f64, f64)) -> f64 {
    let l = dir.0.hypot(dir.1);
    if l > 0.0 {
        (dir.0 * (p.1 - o.1) - dir.1 * (p.0 - o.0)).abs() / l
    } else {
        f64::INFINITY
    }
}

/// The run from `p0` toward `p` and the run from `q0` toward `q` are
/// ONE shared run: the same direction, leaving points on one line.
fn same_run(p0: (f64, f64), p: (f64, f64), q0: (f64, f64), q: (f64, f64)) -> bool {
    sin_between(p, q).abs() <= SHARED_RUN_SIN
        && p.0 * q.0 + p.1 * q.1 > 0.0
        && off_line(p, p0, q0) <= SHARED_RUN_OFF_IN
}

/// The two routes' runs at a SHARED contact vertex alternate around
/// it (r1, r2, r1, r2 in angular order) — they cross; grouped-by-
/// route rays mean the routes merely touch (#294 review). A ray one
/// route SHARES with the other — it arrives or leaves along the
/// other's own run, on the same line — is a merge and never a crossing
/// (#334). That is decided FIRST: equal angles tie in the sort, and
/// whether the sequence then reads "alternating" depends only on where
/// the other two rays fall (a join heading east hopped, westward did
/// not).
fn alternating(vi: usize, a: &[(f64, f64)], vj: usize, b: &[(f64, f64)]) -> bool {
    let (vx, vy) = a[vi];
    // Each route's two runs leave ITS OWN vertex, so the directions stay
    // exact even when the two contact vertices differ a little.
    let runs = |pts: &[(f64, f64)], v: usize| {
        let d = |p: (f64, f64)| (p.0 - pts[v].0, p.1 - pts[v].1);
        [d(pts[v - 1]), d(pts[v + 1])]
    };
    if runs(a, vi)
        .iter()
        .any(|&p| runs(b, vj).iter().any(|&q| same_run(a[vi], p, b[vj], q)))
    {
        return false;
    }
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

/// The two runs at the INTERIOR vertex `vx` do not straddle the other
/// segment's line, so the polyline does not cross it there (#294
/// review): either both sit on the same side — it touches the line and
/// bounces back (a tangency) — or one lies ALONG the segment, a shared
/// run: the route merges onto it or off it (#334).
fn merely_touches(
    vx: usize,
    pts: &[(f64, f64)],
    other: &[(f64, f64)],
    breaks: &[usize],
) -> bool {
    // A BLANK incident window is not ink: a stroke that ENDS (or
    // starts) ON the other line touches it, never crosses — the
    // phantom post-break window once served as the outgoing ray and
    // a leg-end contact drew a hop (CodeRabbit round 7 on PR #339).
    if breaks.contains(&(vx + 1)) || breaks.contains(&vx) {
        return true;
    }
    let (a, b) = (other[0], other[1]);
    let dir = (b.0 - a.0, b.1 - a.1);
    let side = |p: (f64, f64)| dir.0 * (p.1 - a.1) - dir.1 * (p.0 - a.0);
    let (back, fwd) = (pts[vx - 1], pts[vx + 1]);
    // Pure sign comparison — a magnitude cutoff would wave through
    // shallow same-side contacts on short segments (#294 review). A
    // run ON the line reads as side 0, which the product took for a
    // crossing — every merge onto a through line hopped. A shared run
    // needs the VERTEX on the other line too — a merge arrives AT the
    // line, so the contact vertex sits on it within the same noise
    // scale (a near-miss inside the hug tolerance can have its far
    // end graze the line while the vertex, and the route, crossed:
    // CodeRabbit round on #324 phase 2 — [(0,−1),(0,5e−10),(0.01,
    // 5e−12)] over a horizontal used to merge away a real crossing).
    // The shared-run arm also needs ANGULAR agreement on the leg that
    // lies on the line: a leg that merely GRAZES it (a hair-short
    // outgoing stub at a shallow angle) passes the offset test but
    // the route crossed for real — only a leg actually running ALONG
    // the line is a merge (CodeRabbit round on #324 phase 4).
    let on_line = |p: (f64, f64)| {
        off_line(dir, a, p) <= SHARED_RUN_OFF_IN
            && sin_between((p.0 - pts[vx].0, p.1 - pts[vx].1), dir).abs() <= SHARED_RUN_SIN
    };
    side(back) * side(fwd) > 0.0
        || (off_line(dir, a, pts[vx]) <= SHARED_RUN_OFF_IN && (on_line(back) || on_line(fwd)))
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
/// the connectors' `ConLineJumpCode` (the Visio 2021 witnesses
/// b01/c02, #322):
/// - page 1 "horizontal lines": the FLATTER crossing segment hops —
///   steepness, not class: between two steep lines the more
///   horizontal one still hops (a 60° link hops where it crosses an
///   84° one, the `d04` witness), and two EQUALLY steep segments
///   (an X of mirrored diagonals) draw no hop;
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
/// |Δ sin| ≈ 0.13) stays far above it.
const STEEPNESS_TIE: f64 = 1e-3;

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
/// segment jumping. A contact where the two routes SHARE a run — one
/// arrives at the other's vertex and continues along it — is a merge,
/// not a crossing (#334).
/// `radius` is the page's resolved jump radius (inches —
/// [`jump_radius_in`]): it sizes the hop arc and bounds what counts
/// as one crossing REGION (#251) when converging crossings cluster.
pub fn line_jumps(lines: &[JumpLine], (radius, radius_y): (f64, f64)) -> Vec<Jump> {
    let filled: Vec<JumpLine> = lines
        .iter()
        .cloned()
        .map(|mut l| {
            l.radii = (radius, radius_y);
            l
        })
        .collect();
    line_jumps_with(&filled, &JumpRules::default())
}

/// [`line_jumps`] under explicit page/connector rules ([`JumpRules`])
/// — what [`page_jumps_stripping`] crosses with once it has read the
/// page's own cells through [`page_jump_rules`].
/// One line's normalized geometry: the deduped points and the
/// remapped stroke-break indices.
struct NormLine {
    pts: Vec<(f64, f64)>,
    breaks: Vec<usize>,
    /// Vertices where an authored STROKE boundary sits even though
    /// the pen move was zero-length and deduplicated away — the run
    /// room stops there without suppressing the ink window
    /// (CodeRabbit round 7 on PR #339).
    bounds: Vec<usize>,
}

pub fn line_jumps_with(lines: &[JumpLine], rules: &JumpRules) -> Vec<Jump> {
    // Degenerate rows (a straight route's elbow LineTos collapse onto
    // the end) repeat points; the endpoint classification below reads
    // indices, so normalize first or the connector's geometric END
    // looks like an interior vertex (#294 review). The stroke-BREAK
    // indices remap with the points — a stale position would skip a
    // REAL segment and leave the phantom bridge crossing (PR #339
    // review: hops appeared in blank space after a duplicate point).
    let norm: Vec<NormLine> = lines
        .iter()
        .map(|l| {
            let mut v: Vec<(f64, f64)> = Vec::with_capacity(l.pts.len());
            let mut kept: Vec<usize> = Vec::with_capacity(l.pts.len());
            let mut bounds: Vec<usize> = Vec::new();
            for (i, p) in l.pts.iter().enumerate() {
                if v.last().is_none_or(|q| (q.0 - p.0).hypot(q.1 - p.1) > 1e-9) {
                    v.push(*p);
                    kept.push(i);
                } else if l.breaks.contains(&i) {
                    // The pen moved ZERO distance — no phantom window
                    // exists, but the strokes still boundary here.
                    bounds.push(v.len().saturating_sub(1));
                }
            }
            // A break before original index i lands before the KEPT
            // position that index i maps to (kept's binary-search
            // slot); the phantom window pairing across it skips.
            // A break maps to its MoveTo point's KEPT position — and
            // VANISHES when the pen-up move itself was zero-length
            // (the MoveTo point deduplicated into its predecessor:
            // no bridge segment exists to suppress, and shifting the
            // break onto the next real segment suppressed INK instead;
            // PR #339 round 3).
            let breaks = l
                .breaks
                .iter()
                .filter_map(|b| kept.iter().position(|&k| k == *b))
                .collect();
            NormLine { pts: v, breaks, bounds }
        })
        .collect();
    let pts: Vec<&Vec<(f64, f64)>> = norm.iter().map(|n| &n.pts).collect();
    let mut out = Vec::new();
    // The jumper's LINE index and run span per emitted hop,
    // index-aligned with `out` — the spacing pass's grouping key.
    let mut out_meta: Vec<(usize, (usize, usize))> = Vec::new();
    for i in 0..lines.len() {
        for j in (i + 1)..lines.len() {
            // Per-PAIR crossings (#251): one crossing REGION hops once.
            // Flattened curves produce entry+exit sample windows whose
            // crossings converge onto the same pixels — hopping each
            // drew overlapping arcs on one intersection (the #238
            // screenshot). Clusters within the hop radius merge.
                let mut pair: Vec<Jump> = Vec::new();
                // The jumper's run span per candidate, index-aligned
                // with `pair` — the cluster's same-run identity.
                // (line, first seg, last seg) — the run identity
                // includes the LINE: two different connectors carry
                // equal-looking spans.
                let mut pair_runs: Vec<(usize, usize, usize)> = Vec::new();
                let mut pair_lines: Vec<usize> = Vec::new();
            for (k1, w1) in pts[i].windows(2).enumerate() {
                // A window that spans an unbridged stroke break draws
                // no ink — never a crossing (#324 phase 5; the indices
                // are the NORMALIZED ones).
                if norm[i].breaks.contains(&(k1 + 1)) {
                    continue;
                }
                for (k2, w2) in pts[j].windows(2).enumerate() {
                    if norm[j].breaks.contains(&(k2 + 1)) {
                        continue;
                    }
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
                    if !(-1e-9..=1.0 + 1e-9).contains(&t)
                        || !(-1e-9..=1.0 + 1e-9).contains(&u)
                    {
                        continue;
                    }
                    let at_end_i = hugged_vertex(t, k1)
                        .is_some_and(|ix| ix == 0 || ix == pts[i].len() - 1);
                    let at_end_j = hugged_vertex(u, k2)
                        .is_some_and(|ix| ix == 0 || ix == pts[j].len() - 1);
                    if at_end_i || at_end_j {
                        continue;
                    }
                    // An interior-vertex contact is a crossing only
                    // when the vertex's two runs straddle the other
                    // segment's line — same-side runs TOUCH the line
                    // and bounce back (a tangency), never a hop (#294
                    // review). When BOTH routes turn at the contact,
                    // the four incident runs must alternate by route
                    // around the vertex; grouped-by-route rays mean
                    // the routes merely touch. And a run the two
                    // routes SHARE — one arrives or leaves along the
                    // other's own run — is a merge, never a crossing
                    // (#334).
                    let vi = hugged_vertex(t, k1).filter(|&ix| ix > 0 && ix + 1 < pts[i].len());
                    let vj = hugged_vertex(u, k2).filter(|&ix| ix > 0 && ix + 1 < pts[j].len());
                    match (vi, vj) {
                        (Some(vi), Some(vj)) => {
                            if !alternating(vi, pts[i], vj, pts[j]) {
                                continue;
                            }
                        }
                        (Some(vi), None) => {
                            if merely_touches(vi, pts[i], w2, &norm[i].breaks) {
                                continue;
                            }
                        }
                        (None, Some(vj)) => {
                            if merely_touches(vj, pts[j], w1, &norm[j].breaks) {
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
                    let (cap, span) = if i_jumps {
                        run_extent(&norm[i].pts, &norm[i].breaks, &norm[i].bounds, k1, t)
                    } else {
                        run_extent(&norm[j].pts, &norm[j].breaks, &norm[j].bounds, k2, u)
                    };
                    let line = if i_jumps { i } else { j };
                    let run = (line, span.0, span.1);
                    // The PER-CROSSING swap: the run's direction
                    // rotated back into the connector's frame decides
                    // which local axis owns it, and the page pair is
                    // pre-swapped when that disagrees with the page
                    // dominance (CodeRabbit round 7 on PR #339).
                    let pair_radii = {
                        let (ca, sa) = (jmp.local_rot.cos(), jmp.local_rot.sin());
                        let lx = d.0 * ca + d.1 * sa;
                        let ly = -d.0 * sa + d.1 * ca;
                        let p = jmp.radii;
                        if (d.0.abs() >= d.1.abs()) == (lx.abs() >= ly.abs()) {
                            p
                        } else {
                            (p.1, p.0)
                        }
                    };
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
                        cap,
                        radii: pair_radii,
                    });
                    pair_runs.push(run);
                    pair_lines.push(line);
                }
            }
            // Merge the pair's jumps inside a hop radius: the arc's
            // own reach (2× the page's jump radius) bounds what "one
            // region" means visually. Distinct crossings beyond it
            // stay. A crossing that lands ON the jumper's own bend
            // contributes one candidate per leg; the merged hop keeps
            // the first leg's direction, so its far endpoint can sit
            // off the other leg — a known boundary (CodeRabbit round
            // on #324 phase 2), deliberately left: the corpus's hops
            // are all MID-RUN (#294's evidence — Visio writes jumps
            // on runs, never at a jumper's bend), so there is no
            // witness for what a bent hop's geometry or stored rows
            // should be; the Phase 3 hop-geometry rewrite owns it.
            let mut clustered: Vec<Jump> = Vec::new();
            let mut clustered_runs: Vec<(usize, usize, usize)> = Vec::new();
            let mut clustered_lines: Vec<usize> = Vec::new();
            for (jmp_i, jmp) in pair.into_iter().enumerate() {
                // The reach is the hop's OWN axis radius — the unrelated
                // axis's would merge crossings the drawn arcs never
                // overlap (PR #299 round-3 review).
                let jr = if jmp.dir.0.abs() >= jmp.dir.1.abs() {
                    jmp.radii.0
                } else {
                    jmp.radii.1
                };
                // A hop on a zero-radius axis cannot draw — and must
                // not ENTER a cluster either: its stroke-width reach
                // would absorb a valid hop on the other axis and the
                // merged direction would draw nothing (CodeRabbit
                // round on #324 phase 2). The strip side already
                // holds the same rule — a zero on the chosen axis
                // never strips (`is_stored_jump_arc`, PR #299
                // round-3).
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
                let run = pair_runs[jmp_i];
                for (ci, c) in clustered.iter_mut().enumerate() {
                    let dd = (c.at.0 - jmp.at.0).hypot(c.at.1 - jmp.at.1);
                    // Only hops of the SAME jumper merge — one pair
                    // can yield hops with different jumpers at
                    // DISTINCT crossings (the L's horizontal leg hops
                    // here, the other line hops over its vertical leg
                    // there); merging those stole the second
                    // connector's hop and moved the first's cut off
                    // its shaft (CodeRabbit round on #324 phase 3).
                    // Two candidates at the SAME point (a shared-
                    // vertex contact seen through different window
                    // pairs, each picking its own jumper) are ONE
                    // crossing though: they merge to the first
                    // candidate's hop — one crossing, one hop
                    // (CodeRabbit round on #324 phase 4; which jumper
                    // wins at a both-bend contact has no witness).
                    let same_point = dd < 1e-6;
                    // Same CONTINUOUS run (the line+span identity) or
                    // the same POINT — mere jumper equality once
                    // absorbed a distinct crossing on a parallel
                    // stroke without averaging it anywhere (the hop
                    // count silently dropped; CodeRabbit round 6 on
                    // PR #339).
                    let on_same_run = clustered_runs[ci] == run
                        && (c.dir.0 * jmp.dir.0 + c.dir.1 * jmp.dir.1) > 0.99;
                    if dd < reach && (on_same_run || same_point) {
                        // Candidates at the SAME contact (different
                        // window pairs, each picking its own leg) may
                        // disagree on ROOM: keep the candidate that
                        // FITS its run — the leg-end candidate's cap
                        // is zero and neither painter nor writer
                        // could place it (CodeRabbit round 7 on PR
                        // #339).
                        if same_point && jmp.cap > c.cap + 1e-9 {
                            *c = jmp;
                            clustered_runs[ci] = run;
                        }
                        // Average the positions only for crossings on
                        // the SAME run (near-parallel directions) —
                        // the centroid of crossings on two legs of a
                        // bend sits on neither shaft, and neither the
                        // painter nor the row writer can place a hop
                        // there (#324 phase-5 review: keep the
                        // cluster's own crossing instead).
                        if on_same_run || same_point {
                            c.at = ((c.at.0 + jmp.at.0) / 2.0, (c.at.1 + jmp.at.1) / 2.0);
                        }
                        // The merged hop must fit the room of BOTH
                        // candidates — the conservative min keeps a
                        // shrunken hop inside the run (PR #339 round
                        // 5). SAME-POINT-only merges (different legs)
                        // keep the RETAINED candidate's cap: the min
                        // once zeroed a valid hop when the later
                        // candidate was the leg-end twin (Greptile
                        // round 8 on PR #339).
                        if on_same_run {
                            c.cap = c.cap.min(jmp.cap);
                        }
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    clustered.push(jmp);
                    clustered_runs.push(run);
                    clustered_lines.push(pair_lines[jmp_i]);
                }
            }
            out.extend(clustered);
            for (run, line) in clustered_runs.drain(..).zip(clustered_lines.drain(..)) {
                out_meta.push((line, (run.1, run.2)));
            }
        }
    }
    // The writer's spacing rule (reconciled_rows' cursor), applied to
    // the drawn set: a hop that does not clear the previous one's end
    // on the SAME run is a plain crossing. Two crossings just beyond
    // the cluster's merge reach still overlap SLOT-wise — both cuts
    // removed the shaft between them and the two bridges crossed in
    // the gap, leaving dangling ends (CodeRabbit round 6 on PR #339).
    {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<(i64, usize, usize), Vec<usize>> = BTreeMap::new();
        for (idx, j) in out.iter().enumerate() {
            let (line, run) = out_meta[idx];
            groups
                .entry((j.jumper, line, run.0 * 1_000 + run.1))
                .or_default()
                .push(idx);
        }
        let mut keep = vec![true; out.len()];
        for (_, idxs) in groups {
            let d = out[idxs[0]].dir;
            let mut idxs = idxs;
            idxs.sort_by(|a, b| {
                let da = (out[*a].at.0 - out[*b].at.0) * d.0 + (out[*a].at.1 - out[*b].at.1) * d.1;
                da.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut cursor = f64::NEG_INFINITY;
            for ix in idxs {
                let j = &out[ix];
                // Only a hop that CAN draw reserves space — the
                // painter/writer room rule (the axis radius capped to
                // the run, past half a pen). A cap-starved candidate
                // once reserved the run's start and suppressed a
                // later VALID hop the writer would have kept: neither
                // crossing got a hop (CodeRabbit round 7 on PR #339,
                // Greptile's twin). The starved hop itself STAYS in
                // the set — the writers decide what draws.
                let drawable = {
                    let axis = if j.dir.0.abs() >= j.dir.1.abs() {
                        j.radii.0
                    } else {
                        j.radii.1
                    };
                    // The writer's rule: a positive radius that fits
                    // the run (the cap already carries the half-pen
                    // lead). A pen-WIDTH floor once erased every hop
                    // of a connector whose LineWeight exceeds the
                    // page radius while the writer kept storing rows
                    // (Greptile round 8 on PR #339).
                    axis.min((j.cap - j.weight * 0.5).max(0.0)) > 0.0
                };
                if !drawable {
                    continue;
                }
                // The slot INTENT (the uncapped axis radius) governs
                // spacing.
                let r = if j.dir.0.abs() >= j.dir.1.abs() {
                    j.radii.0
                } else {
                    j.radii.1
                };
                let p = j.at.0 * d.0 + j.at.1 * d.1;
                if r <= 0.0 || p - r <= cursor + 2e-6 {
                    keep[ix] = false;
                    continue;
                }
                cursor = p + r;
            }
        }
        let mut spaced = Vec::with_capacity(out.len());
        for (j, k) in out.into_iter().zip(keep) {
            if k {
                spaced.push(j);
            }
        }
        out = spaced;
    }
    out
}

/// The straight-run room around the crossing at parameter `t` of
/// polyline segment `k` (window `pts[k] → pts[k+1]`): the distance to
/// the nearer end of the run the crossing sits on. The run extends
/// through COLLINEAR same-direction neighbors — a stored hop itself
/// splits its run into rows, so a row boundary is NOT a turn — and
/// stops at a bend, an unbridged stroke break (`breaks` holds the
/// phantom window's opening index), or the route's end (PR #339
/// round 5: the hop slot once ran past a corner and cut the next
/// leg).
fn run_extent(
    pts: &[(f64, f64)],
    breaks: &[usize],
    bounds: &[usize],
    k: usize,
    t: f64,
) -> (f64, (usize, usize)) {
    let seg = (pts[k + 1].0 - pts[k].0, pts[k + 1].1 - pts[k].1);
    let len = seg.0.hypot(seg.1);
    let continues = |d: (f64, f64)| {
        let dl = d.0.hypot(d.1);
        dl > 1e-9
            && (d.0 * seg.1 - d.1 * seg.0).abs() <= 1e-6 * dl * len
            && d.0 * seg.0 + d.1 * seg.1 > 0.0
    };
    let mut back = t * len;
    let mut m = k;
    while m > 0 && !breaks.contains(&m) && !bounds.contains(&m) {
        let prev = (pts[m].0 - pts[m - 1].0, pts[m].1 - pts[m - 1].1);
        if prev.0.hypot(prev.1) <= 1e-9 {
            m -= 1; // a collapsed segment — skip it
            continue;
        }
        if !continues(prev) {
            break;
        }
        back += prev.0.hypot(prev.1);
        m -= 1;
    }
    // The forward scan starts at the segment AFTER k — `fwd` already
    // carries k's own post-crossing part; adding segment k again once
    // reported nearly the whole leg as room past its end (CodeRabbit
    // round 6 on PR #339).
    let mut fwd = (1.0 - t) * len;
    let mut n = k + 1;
    // The bounds check reads the vertex the read segment STARTS at
    // (`n`) — the boundary's own segment belongs to the next stroke,
    // but the segment ENDING at the boundary is this run's. Checking
    // n+1 stopped one segment early (Greptile round 8 on PR #339).
    while n + 1 < pts.len() && !breaks.contains(&(n + 1)) && !bounds.contains(&n) {
        let next = (pts[n + 1].0 - pts[n].0, pts[n + 1].1 - pts[n].1);
        if next.0.hypot(next.1) <= 1e-9 {
            n += 1;
            continue;
        }
        if !continues(next) {
            break;
        }
        fwd += next.0.hypot(next.1);
        n += 1;
    }
    // The run's segment span (m ..= the last segment kept): the
    // CLUSTER identity — two crossings merge only on ONE CONTINUOUS
    // run, never merely on parallel directions (an N-route's two
    // horizontal strokes point the same way but are not one run;
    // merging them once averaged a hop into the blank span between
    // the strokes, CodeRabbit round 6 on PR #339).
    (back.min(fwd), (m, n.saturating_sub(1).max(k)))
}

/// Parse a `#rrggbb` line-color cell into the overlay's (r, g, b).
/// `color_value` hands a `#…` cell through verbatim, so the body is
/// whatever the file carries — only six hex digits parse:
/// - six BYTES need not be six digits. A multi-byte character
///   straddling a slice offset used to panic here, taking the render
///   and the reroute tail down with it (#333).
/// - the integer parser accepts a sign: `#+0ff00` passed the byte
///   checks and came out green instead of falling back.
///
/// Anything else is `None`, and the caller's gray fallback answers.
fn parse_hex_rgb(v: &str) -> Option<(u8, u8, u8)> {
    let hex = v.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // Six ASCII digits: every offset is a character boundary.
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((pair(0)?, pair(2)?, pair(4)?))
}

/// A jump candidate is a REAL connector — `ObjType = 2`, locally or
/// chain-resolved through the master (the #322 witness: Visio hops
/// only true connectors; a plain glued 1D line — `ObjType 0`, static
/// `PAR(PNT` glue — never joins the crossing set). The wider
/// glue-ownership gate [`crate::geom::is_connector_in`] stays what
/// reroute and glue use; jump candidacy is deliberately narrower,
/// and EVERY site that strips or rewrites stored jump arcs gates on
/// it too (the ink path and the reconcile — an authored arc on a
/// non-candidate line is artwork, never a hop to straighten).
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

/// The page's connector crossings (#187): every top-level connector's
/// polyline — `hit_frame` + [`geometry_hit_segments`], formula-
/// preferred, curve-aware — crossed under the page's own jump rules
/// ([`page_jump_rules`]). Hidden-layer members and guides skip like
/// the render walk. This is THE crossing set: the canvas overlay, the
/// export and the stored-jump reconciliation all draw from it (#336).
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
    // The page's Show-Line-Jumps switch gates the crossing set AT THE
    // SOURCE (#324 phase-3 review): a forced connector code (2) must
    // not draw or save a hop on a page whose jumps are off — every
    // consumer (export, canvas, reconcile) inherits this gate instead
    // of each remembering its own. The read is the STYLE-RESOLVED
    // cell (a page can inherit `LineJumpCode 0` from its LineStyle
    // chain — the local-only `line_jumps_shown` would miss it;
    // CodeRabbit round on #324 phase 4); absent anywhere still shows
    // (#251's corpus rule).
    let switch_off = doc
        .resolve_page_layout_cell(page, "LineJumpCode")
        .and_then(|c| c.numeric())
        .is_some_and(|v| v == 0.0 || v == 6.0);
    if switch_off || !page.line_jumps_shown() {
        return Vec::new();
    }
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
        if !is_jump_candidate(doc, &chain, s) {
            continue;
        }
        // The renderer's section-visibility twin: a connector whose
        // EVERY Geometry section carries `NoShow = 1` paints no
        // stroke, so nothing hops over it and it hops over nothing
        // (whole-shape gate — a connector with any visible section
        // stays; CodeRabbit round on #324 phase 4). The flag reads
        // the RENDERER's way — formula first, cached V as fallback —
        // so a formula that now evaluates to 0 with a stale NoShow=1
        // cache keeps its visible crossings (PR #339 round 4); the
        // sections come from the EFFECTIVE sheet (master-inherited
        // NoShow included).
        let eff = doc.effective_sheet(s, &chain);
        let geos = eff.sections_named("Geometry");
        if !geos.is_empty()
            && geos
                .iter()
                .all(|g| crate::svg::geometry_flag_on(doc, s, &chain, g, "NoShow"))
        {
            continue;
        }
        let frame = hit_frame(doc, &chain, s, msize);
        let master_root = chain.get(1).copied();
        // The renderer's per-SECTION visibility twin (PR #339 round
        // 5): a connector may carry several Geometry sections, and
        // one with NoShow or NoLine paints no stroke — it contributes
        // no crossings and nothing hops over it, while its VISIBLE
        // siblings stay eligible. The flags read the RENDERER's way
        // and in the SAME frame the ink path evaluates them
        // (`geometry_flag_on`); the whole-shape all-NoShow gate above
        // stays as the fast path. The skip set names the sections the
        // walk itself will read — own merged sections when the shape
        // has them, else the master root's.
        let walk_secs: Vec<&vsdx::model::Section> = if s.sheet.sections_named("Geometry").is_empty()
        {
            master_root
                .map(|r| r.sheet.sections_named("Geometry"))
                .unwrap_or_default()
        } else {
            s.sheet.sections_named("Geometry")
        };
        // NoShow drops the section whole (nothing paints, nothing
        // feeds — the render's early return); NoLine KEEPS the walk
        // (the section fills and its coordinates feed later sections'
        // formulas) and only the stroke leaves (Greptile round 8 on
        // PR #339).
        let mut hidden_secs: Vec<&vsdx::model::Section> = Vec::new();
        let mut noline_secs: Vec<&vsdx::model::Section> = Vec::new();
        for g in walk_secs {
            if crate::svg::geometry_flag_on(doc, s, &chain, g, "NoShow") {
                hidden_secs.push(g);
            } else if crate::svg::geometry_flag_on(doc, s, &chain, g, "NoLine") {
                noline_secs.push(g);
            }
        }
        // Everything reaching here is a connector (the gate above), so
        // stored jump arcs strip from the polyline (#294) — the hop
        // computes on the straight run, never on a stale arc. The
        // evidence is THIS page's resolved radius (the same value the
        // hop draws with) — or the caller's override, the #301
        // pre-edit radius when a jump-size edit just landed.
        // The gap/polyline writers choose their evidence radius from
        // the ROW-LOCAL direction, but the marked walk's segments are
        // PAGE-space — a rotated connector with unequal X/Y sizes
        // would write at one radius and re-classify against the other
        // (its saved gap then read as an ordinary stroke break and the
        // crossing vanished). The bridge classification below reads
        // the skip back through the frame's to_local, matching the
        // writer's frame (PR #339 round 4).
        let segs = geometry_hit_segments_marked(
            s,
            master_root,
            &frame,
            Some((strip_r, strip_ry)),
            &hidden_secs,
            &noline_secs,
        );
        if segs.is_empty() {
            continue;
        }
        // The polyline the crossing math walks. A PEN-UP bridge
        // segment (an authored stroke break) stays OUT: bridging it
        // drew hops in blank space between unrelated strokes. The one
        // bridged exception is a RECOGNIZED stored gap slot — the
        // saved gap's own strokes must read as one run so the
        // crossing inside the slot recomputes (the reconcile rewrites
        // it; #324 phase 5).
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(segs.len() + 1);
        let mut breaks: Vec<usize> = Vec::new();
        let unit = |a: (f64, f64), b: (f64, f64)| {
            let d = (b.0 - a.0, b.1 - a.1);
            let l = d.0.hypot(d.1).max(1e-9);
            (d.0 / l, d.1 / l)
        };
        let mut prev_run: Option<(f64, f64)> = None;
        for (k, (a, b, mark)) in segs.iter().enumerate() {
            if k == 0 {
                pts.push(*a);
            }
            if *mark != crate::geom::SegMark::Ink {
                // A SECTION-BOUNDARY bridge never classifies as a
                // stored gap — two sections are two strokes by
                // authorship and the blank span between them is
                // authored space, however gap-shaped (CodeRabbit
                // round 6 on PR #339).
                if *mark == crate::geom::SegMark::SectionStart {
                    breaks.push(pts.len());
                    prev_run = None;
                    pts.push(*b);
                    continue;
                }
                // THIS segment is the bridge: a is the previous
                // stroke's end, b the pen move's target — the skip is
                // b − a. The run directions around it: the last real
                // run INTO a, the first real run OUT of b — stopping
                // at ANOTHER pen move (a second pen move once supplied
                // a fake outgoing run and bridged blank span; PR #339
                // round 4) and skipping zero-length drawn segments.
                // The skip measured in the ROW-LOCAL frame — the
                // frame the writer's radius choice and the gap
                // classifier's axis pick both live in.
                let skip_local = (
                    frame.to_local(*b).0 - frame.to_local(*a).0,
                    frame.to_local(*b).1 - frame.to_local(*a).1,
                );
                let mut next_dir = None;
                let mut n = k + 1;
                while n < segs.len() {
                    let (na, nb, nmark) = &segs[n];
                    if *nmark != crate::geom::SegMark::Ink {
                        break;
                    }
                    if (na.0 - nb.0).hypot(na.1 - nb.1) > 1e-9 {
                        next_dir = Some(unit(*b, *nb));
                        break;
                    }
                    n += 1; // a collapsed drawn segment — skip it
                }
                // The gap's three vectors share ONE frame: the skip is
                // measured row-locally, so the neighbor directions
                // rotate into the same local frame through the rigid
                // to_local difference — a rotated connector's
                // page-space runs once rejected its own saved gap
                // (PR #339 round 5).
                let dir_local = |d: (f64, f64)| {
                    let o = frame.to_local((0.0, 0.0));
                    let e = frame.to_local(d);
                    let v = (e.0 - o.0, e.1 - o.1);
                    let l = v.0.hypot(v.1);
                    if l > 1e-12 {
                        (v.0 / l, v.1 / l)
                    } else {
                        (0.0, 0.0)
                    }
                };
                if !is_stored_jump_gap(
                    skip_local,
                    prev_run.map(dir_local),
                    next_dir.map(dir_local),
                    strip_r,
                    strip_ry,
                ) {
                    breaks.push(pts.len());
                    // A NEW STROKE begins — the saved direction belongs
                    // to the old one. Keeping it let a later pen move
                    // borrow the first stroke's direction and bridge a
                    // blank span (PR #339 round 3).
                    prev_run = None;
                }
            } else {
                // EVERY real segment records its OWN direction — the
                // pen-move branch below must read the run immediately
                // BEFORE the break, and saving the previous segment's
                // direction at each step left it one behind after a
                // bend (a stored gap after a corner was rejected; PR
                // #339 review). A ZERO-LENGTH segment (a collapsed
                // elbow row) records nothing — a (0,0) direction
                // failed run_continues and killed the gap after it
                // (PR #339 round 2); the last real direction stands.
                if (a.0 - b.0).hypot(a.1 - b.1) > 1e-9 {
                    prev_run = Some(unit(*a, *b));
                }
            }
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
        // The renderer's stroke-visibility twin: `LinePattern = 0`
        // hides the line entirely, so an invisible shaft must not
        // make visible connectors hop over it either (#324 phase-3
        // review — same parity as the zero-weight corollary above).
        if doc
            .resolve_cell(s, &chain, "LinePattern")
            .and_then(|c| c.numeric())
            .is_some_and(|p| p == 0.0)
        {
            continue;
        }
        // Resolve like the renderer (#218 review): a legacy palette
        // index (`LineColor V='0'`) is common, and the hex-only read
        // drew every such crossing's under line and hop arc in gray.
        let color = parse_hex_rgb(&crate::svg::themed_color_string(doc, s, &chain, "LineColor"))
            .unwrap_or((0x42, 0x42, 0x42));
        // One radius policy (CodeRabbit rounds 6–7 on PR #339): the
        // stored rows and the row-local classifiers pick the axis in
        // the CONNECTOR'S OWN frame, so the line carries its frame's
        // rotation and the UNSWAPPED page pair — each crossing's Jump
        // pre-swaps per its own run direction (a diagonal run under a
        // rotated frame is not the frame's x-axis question). Unequal
        // per-axis cells with a rotated connector once drew a hop the
        // writer refused to save (a zero on the writer's axis).
        let x_img = (
            frame.to_page((1.0, 0.0)).0 - frame.to_page((0.0, 0.0)).0,
            frame.to_page((1.0, 0.0)).1 - frame.to_page((0.0, 0.0)).1,
        );
        lines.push(JumpLine {
            id: s.id,
            z,
            weight,
            color,
            style: line_jump_style(doc, &chain, s, page_style),
            pts,
            breaks,
            radii: (radius, radius_y),
            local_rot: x_img.1.atan2(x_img.0),
        });
    }
    line_jumps_with(&lines, &rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::{Cell, Row, Section, Shape, SheetChild};

    /// The tests' hop radius — Visio's default (0.125 in × 2/3 / 2,
    /// the #294 identity).
    const TEST_JUMP_RADIUS: f64 = 0.125 * (2.0 / 3.0) / 2.0;

    /// A plain black 0.01-in test line (an Arc-styled jumper).
    fn jline(id: i64, z: usize, pts: &[(f64, f64)]) -> JumpLine {
        JumpLine {
            id,
            z,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: pts.to_vec(),
            breaks: Vec::new(),
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        }
    }

    /// #251: one crossing REGION hops once — a flattened curve's
    /// entry/exit sample windows converge onto the same pixels and
    /// merge to a single arc; distinct crossings beyond the hop
    /// radius stay separate.
    #[test]
    fn line_jumps_cluster_converging_crossings() {
        let l = |id: i64, z: usize, pts: &[(f64, f64)]| JumpLine {
            id,
            z,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: pts.to_vec(),
            breaks: Vec::new(),
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        // A shallow arc's belly cut by a horizontal just under its
        // apex: the line enters and leaves 0.05 in apart — well inside
        // a hop's reach (2 × 0.0417 in).
        let arc = vec![
            (0.0, 1.0),
            (0.5, 1.04),
            (1.0, 1.06),
            (1.5, 1.04),
            (2.0, 1.0),
        ];
        let through = [(0.0, 1.059), (2.0, 1.059)];
        // Control: that really is a PAIR of crossings. With nothing to
        // merge in — zero weights and a ~0 radius leave no reach — both
        // hop, so the one-hop assertion cannot pass on a single
        // crossing by accident (CodeRabbit round on #324: the steep
        // line this test used meets the arc once).
        let pair = [
            JumpLine {
                weight: 0.0,
                ..l(1, 0, &arc)
            },
            JumpLine {
                weight: 0.0,
                ..l(2, 1, &through)
            },
        ];
        let unmerged = line_jumps(&pair, (1e-9, 1e-9));
        assert_eq!(unmerged.len(), 2, "the pair is two crossings: {unmerged:?}");
        let js = line_jumps(
            &[l(1, 0, &arc), l(2, 1, &through)],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "the converging pair merges: {js:?}");
        assert!(
            (js[0].at.0 - 1.0).abs() < 1e-9,
            "the merged hop sits at the pair's centroid: {js:?}"
        );
        // Two crossings far apart stay two hops.
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (1.0, 1.0), (1.0, 3.0), (4.0, 3.0)]),
                l(2, 1, &[(2.0, 0.0), (2.0, 4.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "one real crossing");
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (3.0, 1.0)]),
                l(2, 1, &[(1.0, 0.0), (1.0, 2.0), (2.5, 2.0), (2.5, 0.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 2, "distinct crossings stay separate: {js:?}");
    }

    /// #294: the stored-jump detector recognizes Visio's saved jump
    /// row (the repro's `X 1.4583→1.5417, A −0.0416667` — chord ON the
    /// run) and leaves corner arcs (diagonal chord), authored shallow
    /// bumps and artwork-sized arcs alone.
    #[test]
    fn stored_jump_arcs_match_the_repro_row() {
        // Mid-run with collinear continuation on both sides (the run).
        assert!(is_stored_jump_arc(

            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        // A corner arc cuts across the run — never a jump.
        assert!(!is_stored_jump_arc(

            0.09,
            (0.5, 0.5),
            Some((1.0, 0.0)),
            Some((0.0, 1.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        // A stored jump on a DIAGONAL run — the d04 witness (a 60°
        // network link's `ArcTo`, chord along the run, |A| = the
        // page radius): the collinear runs on both sides are the
        // evidence now that the axis-aligned chord is gone (#324
        // phase 3). cos 60° = 0.5, sin 60° = √3/2.
        let (c60, s60) = (0.5_f64, 0.866_025_4_f64);
        assert!(is_stored_jump_arc(
            -0.032_808_4,
            (2.0 * 0.032_808_4 * c60, 2.0 * 0.032_808_4 * s60),
            Some((c60, s60)),
            Some((c60, s60)),
            0.032_808_4,
            0.032_808_4,
        ));
        // The same diagonal arc OFF the page radius stays authored
        // ink, exactly like the axis-aligned twin.
        assert!(!is_stored_jump_arc(
            -0.05,
            (2.0 * 0.05 * c60, 2.0 * 0.05 * s60),
            Some((c60, s60)),
            Some((c60, s60)),
            0.032_808_4,
            0.032_808_4,
        ));
        // An authored shallow bump (the #294 review's control) is a
        // curve: |A| ≪ the semicircle band.
        assert!(!is_stored_jump_arc(

            0.25,
            (4.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.25,
            0.25,
        ));
        // A large SHALLOW arc stays a curve — size alone never makes
        // a jump (the semicircle band does the work; the old 0.5 in
        // cap wrongly rejected documented factor-10 jumps, #294
        // review).
        assert!(!is_stored_jump_arc(

            1.0,
            (0.0, 4.0),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            1.0,
            1.0,
        ));
        // A large SEMICIRCLE with full route context is a jump at any
        // size (LineJumpFactorX=10 → 0.625 in at the defaults).
        assert!(is_stored_jump_arc(

            2.0,
            (0.0, 4.0),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            2.0,
            2.0,
        ));
        // Degenerate: zero chord and a zero-sagitta straight row
        // (which the ink path already emits as a line).
        assert!(!is_stored_jump_arc(

            0.0,
            (0.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        assert!(!is_stored_jump_arc(

            0.0,
            (0.0, 1.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        // An authored whole-route semicircle (#294 review) has no
        // continuing runs — it stays a curve.
        assert!(!is_stored_jump_arc(0.25, (0.5, 0.0), None, None, 0.25, 0.25));
        // Mid-run geometry but a run that stops at the arc (an
        // authored end-arc) is not a jump either.
        assert!(!is_stored_jump_arc(

            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            None,
            0.041_666_7,
            0.041_666_7,
        ));
        // Radius evidence (CodeRabbit rounds 1–2 on #121): the shape
        // checks alone cannot tell an AUTHORED semicircle between two
        // straight runs from a stored jump — a 1-inch arc with A=−0.5
        // matches every geometric test. The page radius decides.
        assert!(!is_stored_jump_arc(
            -0.5,
            (1.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7, // the page's radius is far smaller: authored ink
            0.5,
        ));
        assert!(is_stored_jump_arc(
            -0.5,
            (1.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.5, // the page's radius matches: a stored jump
            0.5,
        ));
        // Writer rounding rides inside the 5% band.
        assert!(is_stored_jump_arc(
            -0.041_666_67,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_691, // 0.125 × 0.66667 / 2, the No Style values
            0.041_666_691,
        ));
        // No radius evidence (0) never strips — preserve the arc.
        assert!(!is_stored_jump_arc(

            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.0,
            0.0,
        ));
        // Round 3 (PR #299 review): only the CHOSEN axis's radius must
        // be positive — a FactorY=0 page still recognizes horizontal
        // stored jumps, and a zero on the arc's own axis never strips.
        assert!(is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.0,
        ));
        assert!(!is_stored_jump_arc(
            -0.041_666_7,
            (0.0, 0.083_333_3),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            0.041_666_7,
            0.0,
        ));
    }

    /// Round 3 (PR #299 review): the cluster reach is the crossing's
    /// OWN axis radius — the unrelated axis's radius must not merge
    /// two crossings the drawn arcs never overlap. ONE pair must
    /// produce BOTH crossings (clustering merges within a pair only),
    /// and the weights stay tiny so the stroke-width reach cannot
    /// mask the radius difference (round-4 review).
    #[test]
    fn line_jumps_clusters_on_the_crossing_axis_radius() {
        let lines = [
            // The jumper: one horizontal run.
            JumpLine {
                id: 1,
                z: 1,
                weight: 0.01,
                color: (0, 0, 0),
                style: JumpStyle::Arc,
                pts: vec![(0.0, 1.0), (2.0, 1.0)],
                breaks: Vec::new(),
                radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
                local_rot: 0.0,
            },
            // The crossed line: a U crossing the run TWICE — at x=1
            // and x=1.5. One pair, two crossings 0.5 in apart.
            JumpLine {
                id: 2,
                z: 2,
                weight: 0.01,
                color: (0, 0, 0),
                style: JumpStyle::Arc,
                pts: vec![(1.0, 0.0), (1.0, 2.0), (1.5, 2.0), (1.5, 0.0)],
                breaks: Vec::new(),
                radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
                local_rot: 0.0,
            },
        ];
        // X radius tiny, Y radius huge: the horizontal hops draw at
        // the X radius, so 0.5 in apart they stay TWO hops — the old
        // max-axis reach (1.0 in) merged them into one hop at their
        // midpoint, x=1.25, where no crossing exists.
        let js = line_jumps(&lines, (0.041_666_7, 0.5));
        assert_eq!(js.len(), 2, "no unrelated-axis clustering: {js:?}");
        assert!(
            js.iter().all(|j| j.dir.1.abs() < 1e-9),
            "the horizontal line hops: {js:?}"
        );
        // And the hops sit ON the crossings, not at a centroid.
        assert!(
            js.iter().any(|j| (j.at.0 - 1.0).abs() < 1e-9)
                && js.iter().any(|j| (j.at.0 - 1.5).abs() < 1e-9),
            "the hops stay at their crossings: {js:?}"
        );
    }

    /// A hop on a ZERO-radius axis never enters a cluster (CodeRabbit
    /// round on #324 phase 2): an always-jump connector crossing the
    /// same line on its horizontal leg and, a stroke's width away, on
    /// its vertical leg used to let the undrawable horizontal
    /// candidate absorb the valid vertical hop — the merged direction
    /// then drew nothing at r = 0.
    #[test]
    fn zero_axis_hops_never_enter_a_cluster() {
        // The jumper: horizontal leg INTO the bend, then a vertical
        // leg up — ConLineJumpCode 2, it hops at both crossings. The
        // under line crosses the horizontal leg at (0.428, 0.5) and
        // the vertical one at (0.4, 0.528) — 0.0396 in apart, inside
        // the stroke-width reach (2 × (0.012 + 0.012)), so the FIRST
        // (horizontal) candidate used to swallow the second.
        let j = JumpLine {
            id: 1,
            z: 0,
            weight: 0.012,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: vec![(1.0, 0.5), (0.4, 0.5), (0.4, 1.0)],
            breaks: Vec::new(),
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        let u = JumpLine {
            id: 2,
            z: 1,
            weight: 0.012,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: vec![(0.35, 0.578), (0.5, 0.428)],
            breaks: Vec::new(),
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        // X radius 0 (no horizontal hops can draw), Y radius 0.05.
        let (mut j, mut u) = (j.clone(), u.clone());
        j.radii = (0.0, 0.05);
        u.radii = (0.0, 0.05);
        let js = line_jumps_with(
            &[j.clone(), u.clone()],
            &JumpRules {
                page_code: 1,
                con_codes: [(1, 2)].into_iter().collect(),
            },
        );
        assert_eq!(js.len(), 1, "the vertical-leg hop alone: {js:?}");
        assert!(js[0].dir.0.abs() < 1e-9, "the survivor is vertical: {js:?}");
        assert_eq!(js[0].jumper, 1);
        // The same geometry under DEFAULT rules, both radii live: the
        // L hops on its horizontal leg AND the other line hops over
        // the L's vertical leg — one PAIR, two jumpers, 0.0396 in
        // apart. The cluster must keep them apart (merging stole the
        // second connector's hop and moved the first's cut off its
        // shaft — CodeRabbit round on #324 phase 3).
        let js = line_jumps(&[j.clone(), u.clone()], (0.05, 0.05));
        let mut jumpers: Vec<i64> = js.iter().map(|x| x.jumper).collect();
        jumpers.sort_unstable();
        assert_eq!(jumpers, vec![1, 2], "each connector keeps its hop: {js:?}");
        assert!(
            js.iter().any(|x| x.jumper == 1 && x.dir.1.abs() < 1e-9)
                && js.iter().any(|x| x.jumper == 2 && x.under.0.abs() < 1e-9),
            "the L hops along its horizontal leg, the line over the L's vertical one: {js:?}"
        );
    }

    /// #294 review: a crossing exactly ON an interior vertex of the
    /// under polyline still hops — the strict-interior intersection
    /// used to drop it (and with stored jump arcs straightened, the
    /// replacement hop vanished with it). A crossing at a connector's
    /// glue END never hops.
    #[test]
    fn line_jumps_hop_at_interior_vertices_not_at_glue_ends() {
        // The vertical's corner (2,1) sits exactly on the horizontal.
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 1.0), (3.0, 1.0)]),
                jline(2, 1, &[(2.0, 0.0), (2.0, 1.0), (2.0, 2.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(
            js.len(),
            1,
            "the T-junction at the interior vertex hops: {js:?}"
        );
        // The same corner AT the horizontal's glue end: no hop.
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 1.0), (2.0, 1.0)]),
                jline(2, 1, &[(2.0, 0.0), (2.0, 1.0), (2.0, 2.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a glue end never hops: {js:?}");
        // A V that only TOUCHES the horizontal (both runs on the same
        // side) is a tangency, not a crossing — no hop (#294 review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 0.0), (3.0, 0.0)]),
                jline(2, 1, &[(0.0, 1.0), (1.0, 0.0), (2.0, 1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a tangency never hops: {js:?}");
        // BOTH routes turning at the shared contact: the four runs
        // must alternate by route around the vertex. An L meeting a V
        // groups two L-runs and two V-runs — a touch, no hop (#294
        // review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)]),
                jline(2, 1, &[(-1.0, 1.0), (0.0, 0.0), (-1.0, -1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a two-corner touch never hops: {js:?}");
        // Two routes cornering THROUGH each other interleave — a real
        // crossing hops ONCE. The vertical runs THROUGH the corner on
        // both sides, so every window pair pairs one of the corner's
        // runs against a vertical one and line 1 (the flatter on both
        // its runs) is the jumper throughout — mirrored corner runs
        // would hand each line one window pair and draw two hops on
        // one crossing (the same-jumper cluster rule keeps them apart;
        // #324 phase-3 review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
                jline(2, 1, &[(0.0, 1.0), (0.0, 0.0), (0.0, -1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "interleaved corner runs cross: {js:?}");
        assert_eq!(js[0].jumper, 1, "the cornering route hops: {js:?}");
    }

    /// #334: a connector that arrives at a vertex of another and leaves
    /// ALONG its run — or runs in along it and leaves — MERGES; it never
    /// crosses. The contact used to hop whenever the tie between the
    /// two shared rays happened to sort into an "alternating" order (a
    /// join heading east hopped, westward did not), and the one-vertex
    /// branch — a route meeting a line that runs straight through —
    /// hopped every time. Genuine crossings at a vertex keep hopping.
    #[test]
    fn shared_runs_merge_without_a_hop() {
        let hops = |lines: &[JumpLine]| line_jumps(lines, (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS));
        let a = vec![(-1.0, 0.0), (0.0, 0.0), (1.0, 0.0)];
        // Both routes turn at the contact.
        for (k, b) in [
            // arrives from the north, joins eastward
            vec![(0.0, 1.0), (0.0, 0.0), (1.0, 0.0)],
            // … westward
            vec![(0.0, 1.0), (0.0, 0.0), (-1.0, 0.0)],
            // runs in along A from the east, leaves north
            vec![(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)],
            // … from the west
            vec![(-1.0, 0.0), (0.0, 0.0), (0.0, 1.0)],
        ]
        .into_iter()
        .enumerate()
        {
            let js = hops(&[jline(1, 0, &a), jline(2, 1, &b)]);
            assert!(js.is_empty(), "merge {k} is not a crossing: {js:?}");
        }
        // Floating-point noise on the shared run — it reached the
        // vertex through the other route's own frame — must not reopen
        // the hop; one sign of the skew sorted into a hop before.
        for skew in [1e-13_f64, -1e-13_f64] {
            let (s, c) = skew.to_radians().sin_cos();
            let b = vec![(0.0, 1.0), (0.0, 0.0), (c, s)];
            let js = hops(&[jline(1, 0, &a), jline(2, 1, &b)]);
            assert!(js.is_empty(), "noise on a shared run (skew {skew}): {js:?}");
        }
        // Only ONE route turns at the contact; the other runs straight
        // through it. A plain T-junction — and this branch always hopped.
        let east = vec![(0.0, -1.0), (0.0, 0.0), (1.0, 0.0)];
        let west = vec![(0.0, -1.0), (0.0, 0.0), (-1.0, 0.0)];
        for (k, lines) in [
            [jline(1, 0, &east), jline(2, 1, &[(-1.0, 0.0), (2.0, 0.0)])],
            [jline(1, 0, &east), jline(2, 1, &[(2.0, 0.0), (-1.0, 0.0)])],
            [jline(1, 0, &west), jline(2, 1, &[(-2.0, 0.0), (2.0, 0.0)])],
            [jline(1, 0, &[(-2.0, 0.0), (2.0, 0.0)]), jline(2, 1, &east)],
        ]
        .into_iter()
        .enumerate()
        {
            let js = hops(&lines);
            assert!(
                js.is_empty(),
                "merge {k} onto a through line is not a crossing: {js:?}"
            );
        }
        // Controls — what must not change. A real crossing at a vertex
        // of both routes still hops once (the vertical runs through;
        // the cornering route is flatter on both its runs — see the
        // twin case in line_jumps_hop_at_interior_vertices…).
        let js = hops(&[
            jline(1, 0, &[(-1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
            jline(2, 1, &[(0.0, 1.0), (0.0, 0.0), (0.0, -1.0)]),
        ]);
        assert_eq!(js.len(), 1, "interleaved corner runs still cross: {js:?}");
        // …so does a line leaving A's run at a visible angle DOWNWARD
        // (it passes through to A's other side)…
        let js = hops(&[
            jline(1, 0, &a),
            jline(2, 1, &[(0.0, 1.0), (0.0, 0.0), (0.99995, -0.01)]),
        ]);
        assert_eq!(js.len(), 1, "a shallow real crossing still hops: {js:?}");
        // …while its mirror image, leaving upward, only touches.
        let js = hops(&[
            jline(1, 0, &a),
            jline(2, 1, &[(0.0, 1.0), (0.0, 0.0), (0.99995, 0.01)]),
        ]);
        assert!(js.is_empty(), "a touch never hops: {js:?}");
        // …and a crossing is not a shared run for running 5e-10 in
        // BESIDE the other line. The contact tolerance reads the
        // near-miss as a vertex contact, but the runs are on different
        // lines: this hopped before #334, in both branches, and must
        // keep hopping (CodeRabbit round on the fix).
        let piercing = vec![(0.0, 1.0), (0.0, -5e-10), (1.0, -5e-10)];
        for (k, through) in [
            vec![(-2.0, 0.0), (2.0, 0.0)],
            vec![(-2.0, 0.0), (0.0, 0.0), (2.0, 0.0)],
        ]
        .into_iter()
        .enumerate()
        {
            let js = hops(&[jline(1, 0, &through), jline(2, 1, &piercing)]);
            assert_eq!(js.len(), 1, "a near-miss is a crossing, case {k}: {js:?}");
        }
        // …and the mirror near-miss — the vertex just ABOVE the line,
        // the outgoing run grazing it — crossed too, until the shared-
        // run branch read the grazing far end alone as a merge
        // (CodeRabbit round on #324 phase 2: a merge needs the vertex
        // ON the line; this route came from below and left above).
        let grazing = vec![(0.0, -1.0), (0.0, 5e-10), (0.01, 5e-12)];
        for (k, through) in [
            vec![(-2.0, 0.0), (2.0, 0.0)],
            vec![(-2.0, 0.0), (0.0, 0.0), (2.0, 0.0)],
        ]
        .into_iter()
        .enumerate()
        {
            let js = hops(&[jline(1, 0, &through), jline(2, 1, &grazing)]);
            assert_eq!(js.len(), 1, "a grazing near-miss is a crossing, case {k}: {js:?}");
        }
    }

    /// Visio's jump rules (the Visio 2021 witnesses b01 and c02,
    /// #322): page `LineJumpCode` 1 hops the flatter segment —
    /// steepness, not class, so the more horizontal of two STEEP
    /// lines hops too (60° over 84°, the `d04` witness) — and
    /// equally steep segments never hop; 2 mirrors it; 3 hops the
    /// later line, 5 the earlier. `ConLineJumpCode` 1 never, 2
    /// always, 3 the other line, 4 neither.
    #[test]
    fn jump_rules_follow_visio() {
        let h = jline(1, 0, &[(0.0, 1.0), (2.0, 1.0)]);
        let v = jline(2, 1, &[(1.0, 0.0), (1.0, 2.0)]);
        // Two STEEP lines that cross (60° through the origin, an 84°
        // one beside it) — both vertical-class under a |dx| ≥ |dy|
        // reading, so this pair pins steepness, not class.
        let s60 = jline(3, 2, &[(-1.0, -1.7320508), (1.0, 1.7320508)]);
        let s84 = jline(4, 3, &[(0.3, -1.0), (0.6, 1.8542526)]);
        // The mirrored X: equal steepness, opposite signs.
        let x1 = jline(5, 4, &[(0.0, 0.0), (2.0, 1.6)]);
        let x2 = jline(6, 5, &[(0.0, 1.6), (2.0, 0.0)]);
        let r = (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS);
        let rules = |page: i64, codes: &[(i64, i64)]| JumpRules {
            page_code: page,
            con_codes: codes.iter().copied().collect(),
        };
        let hops = |ls: &[JumpLine], ru: &JumpRules| {
            let mut filled = ls.to_vec();
            for l in &mut filled {
                l.radii = r;
            }
            line_jumps_with(&filled, ru)
        };
        // Page 1: the horizontal hops over the vertical…
        let js = hops(&[h.clone(), v.clone()], &rules(1, &[]));
        assert!(js.len() == 1 && js[0].dir.1.abs() < 1e-9, "{js:?}");
        // …the FLATTER (60°) of two steep lines hops — the d04
        // witness, where a class reading would draw no hop…
        let js = hops(&[s60.clone(), s84.clone()], &rules(1, &[]));
        assert!(
            js.len() == 1 && js[0].jumper == s60.id,
            "the 60° link hops over the 84° one: {js:?}"
        );
        // …and equally steep segments (the mirrored X) never hop.
        assert!(hops(&[x1.clone(), x2.clone()], &rules(1, &[])).is_empty());
        // Page 2: the vertical hops; the steeper (84°) hops; the X
        // still never hops.
        let js = hops(&[h.clone(), v.clone()], &rules(2, &[]));
        assert!(js.len() == 1 && js[0].dir.0.abs() < 1e-9, "{js:?}");
        let js = hops(&[s60.clone(), s84.clone()], &rules(2, &[]));
        assert!(
            js.len() == 1 && js[0].jumper == s84.id,
            "the 84° link hops over the 60° one: {js:?}"
        );
        assert!(hops(&[x1.clone(), x2.clone()], &rules(2, &[])).is_empty());
        // Page 3 hops the X too — the LATER line; page 5 the earlier.
        let js = hops(&[x1.clone(), x2.clone()], &rules(3, &[]));
        assert_eq!((js.len(), js[0].jumper), (1, x2.id), "{js:?}");
        let js = hops(&[x1.clone(), x2.clone()], &rules(5, &[]));
        assert_eq!((js.len(), js[0].jumper), (1, x1.id), "{js:?}");
        // Connector codes on the horizontal line under page 1.
        let on_h = |code: i64| hops(&[h.clone(), v.clone()], &rules(1, &[(1, code)]));
        assert_eq!(on_h(1).len(), 0, "never");
        assert!(on_h(2)[0].dir.1.abs() < 1e-9, "always: the horizontal hops");
        assert!(
            on_h(3)[0].dir.0.abs() < 1e-9,
            "other line: the vertical hops"
        );
        assert_eq!(on_h(4).len(), 0, "neither");
    }

    /// `page_jump_rules` reads what Visio reads — the PageSheet's
    /// `LineJumpCode` through the style chain, the connectors'
    /// resolved `ConLineJumpCode`s — and jump candidacy is
    /// `ObjType = 2`: a plain glued 1D line (static `PAR(PNT` glue,
    /// no ObjType) never joins the crossing set (#322's witness),
    /// while the master chain still supplies the marker for real
    /// connectors (#174, covered end to end by the b01/c02 fixtures).
    #[test]
    fn page_jump_rules_read_visio_and_gate_on_objtype() {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.set_cell("LineJumpCode", Cell::value("2"));
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            children: vec![vsdx::model::IndexChild::Sheet {
                tag: "PageSheet".into(),
                sheet,
            }],
            ..vsdx::Page::default()
        };
        let mut a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(a.clone());
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        let rules = page_jump_rules(&doc, &page);
        assert_eq!(rules.page_code, 2, "the PageSheet cell is read");
        assert!(rules.con_codes.is_empty(), "absent codes stay absent");
        // Page 2 hops the vertical.
        let js = page_jumps(&doc, &page, &[]);
        assert!(js.len() == 1 && js[0].jumper == 2, "{js:?}");
        // The horizontal's own ConLineJumpCode overrides the page
        // rule: 2 forces it to hop, 3 hands the hop to the vertical,
        // 1 is its OWN veto only (the page-2 jumper B still hops over
        // it — c02's witness is the veto on the WOULD-BE jumper), and
        // 4 kills the hop at its crossings outright.
        for (code, expect) in [(2, vec![1]), (3, vec![2]), (1, vec![2])] {
            a.sheet
                .set_cell("ConLineJumpCode", Cell::value(format!("{code}")));
            page.shapes[0] = a.clone();
            let js = page_jumps(&doc, &page, &[]);
            let mut got: Vec<i64> = js.iter().map(|j| j.jumper).collect();
            got.sort_unstable();
            assert_eq!(got, expect, "code {code}: {js:?}");
            assert_eq!(
                page_jump_rules(&doc, &page).con_codes.get(&1),
                Some(&code),
                "the connector code resolves through the chain"
            );
        }
        a.sheet.set_cell("ConLineJumpCode", Cell::value("4"));
        page.shapes[0] = a.clone();
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "code 4: no hop at its crossings"
        );
        // A plain glued 1D line — static PAR(PNT glue, ObjType absent
        // — is NOT a jump candidate: nothing hops over it and it hops
        // over nothing, where the old glue-ownership gate drew one.
        let mut g = straight_connector(7, (1.0, 0.5), (5.0, 0.5), None);
        g.sheet
            .children
            .retain(|c| !matches!(c, vsdx::model::SheetChild::Cell(n, _) if n == "ObjType"));
        let mut glue = Cell::value("1.0");
        glue.f = Some("PAR(PNT(Sheet.99!Connections.X1,Sheet.99!Connections.Y1))".into());
        g.sheet.set_cell("BeginX", glue);
        page.shapes[0] = g;
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "an ObjType-0 glued line never joins the crossing set"
        );
    }

    /// The page's Show-Line-Jumps switch gates the crossing set AT THE
    /// SOURCE (#324 phase-3 review): a forced `ConLineJumpCode = 2`
    /// must not draw or save a hop on a page whose jumps are off — and
    /// a `LinePattern = 0` shaft paints nothing, so nothing hops over
    /// it (the renderer's stroke-visibility twin of the zero-weight
    /// corollary).
    #[test]
    fn page_jumps_gates_on_the_page_switch_and_invisible_shafts() {
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        let mut h = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        h.sheet.set_cell("ConLineJumpCode", Cell::value("2"));
        let mut v = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(h);
        page.shapes.push(v.clone());
        let doc = vsdx::Document::default();
        assert_eq!(
            page_jumps(&doc, &page, &[]).len(),
            1,
            "forced hop on a default page"
        );
        // Jumps off (visPLOJumpNone): no hop survives the forced code.
        let sheet = vsdx::model::Sheet::default();
        page.children = vec![vsdx::model::IndexChild::Sheet {
            tag: "PageSheet".into(),
            sheet,
        }];
        page.sheet_mut()
            .expect("page sheet")
            .set_cell("LineJumpCode", Cell::value("0"));
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "the off switch beats the forced connector code"
        );
        // Back on, but the UNDER shaft paints nothing: no crossing.
        page.sheet_mut()
            .expect("page sheet")
            .set_cell("LineJumpCode", Cell::value("1"));
        v.sheet.set_cell("LinePattern", Cell::value("0"));
        page.shapes[1] = v;
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "nothing hops over an invisible shaft"
        );
    }

    /// #324 phase 4: the hop carries its JUMPER's resolved style —
    /// the page's `LineJumpStyle`, overridden per connector by a
    /// nonzero `ConLineJumpStyle`.
    #[test]
    fn page_jumps_resolves_the_hop_style() {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.set_cell("LineJumpStyle", Cell::value("3"));
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            children: vec![vsdx::model::IndexChild::Sheet {
                tag: "PageSheet".into(),
                sheet,
            }],
            ..vsdx::Page::default()
        };
        let mut a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(a.clone());
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        assert_eq!(page_jump_style(&doc, &page), JumpStyle::Square);
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!((js.len(), js[0].style), (1, JumpStyle::Square));
        // The connector's own code overrides the page: 4 = the
        // 2-sided triangle; 0 = the page default.
        for (code, want) in [
            ("4", JumpStyle::Sides(2)),
            ("0", JumpStyle::Square),
            ("9", JumpStyle::Sides(7)),
        ] {
            a.sheet.set_cell("ConLineJumpStyle", Cell::value(code));
            page.shapes[0] = a.clone();
            let js = page_jumps(&doc, &page, &[]);
            assert_eq!((js.len(), js[0].style), (1, want), "code {code}");
        }
    }

    /// PR #339 round 2: a duplicate endpoint right before a stored
    /// gap must not clobber the incoming direction — the crossing
    /// inside the slot still bridges.
    #[test]
    fn pr339_duplicate_endpoint_before_a_gap_keeps_the_bridge() {
        let r = 0.125 * (2.0 / 3.0) / 2.0;
        let rows = |rows: &[(&str, f64, f64)]| {
            let mut s = vsdx::model::Shape {
                id: 1,
                type_: Some("Shape".into()),
                ..vsdx::model::Shape::default()
            };
            for (n, v) in [
                ("PinX", 0.0),
                ("PinY", 0.0),
                ("Width", 2.0),
                ("Height", 3.0),
                ("LocPinX", 0.0),
                ("LocPinY", 0.0),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = rows
                .iter()
                .enumerate()
                .map(|(k, (t, x, y))| Row {
                    t: Some((*t).into()),
                    ix: k as i64 + 1,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x}"))),
                        ("Y".to_string(), Cell::value(format!("{y}"))),
                    ],
                    ..Row::default()
                })
                .collect();
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        // A NORTHBOUND run with a DUPLICATE endpoint just before the
        // stored gap's MoveTo, and the probe THROUGH the gap's
        // interior (page y = 1 — the slot spans 1−r..1+r; the old
        // fixture's probe landed on the solid run at page y = 2 and
        // its incoming run read southbound, so the assertion passed
        // either way; CodeRabbit, PR #339 round 3).
        let gapped = rows(&[
            ("MoveTo", 1.0, 0.0),
            ("LineTo", 1.0, 1.0 - r),
            ("LineTo", 1.0, 1.0 - r), // duplicate
            ("MoveTo", 1.0, 1.0 + r),
            ("LineTo", 1.0, 3.0),
        ]);
        let mut v = rows(&[("MoveTo", 0.0, 0.0), ("LineTo", 2.0, 0.0)]);
        v.id = 2;
        for (n, val) in [
            ("PinX", 0.0),
            ("PinY", 1.0),
            ("Width", 2.0),
            ("Height", 0.0),
            ("LocPinX", 0.0),
            ("LocPinY", 0.0),
        ] {
            v.sheet.set_cell(n, Cell::value(format!("{val}")));
        }
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(gapped);
        page.shapes.push(v);
        let doc = vsdx::Document::default();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1, "the duplicate does not kill the gap: {js:?}");
    }

    /// PR #339 round 4: a second pen move supplies no outgoing run —
    /// the gap recognition must stop at it (Greptile's scenario:
    /// M(0,0) L(1,0) | M(1+2r,0) | M(3,0) L(4,0)).
    #[test]
    fn pr339_next_dir_stops_at_a_second_pen_move() {
        let r = 0.125 * (2.0 / 3.0) / 2.0;
        let rows = |rows: &[(&str, f64, f64)]| {
            let mut s = vsdx::model::Shape {
                id: 1,
                type_: Some("Shape".into()),
                ..vsdx::model::Shape::default()
            };
            for (n, v) in [
                ("PinX", 0.0),
                ("PinY", 0.0),
                ("Width", 4.0),
                ("Height", 0.0),
                ("LocPinX", 0.0),
                ("LocPinY", 0.0),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = rows
                .iter()
                .enumerate()
                .map(|(k, (t, x, y))| Row {
                    t: Some((*t).into()),
                    ix: k as i64 + 1,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x}"))),
                        ("Y".to_string(), Cell::value(format!("{y}"))),
                    ],
                    ..Row::default()
                })
                .collect();
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        let gapped = rows(&[
            ("MoveTo", 0.0, 0.0),
            ("LineTo", 1.0, 0.0),
            ("MoveTo", 1.0 + 2.0 * r, 0.0),
            ("MoveTo", 3.0, 0.0),
            ("LineTo", 4.0, 0.0),
        ]);
        // The probe crosses the first blank span (1, 1+2r) — with a
        // pen-move-supplied outgoing run it bridged and hopped there.
        let mut v = rows(&[("MoveTo", 1.0 + r, -1.0), ("LineTo", 1.0 + r, 1.0)]);
        v.id = 2;
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(gapped);
        page.shapes.push(v);
        let doc = vsdx::Document::default();
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "a second pen move supplies no outgoing run"
        );
    }

    /// PR #339 round 4: a NoShow FORMULA that now evaluates to 0
    /// keeps the connector in the crossing set (the stale NoShow=1
    /// cache must not hide a visible line).
    #[test]
    fn pr339_noshow_formula_beats_the_stale_cache() {
        let mut a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        {
            for child in a.sheet.children.iter_mut() {
                if let vsdx::model::SheetChild::Section(sec) = child {
                    if sec.name == "Geometry" {
                        sec.cells.push(("NoShow".to_string(), {
                            let mut c = Cell::value("1");
                            c.f = Some("GUARD(0)".into());
                            c
                        }));
                    }
                }
            }
        }
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(a);
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        assert_eq!(
            page_jumps(&doc, &page, &[]).len(),
            1,
            "the formula-visible connector stays in the crossing set"
        );
    }

    /// PR #339 round 3: a COLLAPSED pen-up move (the MoveTo point
    /// duplicates its predecessor) dedups away — its break must
    /// VANISH, not shift onto the next inked segment (a crossing on
    /// that segment keeps its hop).
    #[test]
    fn pr339_collapsed_break_drops_not_shifts() {
        let r = (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS);
        let collapsed = JumpLine {
            id: 1,
            z: 0,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: vec![(0.0, 1.0), (1.0, 1.0), (1.0, 1.0), (2.0, 1.0)],
            breaks: vec![2],
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        let v = jline(2, 1, &[(1.5, 0.0), (1.5, 2.0)]);
        let js = line_jumps(&[collapsed, v], r);
        assert_eq!(
            js.len(),
            1,
            "the crossing on the REAL segment (1,1)->(2,1) hops: {js:?}"
        );
        assert!((js[0].at.0 - 1.5).abs() < 1e-9, "{js:?}");
    }

    /// PR #339 round 3: a pen move to a NEW stroke clears the saved
    /// direction — a later gap cannot borrow the first stroke's run
    /// and bridge a blank span (Greptile's scenario: M(0,0) L(1,0) |
    /// M(3,0) L(3,0) | M(3+2r,0) L(5,0)).
    #[test]
    fn pr339_pen_move_clears_the_saved_direction() {
        let r = 0.125 * (2.0 / 3.0) / 2.0;
        let rows = |rows: &[(&str, f64, f64)]| {
            let mut s = vsdx::model::Shape {
                id: 1,
                type_: Some("Shape".into()),
                ..vsdx::model::Shape::default()
            };
            for (n, v) in [
                ("PinX", 0.0),
                ("PinY", 0.0),
                ("Width", 5.0),
                ("Height", 0.0),
                ("LocPinX", 0.0),
                ("LocPinY", 0.0),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = rows
                .iter()
                .enumerate()
                .map(|(k, (t, x, y))| Row {
                    t: Some((*t).into()),
                    ix: k as i64 + 1,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x}"))),
                        ("Y".to_string(), Cell::value(format!("{y}"))),
                    ],
                    ..Row::default()
                })
                .collect();
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        let gapped = rows(&[
            ("MoveTo", 0.0, 0.0),
            ("LineTo", 1.0, 0.0),
            ("MoveTo", 3.0, 0.0),
            ("LineTo", 3.0, 0.0),
            ("MoveTo", 3.0 + 2.0 * r, 0.0),
            ("LineTo", 5.0, 0.0),
        ]);
        // The probe crosses the PHANTOM span (3, 3+2r) — blank ink.
        // With the borrowed direction it hopped here; with the clear
        // the window is suppressed and nothing else crosses (the real
        // final segment starts at 3+2r).
        let mut v = rows(&[("MoveTo", 3.0 + r, -1.0), ("LineTo", 3.0 + r, 1.0)]);
        v.id = 2;
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(gapped);
        page.shapes.push(v);
        let doc = vsdx::Document::default();
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "the +2r pen move does not borrow the first stroke's direction"
        );
    }

    /// PR #339 E: a stored gap on the run AFTER a corner bridges in
    /// the crossing set — the run INTO the gap is the corner's
    /// outgoing leg (the vertical), not the incoming horizontal the
    /// off-by-one used to read.
    #[test]
    fn pr339_page_jumps_bridges_a_gap_after_a_bend() {
        let r = 0.125 * (2.0 / 3.0) / 2.0;
        let rows = |rows: &[(&str, f64, f64)]| {
            let mut s = vsdx::model::Shape {
                id: 1,
                type_: Some("Shape".into()),
                ..vsdx::model::Shape::default()
            };
            for (n, v) in [
                ("PinX", 0.0),
                ("PinY", 0.0),
                ("Width", 2.0),
                ("Height", 3.0),
                ("LocPinX", 0.0),
                ("LocPinY", 0.0),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = rows
                .iter()
                .enumerate()
                .map(|(k, (t, x, y))| Row {
                    t: Some((*t).into()),
                    ix: k as i64 + 1,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x}"))),
                        ("Y".to_string(), Cell::value(format!("{y}"))),
                    ],
                    ..Row::default()
                })
                .collect();
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        // The route: east, north to the slot start, a stored gap
        // (MoveTo at +2r on the VERTICAL run), then on north.
        let gapped = rows(&[
            ("MoveTo", 0.0, 0.0),
            ("LineTo", 1.0, 0.0),
            ("LineTo", 1.0, 1.0 - r),
            ("MoveTo", 1.0, 1.0 + r),
            ("LineTo", 1.0, 3.0),
        ]);
        // The probe passes through the GAP's interior (page y = 1 —
        // the slot spans 1−r..1+r; the old probe landed at page y = 2
        // on the solid run; CodeRabbit, PR #339 round 3).
        let mut v = rows(&[("MoveTo", 0.0, 0.0), ("LineTo", 2.0, 0.0)]);
        v.id = 2;
        for (n, val) in [
            ("PinX", 0.0),
            ("PinY", 1.0),
            ("Width", 2.0),
            ("Height", 0.0),
            ("LocPinX", 0.0),
            ("LocPinY", 0.0),
        ] {
            v.sheet.set_cell(n, Cell::value(format!("{val}")));
        }
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(gapped);
        page.shapes.push(v);
        let doc = vsdx::Document::default();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(
            js.len(),
            1,
            "the crossing inside the after-corner gap slot: {js:?}"
        );
    }

    /// PR #339 review regressions.
    #[test]
    fn pr339_break_remap_after_dedup_and_gap_after_a_bend() {
        let r = (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS);
        // D: a duplicate point before a stroke break — the break must
        // remap to the NORMALIZED index (the phantom window stays
        // skipped, the final segment's real crossing is found).
        let dup_break = JumpLine {
            id: 1,
            z: 0,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: vec![(0.0, 1.0), (1.0, 1.0), (1.0, 1.0), (3.0, 1.0), (4.0, 1.0)],
            breaks: vec![3],
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        let v = jline(2, 1, &[(3.5, 0.0), (3.5, 2.0)]);
        let js = line_jumps(&[dup_break, v], r);
        assert_eq!(
            js.len(),
            1,
            "the real crossing on the final segment: {js:?}"
        );
        assert!((js[0].at.0 - 3.5).abs() < 1e-9, "{js:?}");
        // And nothing hops on the blank bridge at x 1..3 — with the
        // control: WITHOUT the break the same crossing produces a hop
        // (the assertion discriminates; CodeRabbit, PR #339 round 3).
        let v_early = jline(2, 1, &[(2.0, 0.0), (2.0, 2.0)]);
        let broken_line = JumpLine {
            id: 1,
            z: 0,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Arc,
            pts: vec![(0.0, 1.0), (1.0, 1.0), (1.0, 1.0), (3.0, 1.0), (4.0, 1.0)],
            breaks: vec![3],
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        assert!(
            line_jumps(&[broken_line.clone(), v_early.clone()], r).is_empty(),
            "no hop in blank space after the dedup"
        );
        let whole = JumpLine {
            breaks: Vec::new(),
            ..broken_line
        };
        assert_eq!(
            line_jumps(&[whole, v_early], r).len(),
            1,
            "the control: without the break the crossing hops"
        );
        // E: a stored gap on the run AFTER a corner — the run INTO the
        // gap is the vertical leg (the horizontal used to win and the
        // gap was rejected).
        let gapped = JumpLine {
            id: 1,
            z: 0,
            weight: 0.01,
            color: (0, 0, 0),
            style: JumpStyle::Gap,
            pts: vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0 - TEST_JUMP_RADIUS),
                (1.0, 1.0 + TEST_JUMP_RADIUS),
                (1.0, 3.0),
            ],
            breaks: vec![3], // the (unrecognized-by-the-test) gap slot
            radii: (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
            local_rot: 0.0,
        };
        let v2 = jline(2, 1, &[(1.0, 0.5), (3.0, 0.5)]);
        let js = line_jumps(&[gapped, v2], r);
        // The vertical run at x=1 crosses y=0.5 inside the gap slot —
        // the break keeps it OUT of the crossing set (an authored
        // break here), pinning that the break still applies after a
        // corner; the E fix itself is exercised through page_jumps'
        // bridge logic below.
        assert!(js.is_empty(), "the break applies: {js:?}");
    }

    /// Authored stroke breaks stay out of the crossing set, and a
    /// stored GAP slot bridges (#324 phase 5): the hit walk marks the
    /// pen-up bridge; page_jumps drops unrecognized bridges (a hop in
    /// blank space between unrelated strokes) and keeps a recognized
    /// 2r slot so the crossing inside it recomputes.
    #[test]
    fn page_jumps_skips_authored_breaks_and_bridges_stored_gaps() {
        let connector = |id: i64, rows: &[(&str, f64, f64)]| {
            let mut s = vsdx::model::Shape {
                id,
                type_: Some("Shape".into()),
                ..vsdx::model::Shape::default()
            };
            for (n, v) in [
                ("PinX", 2.5),
                ("PinY", 3.0),
                ("Width", 5.0),
                ("Height", 0.0),
                ("LocPinX", 2.5),
                ("LocPinY", 0.0),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = rows
                .iter()
                .enumerate()
                .map(|(k, (t, x, y))| Row {
                    t: Some((*t).into()),
                    ix: k as i64 + 1,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x}"))),
                        ("Y".to_string(), Cell::value(format!("{y}"))),
                    ],
                    ..Row::default()
                })
                .collect();
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        let v = connector(2, &[("MoveTo", 0.0, 0.0), ("LineTo", 0.0, 6.0)]);
        let v = {
            let mut s = v;
            for (n, val) in [("PinX", 2.5), ("PinY", 3.0), ("Width", 0.0), ("Height", 6.0), ("LocPinX", 0.0), ("LocPinY", 3.0)] {
                s.sheet.set_cell(n, Cell::value(format!("{val}")));
            }
            s
        };
        // Two disjoint strokes (x 0..1 and x 3..4 in local — page x
        // 0..1 and 3..4 around pin 2.5... build with local rows): the
        // vertical at local x 2 crosses ONLY the phantom bridge — no
        // hop may appear.
        let broken = connector(
            1,
            &[
                ("MoveTo", 0.0, 0.0),
                ("LineTo", 1.0, 0.0),
                ("MoveTo", 3.0, 0.0),
                ("LineTo", 4.0, 0.0),
            ],
        );
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(broken);
        page.shapes.push(v.clone());
        let doc = vsdx::Document::default();
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "no hop in blank space between unrelated strokes"
        );
        // A stored GAP slot: strokes ending/beginning 2r apart on one
        // run — the vertical crossing INSIDE the slot recomputes (the
        // reconcile rewrites the gap there).
        let r = 0.125 * (2.0 / 3.0) / 2.0;
        let gapped = connector(
            1,
            &[
                ("MoveTo", 0.0, 0.0),
                ("LineTo", 2.5 - r, 0.0),
                ("MoveTo", 2.5 + r, 0.0),
                ("LineTo", 5.0, 0.0),
            ],
        );
        let mut page2 = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page2.shapes.push(gapped);
        page2.shapes.push(v);
        let js = page_jumps(&doc, &page2, &[]);
        assert_eq!(js.len(), 1, "the crossing inside the stored gap: {js:?}");
        assert!(
            (js[0].at.0 - 2.5).abs() < 1e-9,
            "the hop sits at the slot's crossing: {:?}",
            js[0].at
        );
    }

    /// A shared-VERTEX contact seen through different window pairs
    /// (each route bends at the contact, each picking its own jumper
    /// per pair of legs) is ONE crossing: the candidates at the same
    /// point merge to one hop, whichever jumper's window came first
    /// (no witness for the choice; CodeRabbit round on #324 phase 4).
    #[test]
    fn shared_vertex_contacts_hop_once() {
        // A: (-1, 0.5) → (0, 0) → (1, -3); B: (-0.5, -1) → (0, 0) →
        // (0.5, 1) — both bend through the origin.
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 0.5), (0.0, 0.0), (1.0, -3.0)]),
                jline(2, 1, &[(-0.5, -1.0), (0.0, 0.0), (0.5, 1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert!(
            js.iter().map(|j| j.at).all(|a| {
                js.iter()
                    .filter(|k| (k.at.0 - a.0).hypot(k.at.1 - a.1) < 1e-6)
                    .count()
                    == 1
            }) && !js.is_empty(),
            "one hop per contact point: {js:?}"
        );
    }

    /// The stored styled-hop recognition (#324 phase 4): the
    /// witnessed `POLYLINE` forms and the gap's MoveTo skip, against
    /// authored art with the same row types.
    #[test]
    fn stored_styled_hop_rows_classify() {
        // The witnessed square controls (fractions of 5 × 0.25).
        let sq = "POLYLINE(0, 0, 0.49166666666667,0.66666666666667, 0.50833333333333,0.66666666666667)";
        let tri = "POLYLINE(0, 0, 0.5,0.66666666666667)";
        let r2 = 2.0 * (0.125 * (2.0 / 3.0) / 2.0);
        for a in [sq, tri] {
            assert!(
                is_stored_jump_polyline(
                    a,
                    (2.458_333_3, 0.125),
                    (r2, 0.0),
                    Some((1.0, 0.0)),
                    Some((1.0, 0.0)),
                    (5.0, 0.25),
                    r2 / 2.0,
                    r2 / 2.0,
                ),
                "{a} is a stored hop"
            );
        }
        // Off the radius (the chord is not the slot) — authored art.
        assert!(!is_stored_jump_polyline(
            sq,
            (2.458_333_3, 0.125),
            (0.5, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            (5.0, 0.25),
            r2 / 2.0,
            r2 / 2.0,
        ));
        // Controls outside the box — authored art.
        let wild = "POLYLINE(0, 0, 0.5,2.0)";
        assert!(!is_stored_jump_polyline(
            wild,
            (2.458_333_3, 0.125),
            (r2, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            (5.0, 0.25),
            r2 / 2.0,
            r2 / 2.0,
        ));
        // No continuing runs — a corner polyline.
        assert!(!is_stored_jump_polyline(
            sq,
            (2.458_333_3, 0.125),
            (r2, 0.0),
            None,
            Some((1.0, 0.0)),
            (5.0, 0.25),
            r2 / 2.0,
            r2 / 2.0,
        ));
        // Local-typed controls are not the stored form.
        assert_eq!(parse_polyline_controls("POLYLINE(1, 0, 1, 1)"), None);
        assert_eq!(parse_polyline_controls("POLYLINE(0, 0)"), None);
        assert_eq!(parse_polyline_controls("not a polyline"), None);
        // The gap: a collinear ≈2r skip with continuing runs.
        assert!(is_stored_jump_gap(
            (r2, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            r2 / 2.0,
            r2 / 2.0
        ));
        // A cornering MoveTo (the next run turns) is a second stroke,
        // not a gap hop.
        assert!(!is_stored_jump_gap(
            (r2, 0.0),
            Some((1.0, 0.0)),
            Some((0.0, 1.0)),
            r2 / 2.0,
            r2 / 2.0
        ));
        // A skip off the radius.
        assert!(!is_stored_jump_gap(
            (0.5, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            r2 / 2.0,
            r2 / 2.0
        ));
    }

    /// #187: two crossing connectors produce one hop; a rotated
    /// jumper crosses too (slice 2 — the Angle gate is gone).
    #[test]
    fn page_jumps_crosses_axis_and_rotated() {
        let connector = |id: i64, pin: (f64, f64), w: f64, h: f64, angle: f64| {
            let mut s = Shape {
                id,
                type_: Some("Shape".into()),
                ..Shape::default()
            };
            for (n, v) in [
                ("PinX", pin.0),
                ("PinY", pin.1),
                ("Width", w),
                ("Height", h),
                ("LocPinX", w / 2.0),
                ("LocPinY", h / 2.0),
                ("Angle", angle),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let row = |ix: i64, x: f64, y: f64| Row {
                t: Some("LineTo".into()),
                ix,
                cells: vec![
                    ("X".to_string(), Cell::value(format!("{x}"))),
                    ("Y".to_string(), Cell::value(format!("{y}"))),
                ],
                ..Row::default()
            };
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![
                        ("X".to_string(), Cell::value("0")),
                        ("Y".to_string(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                row(2, w, h),
            ];
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        // A horizontal connector (1,1)..(5,1) and a 45°-rotated one
        // through the same page area.
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(connector(1, (3.0, 1.0), 4.0, 0.0, 0.0));
        page.shapes.push(connector(
            2,
            (3.0, 1.0),
            3.0,
            0.0,
            -std::f64::consts::FRAC_PI_4,
        ));
        let doc = vsdx::Document::default();
        let jumps = page_jumps(&doc, &page, &[]);
        assert_eq!(jumps.len(), 1, "one crossing: {jumps:?}");
        // The more-horizontal line hops: the AXIS line jumps (dy 0
        // beats the 45° line's 0.707); the rotated one is the under.
        let j = &jumps[0];
        assert!(
            j.dir.1.abs() < 1e-6 && j.under.0.abs() > 0.5,
            "the axis line hops over the rotated one: dir {:?} under {:?}",
            j.dir,
            j.under
        );
    }

    /// A straight two-row `ObjType=2` connector from `from` to `to`
    /// (page inches, y-up), with an optional `LineColor` cell value.
    fn straight_connector(id: i64, from: (f64, f64), to: (f64, f64), color: Option<&str>) -> Shape {
        let (w, h) = (to.0 - from.0, to.1 - from.1);
        let mut s = Shape {
            id,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", (from.0 + to.0) / 2.0),
            ("PinY", (from.1 + to.1) / 2.0),
            ("Width", w),
            ("Height", h),
            ("LocPinX", w / 2.0),
            ("LocPinY", h / 2.0),
            ("ObjType", 2.0),
        ] {
            s.sheet.set_cell(n, Cell::value(format!("{v}")));
        }
        if let Some(c) = color {
            s.sheet.set_cell("LineColor", Cell::value(c));
        }
        let row = |ix: i64, t: &str, x: f64, y: f64| Row {
            t: Some(t.into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![row(1, "MoveTo", 0.0, 0.0), row(2, "LineTo", w, h)];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    }

    /// #333: `color_value` passes a `#…` LineColor through verbatim, so
    /// a malformed one reaches the crossing set — and must land in the
    /// gray fallback, never panic. `#aé000` is six bytes with `é`
    /// straddling offset 2, `#000é0` straddles offset 4: the fixed
    /// slices panicked inside the character on both.
    #[test]
    fn malformed_line_colors_fall_back_to_gray() {
        for v in [
            "#a\u{e9}000",
            "#000\u{e9}0",
            "#gggggg",
            "#12345",
            "ff0000",
            "",
            "#+0ff00",
            "#ff00+0",
            "#-0ff00",
            "# 0ff00",
            "#ff00 0",
        ] {
            assert_eq!(parse_hex_rgb(v), None, "{v:?}");
        }
        assert_eq!(parse_hex_rgb("#ff0000"), Some((0xff, 0, 0)));
        assert_eq!(parse_hex_rgb("#00Ff0A"), Some((0x00, 0xff, 0x0a)));
        // Through the crossing set: the malformed connector is the
        // jumper (it lies flatter), and both strokes of its hop
        // resolve — its own to the gray fallback, the other's to its
        // real color.
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(straight_connector(
            1,
            (1.0, 1.0),
            (5.0, 1.0),
            Some("#a\u{e9}000"),
        ));
        page.shapes.push(straight_connector(
            2,
            (3.0, 0.0),
            (3.0, 2.0),
            Some("#ff0000"),
        ));
        let doc = vsdx::Document::default();
        let jumps = page_jumps(&doc, &page, &[]);
        assert_eq!(jumps.len(), 1, "one crossing: {jumps:?}");
        assert_eq!(
            (jumps[0].jumper, jumps[0].color, jumps[0].under_color),
            (1, (0x42, 0x42, 0x42), (0xff, 0, 0))
        );
    }

    /// The crossing set's inputs resolve the way the renderer does, and
    /// hidden layers leave it. The canvas overlay draws from this same
    /// function (#336), so both are its to keep: a palette-index
    /// jumper's hop is its own color, never the gray fallback, and
    /// nothing hops over a connector on a hidden layer.
    #[test]
    fn page_jumps_resolves_palette_colors_and_skips_hidden_layers() {
        // LineColor as a legacy palette index — 2 is red.
        let mut crossing = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        crossing.sheet.set_cell("LayerMember", Cell::value("3"));
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes
            .push(straight_connector(1, (1.0, 1.0), (5.0, 1.0), Some("2")));
        page.shapes.push(crossing);
        let doc = vsdx::Document::default();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1, "one crossing: {js:?}");
        assert_eq!(
            (js[0].jumper, js[0].color),
            (1, (0xff, 0, 0)),
            "the palette-index jumper hops in its own color: {js:?}"
        );
        // A layer nothing sits on changes nothing; hiding the layer the
        // crossed connector sits on takes it out of the set.
        assert_eq!(page_jumps(&doc, &page, &[7]).len(), 1);
        assert!(
            page_jumps(&doc, &page, &[3]).is_empty(),
            "nothing hops over a hidden connector"
        );
    }
    /// PR #339 round 5: a NEAR-DIAGONAL stored slot's axis pick can
    /// flip across six-decimal row rounding — the styled classifiers
    /// take the arc classifier's both-radii rule instead of the `>=`
    /// tie (a 45° run saved at the X radius was once re-read against
    /// Y and rejected).
    #[test]
    fn near_diagonal_styled_slots_match_either_axis_radius() {
        // A 45° skip whose dy edges out dx: the >= tie picks Y, but the
        // slot was WRITTEN at the X radius; the Y cells match nothing.
        let (dx, dy): (f64, f64) = (0.0707106781186547, 0.070710678118655);
        let skip = (dx, dy);
        let len = dx.hypot(dy);
        let rx = len / 2.0; // the writer's slot
        let ry = 0.10; // the axis the tie would pick — off-band
        let dir = (dx / len, dy / len);
        assert!(
            is_stored_jump_gap(skip, Some(dir), Some(dir), rx, ry),
            "the near-diagonal gap classifies at the radius that wrote it"
        );
        assert!(
            !is_stored_jump_gap(skip, Some(dir), Some(dir), 0.07, 0.10),
            "off-band on both axes still rejects"
        );
        // The polyline twin: a Sides(2) apex written at rx over the
        // same chord, fractions of a 1 × 1 frame.
        let start = (0.2, 0.1);
        let u = (dx / len, dy / len);
        let n = (-u.1, u.0);
        let apex = (
            start.0 + u.0 * len / 2.0 + n.0 * rx,
            start.1 + u.1 * len / 2.0 + n.1 * rx,
        );
        let a = format!("POLYLINE(0, 0, {},{})", apex.0, apex.1);
        assert!(
            is_stored_jump_polyline(&a, start, skip, Some(dir), Some(dir), (1.0, 1.0), rx, ry),
            "the near-diagonal polygon classifies at the radius that wrote it"
        );
        assert!(
            !is_stored_jump_polyline(&a, start, skip, Some(dir), Some(dir), (1.0, 1.0), 0.07, 0.10),
            "off-band on both axes still rejects"
        );
    }


    /// CodeRabbit round 6 on PR #339: crossings on two PARALLEL
    /// strokes of one connector (an authored break between) never
    /// merge — they are not one continuous run, and the averaged
    /// centroid once landed a hop in the blank span between the
    /// strokes where neither exists.
    #[test]
    fn parallel_strokes_of_one_connector_keep_their_own_hops() {
        let mut n_route = jline(1, 0, &[(0.0, 0.0), (4.0, 0.0), (0.0, 0.04), (4.0, 0.04)]);
        n_route.breaks = vec![2]; // the pen move onto the second stroke
        let v = jline(2, 1, &[(1.0, -1.0), (1.0, 1.0)]);
        let js = line_jumps_with(
            &[n_route, v],
            &JumpRules {
                page_code: 1,
                con_codes: std::collections::HashMap::new(),
            },
        );
        assert_eq!(js.len(), 2, "each stroke keeps its own hop: {js:?}");
        assert!(
            js.iter()
                .any(|j| (j.at.1 - 0.0).abs() < 1e-6)
                && js.iter().any(|j| (j.at.1 - 0.04).abs() < 1e-6),
            "the hops sit ON the strokes, not between them: {js:?}"
        );
    }



    /// CodeRabbit round 7 on PR #339: candidates at the same contact
    /// may disagree on ROOM — the leg-end candidate (cap 0, neither
    /// painter nor writer can place it) once won the cluster and the
    /// placeable through-run candidate was discarded. The roomier one
    /// wins now.
    #[test]
    fn the_roomier_candidate_wins_a_shared_contact() {
        // A bends through the origin: its incoming leg is FLATTER than
        // the 45° under line (A hops there — a leg end, cap 0), its
        // outgoing leg is STEEPER (B hops — B passes through, room on
        // both sides).
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let a = jline(1, 0, &[(-1.0, 0.5), (0.0, 0.0), (1.0, -3.0)]);
        let b = jline(2, 1, &[(-0.5, -0.5), (0.5, 0.5)]);
        let js = line_jumps_with(&[a, b], &rules);
        assert_eq!(js.len(), 1, "one contact, one hop: {js:?}");
        assert!(
            (js[0].at.0).abs() < 1e-9 && (js[0].at.1).abs() < 1e-9,
            "at the contact"
        );
        assert!(
            js[0].cap > 1e-9,
            "the placeable through-run candidate won: {:?}",
            js[0]
        );
        assert_eq!(js[0].jumper, 2, "B hops (the steep leg's crossing)");
    }


    /// Greptile round 8 on PR #339: the run room's forward bounds
    /// check reads the vertex the next segment STARTS at — the
    /// boundary's own segment belongs to the next stroke, but the
    /// segment ENDING at the boundary is this run's (the n+1 check
    /// once stopped one segment early and erased a valid hop).
    #[test]
    fn the_forward_bounds_check_stops_at_the_boundary_segment() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let mut joined = jline(
            1,
            0,
            &[(0.0, 0.0), (0.2, 0.0), (1.0, 0.0), (1.0, 0.0), (2.0, 0.0)],
        );
        joined.breaks = vec![3]; // the zero-length pen move at (1, 0)
        let near = jline(2, 1, &[(0.19, -1.0), (0.19, 1.0)]);
        let js = line_jumps_with(&[joined, near], &rules);
        assert_eq!(js.len(), 1);
        // cap = min(back 0.19, fwd 0.81) — the FORWARD room now
        // reaches the boundary vertex (the old n+1 check stopped at
        // the first collinear segment and left cap = 0.01, under the
        // radius: the hop vanished).
        assert!(
            (js[0].cap - 0.19).abs() < 1e-9,
            "the forward room reaches the boundary vertex: {:?}",
            js[0].cap
        );
    }

    /// Greptile round 8 on PR #339: a same-POINT merge keeps the
    /// RETAINED candidate's cap — the conservative min once zeroed a
    /// valid hop when the later candidate was the leg-end twin (point
    /// order alone decided whether the hop drew).
    #[test]
    fn a_reversed_bent_line_keeps_its_valid_hop() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        // The roomier test's bent line, REVERSED: the first candidate
        // hops the diagonal with room on both sides; the second (the
        // leg-end twin, cap 0) must not zero it.
        let a = jline(1, 0, &[(1.0, -3.0), (0.0, 0.0), (-1.0, 0.5)]);
        let b = jline(2, 1, &[(-0.5, -0.5), (0.5, 0.5)]);
        let js = line_jumps_with(&[a, b], &rules);
        assert_eq!(js.len(), 1, "one contact, one hop: {js:?}");
        assert!(
            js[0].cap > 1e-9,
            "point order does not erase the valid hop: {:?}",
            js[0]
        );
    }

    /// CodeRabbit round 7 on PR #339: a hop that CANNOT draw reserves
    /// no space — a cap-starved crossing at the run's start once
    /// reserved the slot and suppressed a later valid hop the writer
    /// would have kept (neither crossing got a hop).
    #[test]
    fn an_undrawable_hop_reserves_no_space() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let run = jline(1, 0, &[(0.0, 0.0), (1.0, 0.0)]);
        let mut run = run;
        run.radii = (0.05, 0.05);
        let near = jline(2, 1, &[(0.005, -1.0), (0.005, 1.0)]);
        let far = jline(3, 1, &[(0.105, -1.0), (0.105, 1.0)]);
        let js = line_jumps_with(&[run, near, far], &rules);
        assert_eq!(js.len(), 2, "both crossings stay in the set: {js:?}");
        // The painter drops the starved one; the drawable one survives
        // the spacing rule.
        let drawable: Vec<_> = js
            .iter()
            .filter(|j| j.radii.0.min((j.cap - j.weight * 0.5).max(0.0)) > j.weight)
            .collect();
        assert_eq!(drawable.len(), 1, "exactly the roomy hop draws: {js:?}");
        assert!((drawable[0].at.0 - 0.105).abs() < 1e-9);
    }

    /// CodeRabbit round 7 on PR #339: a stroke that ENDS on the other
    /// line touches it, never crosses — the blank post-break window
    /// once served as merely_touches' outgoing ray.
    #[test]
    fn a_stroke_ending_on_the_line_never_hops() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let mut bent = jline(
            1,
            0,
            &[(0.0, -1.0), (0.0, 0.0), (0.0, 1.0), (1.0, 1.0)],
        );
        bent.breaks = vec![2]; // the second stroke starts at (0, 1)
        let through = jline(2, 1, &[(-1.0, 0.0), (1.0, 0.0)]);
        let js = line_jumps_with(&[bent, through], &rules);
        assert!(
            js.is_empty(),
            "the leg-end contact is a touch, not a crossing: {js:?}"
        );
    }

    /// CodeRabbit round 7 on PR #339: a ZERO-LENGTH pen move keeps its
    /// run boundary through deduplication — the room stops at the
    /// merged vertex instead of running the whole preceding stroke.
    #[test]
    fn a_zero_length_pen_move_keeps_its_run_boundary() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let mut joined = jline(1, 0, &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.0), (2.0, 0.0)]);
        joined.breaks = vec![2];
        let near = jline(2, 1, &[(1.02, -1.0), (1.02, 1.0)]);
        let js = line_jumps_with(&[joined.clone(), near], &rules);
        assert_eq!(js.len(), 1);
        assert!(
            (js[0].cap - 0.02).abs() < 1e-9,
            "the room stops at the stroke boundary: {:?}",
            js[0].cap
        );
        // The crossing beyond the boundary still hops.
        let mid = jline(2, 1, &[(1.5, -1.0), (1.5, 1.0)]);
        let js = line_jumps_with(&[joined, mid], &rules);
        assert_eq!(js.len(), 1, "the mid-stroke crossing hops");
    }

    /// Greptile round 7 on PR #339: the radius pair is chosen PER
    /// CROSSING from the run's direction rotated into the connector's
    /// frame — a 30° page run under a 60° frame is local −30°, both
    /// X-dominant, so the pair stays UNSWAPPED (the binary
    /// per-connector swap once flipped it).
    #[test]
    fn a_diagonal_run_under_a_rotated_frame_keeps_its_pair() {
        let rules = JumpRules {
            page_code: 1,
            con_codes: std::collections::HashMap::new(),
        };
        let (c30, s30) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let mut diag = jline(
            1,
            0,
            &[(0.0, 0.0), (c30 * 2.0, s30 * 2.0)],
        );
        diag.radii = (0.05, 0.10);
        diag.local_rot = 60f64.to_radians();
        let v = jline(2, 1, &[(0.8, -1.0), (0.8, 1.0)]);
        let js = line_jumps_with(&[diag, v], &rules);
        assert_eq!(js.len(), 1);
        assert_eq!(
            js[0].radii,
            (0.05, 0.10),
            "both X-dominant — unswapped: {:?}",
            js[0]
        );
    }

    /// PR #339 round 5: the hop carries its run's own room (`Jump.cap`)
    /// — a crossing 0.05 above an L's corner caps the slot at 0.05 so
    /// the cut and the bridge stay on the crossing leg; a mid-run
    /// crossing carries the full room to both ends.
    #[test]
    fn a_corner_adjacent_crossing_carries_its_run_room() {
        let rules = JumpRules {
            page_code: 2, // vertical lines jump — the L's steep leg hops
            con_codes: std::collections::HashMap::new(),
        };
        let l = jline(1, 0, &[(0.0, 1.0), (1.0, 1.0), (1.0, 3.0)]);
        let near = jline(2, 1, &[(0.5, 1.05), (1.5, 1.05)]);
        let js = line_jumps_with(&[l, near], &rules);
        assert_eq!(js.len(), 1);
        assert_eq!(js[0].jumper, 1);
        assert!(
            (js[0].cap - 0.05).abs() < 1e-9,
            "the room stops at the corner: {:?}",
            js[0].cap
        );
        // A collinear row boundary is NOT a turn: the room runs
        // through the split (a stored hop itself splits its run).
        let split = jline(
            1,
            0,
            &[(0.0, 1.0), (1.0, 1.0), (1.0, 2.0), (1.0, 2.5), (1.0, 3.0)],
        );
        let mid = jline(2, 1, &[(0.5, 2.2), (1.5, 2.2)]);
        let js = line_jumps_with(&[split, mid], &rules);
        assert_eq!(js.len(), 1);
        assert!(
            (js[0].cap - 0.8).abs() < 1e-9,
            "the room spans the collinear rows to the NEARER end (the forward leg is shorter): {:?}",
            js[0].cap
        );
        // The FORWARD end bounds too: a crossing 0.05 below the top of
        // a plain 2-inch leg caps at 0.05 (CodeRabbit round 6 — the
        // forward scan once counted the crossing's own segment twice
        // and reported 1.95).
        let leg = jline(1, 0, &[(1.0, 1.0), (1.0, 3.0)]);
        let top = jline(2, 1, &[(0.5, 2.95), (1.5, 2.95)]);
        let js = line_jumps_with(&[leg, top], &rules);
        assert_eq!(js.len(), 1);
        assert!(
            (js[0].cap - 0.05).abs() < 1e-9,
            "the room stops at the forward end: {:?}",
            js[0].cap
        );
    }
}

