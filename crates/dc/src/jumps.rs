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

use crate::geom::{geometry_hit_segments, hit_frame, is_connector_in};

pub struct JumpLine {
    pub id: i64,
    pub z: usize,
    pub weight: f64,
    /// (r, g, b) — the stroke color both overlays repaint.
    pub color: (u8, u8, u8),
    pub pts: Vec<(f64, f64)>,
}

/// A computed hop (#108): where the jumping line crosses the under
/// line, both directions (unit), and both strokes. Visio's default
/// crossing treatment — the more-horizontal line hops (ties: the
/// later in z), arc style. The overlay itself persists nothing;
/// dc-ui's `ReconcileLineJumps` is what writes these hops into the
/// jumper's Geometry rows.
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

/// A stored line-jump arc (#294): Visio saves a connector's jump as an
/// `ArcTo` row ON the orthogonal run — the chord lies along one axis,
/// the arc is a SEMICIRCLE over the gap (`|A| = radius = chord / 2`
/// exactly; the #294 repro's row: chord 0.0833, A −0.0416667 — the
/// band admits writer rounding), and the route continues collinear on
/// both sides (`prev_dir`/`next_dir`: the neighboring segments'
/// directions; `None` = no such run). Corner arcs cut ACROSS the run
/// (a diagonal chord), authored shallow bumps fall far below the
/// band, and whole-route semicircles have no continuing runs — all
/// stay curves. The callers additionally gate on connector-ness so
/// artwork arcs never strip. Once stripped, the route is straight at
/// the old arc and the jump overlay draws the ONE arc at the current
/// crossing with the page's own radius — a stored arc stops lingering
/// after a move (#294).
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
    // band, the axis-aligned chord, the straight-run context, the
    // connector gate and the page-radius evidence already separate
    // jumps from artwork (#294 review).
    len > 1e-9
        && a >= 0.4 * len
        && a <= 0.6 * len
        && chord.0.abs().min(chord.1.abs()) <= len * 0.02
        && run_continues(prev_dir, chord)
        && run_continues(next_dir, chord)
        // The arc's own axis picks the evidence radius: Visio authors
        // a jump on a horizontal run at the X cells, on a vertical run
        // at the Y cells (PR #299 review — one shared radius fails a
        // vertical stored jump whenever the page's Y spacing differs).
        // Only the CHOSEN axis's radius must be positive: a FactorY=0
        // page still recognizes horizontal stored jumps (PR #299
        // round-3 review); a zero on the chosen axis never strips.
        && {
            let radius = if chord.0.abs() >= chord.1.abs() {
                radius_x
            } else {
                radius_y
            };
            radius > 0.0 && (a - radius).abs() <= (radius * 0.05).max(1e-6)
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
fn merely_touches(vx: usize, pts: &[(f64, f64)], other: &[(f64, f64)]) -> bool {
    let (a, b) = (other[0], other[1]);
    let dir = (b.0 - a.0, b.1 - a.1);
    let side = |p: (f64, f64)| dir.0 * (p.1 - a.1) - dir.1 * (p.0 - a.0);
    let (back, fwd) = (pts[vx - 1], pts[vx + 1]);
    // Pure sign comparison — a magnitude cutoff would wave through
    // shallow same-side contacts on short segments (#294 review). A
    // run ON the line reads as side 0, which the product took for a
    // crossing — every merge onto a through line hopped. The vertex is
    // on the segment already (that is the contact), so the run is the
    // segment's own when its far end lies on its line too.
    side(back) * side(fwd) > 0.0
        || [back, fwd]
            .iter()
            .any(|&p| off_line(dir, a, p) <= SHARED_RUN_OFF_IN)
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

/// Every crossing hop among the lines (#108): pairwise proper
/// segment intersections; the more-horizontal segment jumps.
/// A contact where the two routes SHARE a run — one arrives at the
/// other's vertex and continues along it — is a merge, not a crossing
/// (#334).
/// `radius` is the page's resolved jump radius (inches —
/// [`jump_radius_in`]): it sizes the hop arc and bounds what counts
/// as one crossing REGION (#251) when converging crossings cluster.
pub fn line_jumps(lines: &[JumpLine], (radius, radius_y): (f64, f64)) -> Vec<Jump> {
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
                    // The more-horizontal line hops; a tie gives the
                    // later-in-z line the hop.
                    let i_jumps = d1.1.abs() < d2.1.abs()
                        || (d1.1.abs() - d2.1.abs()).abs() < 1e-9 && lines[i].z > lines[j].z;
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

/// The page's connector crossings (#187): every top-level connector's
/// polyline — `hit_frame` + [`geometry_hit_segments`], formula-
/// preferred, curve-aware — crossed with Visio's rule ([`line_jumps`]:
/// the more-horizontal line hops, ties to the later z). Hidden-layer
/// members and guides skip like the render walk. This is THE crossing
/// set: the canvas overlay, the export and the stored-jump
/// reconciliation all draw from it (#336).
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
        if !is_connector_in(doc, &chain, s) {
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
        let segs = geometry_hit_segments(s, master_root, &frame, Some((strip_r, strip_ry)));
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
        let color = parse_hex_rgb(&crate::svg::themed_color_string(doc, s, &chain, "LineColor"))
            .unwrap_or((0x42, 0x42, 0x42));
        lines.push(JumpLine {
            id: s.id,
            z,
            weight,
            color,
            pts,
        });
    }
    line_jumps(&lines, (radius, radius_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::{Cell, Row, Section, Shape, SheetChild};

    /// The tests' hop radius — Visio's default (0.125 in × 2/3 / 2,
    /// the #294 identity).
    const TEST_JUMP_RADIUS: f64 = 0.125 * (2.0 / 3.0) / 2.0;

    /// A plain black 0.01-in test line.
    fn jline(id: i64, z: usize, pts: &[(f64, f64)]) -> JumpLine {
        JumpLine {
            id,
            z,
            weight: 0.01,
            color: (0, 0, 0),
            pts: pts.to_vec(),
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
            pts: pts.to_vec(),
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
                pts: vec![(0.0, 1.0), (2.0, 1.0)],
            },
            // The crossed line: a U crossing the run TWICE — at x=1
            // and x=1.5. One pair, two crossings 0.5 in apart.
            JumpLine {
                id: 2,
                z: 2,
                weight: 0.01,
                color: (0, 0, 0),
                pts: vec![(1.0, 0.0), (1.0, 2.0), (1.5, 2.0), (1.5, 0.0)],
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
        // crossing hops.
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
                jline(2, 1, &[(1.0, 1.0), (0.0, 0.0), (-1.0, 0.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "interleaved corner runs cross: {js:?}");
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
        // of both routes still hops…
        let js = hops(&[
            jline(1, 0, &[(-1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
            jline(2, 1, &[(1.0, 1.0), (0.0, 0.0), (-1.0, 0.0)]),
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
}
