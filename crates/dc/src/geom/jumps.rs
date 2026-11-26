//! The connector-crossing math: the page's resolved jump radii, the
//! stored-jump classifier, the crossing set (`line_jumps`), and the
//! page walk that feeds it (`page_jumps`).

use super::connector::is_connector_in;
use super::hitframe::hit_frame;
use super::segments::geometry_hit_segments_straight;

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
/// `radius` is the page's own jump radius ([`jump_radius_in`])
/// expressed in THIS frame — the shape-local units the sagitta and
/// chord are measured in (the caller divides by the frame's
/// perpendicular-axis scale when the rows live in master units). The
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

/// The two routes' runs at a SHARED contact vertex alternate around
/// it (r1, r2, r1, r2 in angular order) — they cross; grouped-by-
/// route rays mean the routes merely touch (#294 review).
fn alternating(vi: usize, a: &[(f64, f64)], vj: usize, b: &[(f64, f64)]) -> bool {
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

/// The two runs at the INTERIOR vertex `vx` sit on the same side of
/// the other segment's line — the polyline touches it and bounces
/// back (a tangency), it does not cross (#294 review).
fn same_side(vx: usize, pts: &[(f64, f64)], other: &[(f64, f64)]) -> bool {
    let (a, b) = (other[0], other[1]);
    let dir = (b.0 - a.0, b.1 - a.1);
    let side = |p: (f64, f64)| dir.0 * (p.1 - a.1) - dir.1 * (p.0 - a.0);
    let (s1, s2) = (side(pts[vx - 1]), side(pts[vx + 1]));
    // Pure sign comparison — a magnitude cutoff would wave through
    // shallow same-side contacts on short segments (#294 review).
    s1 * s2 > 0.0
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
                    // and bounce back (a tangency), never a hop (#294
                    // review). When BOTH routes turn at the contact,
                    // the four incident runs must alternate by route
                    // around the vertex; grouped-by-route rays mean
                    // the routes merely touch.
                    let vi = hugged_vertex(t, k1).filter(|&ix| ix > 0 && ix + 1 < pts[i].len());
                    let vj = hugged_vertex(u, k2).filter(|&ix| ix > 0 && ix + 1 < pts[j].len());
                    match (vi, vj) {
                        (Some(vi), Some(vj)) => {
                            if !alternating(vi, &pts[i], vj, &pts[j]) {
                                continue;
                            }
                        }
                        (Some(vi), None) => {
                            if same_side(vi, &pts[i], w2) {
                                continue;
                            }
                        }
                        (None, Some(vj)) => {
                            if same_side(vj, &pts[j], w1) {
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

/// The page's connector crossings (#187 export parity): every
/// top-level connector's polyline — the same `hit_frame` +
/// [`geometry_hit_segments_straight`] walk the canvas jump overlay
/// uses, formula-preferred, restricted to connectors whose geometry
/// is entirely straight runs (#324 step 1: Visio never hops curved
/// connectors and never hops over one) — crossed with Visio's rule
/// ([`line_jumps`]: the more-horizontal line hops, ties to the later
/// z). Hidden-layer members and guides skip like the render walk.
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
            pts,
        });
    }
    line_jumps(&lines, (radius, radius_y))
}
