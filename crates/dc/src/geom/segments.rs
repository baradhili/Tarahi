//! The geometry hit walk: [`geometry_hit_segments`]'s two-pass row
//! evaluation and its render-parity row budget.

use super::hitframe::HitFrame;
use super::jumps::{is_stored_jump_arc, is_stored_jump_polyline};
use super::rows::row_cell_value;
use vsdx::model::{Row, Section, Shape};

/// One walked run: start and end in PAGE space.
pub type HitSeg = ((f64, f64), (f64, f64));

/// One walked geometry row of the hit walk: the row, its master
/// fallback, the section feed name, the type, the endpoint in the
/// LOCAL frame and the frame-scaled one, and the row's A/B/C/D
/// control cells CAPTURED at the row's own evaluation point — the
/// render (#249) reads a row's controls while the env carries the
/// feeds of the rows before it (and its own) only, so the hit walk
/// must too (PR #299 review: a two-pass read saw later rows' feeds,
/// and the clickable curve drifted from the drawn one after a
/// resize).
type EvaldRow<'a> = (
    &'a Row,
    Option<&'a Row>,
    &'a str,
    String,
    (f64, f64),
    (f64, f64),
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

/// The geometry walk's row budget: [`geometry_hit_segments`] reads at
/// most this many rows per section — a render-time horizon. Rows past
/// it are invisible to the walk (and to the jump overlay, which rides
/// the same walk), so a document edit that REPLACES rows from the
/// walk's output must skip longer sections or it erases their tails
/// (review on the #303 batch).
pub const MAX_WALK_ROWS: usize = 64;

/// The connector hit segments of a shape's geometry in PAGE space
/// (#173/#179): walks the shape's Geometry rows — merging inherited
/// rows and cells from the master — sampling arcs, and mapping every
/// point through the full placement frame (rotation + flips).
/// `strip_stored_jumps` (callers gate on connector-ness) treats
/// stored jump `ArcTo` rows as straight runs (#294) — the hit
/// polyline, the crossing set and the rendered ink all agree on the
/// route without the stale arc.
pub fn geometry_hit_segments(
    s: &Shape,
    master_root: Option<&Shape>,
    frame: &HitFrame,
    strip_stored_jumps: Option<(f64, f64)>,
) -> Vec<HitSeg> {
    geometry_hit_segments_straight(s, master_root, frame, strip_stored_jumps).0
}

/// [`geometry_hit_segments`] plus the shape's straight-run
/// eligibility (#324 step 1): the flag is `true` when every walked
/// row draws a STRAIGHT run — `MoveTo`/`LineTo` (Rel variants), an
/// `ArcTo` that classified as a stored jump arc and straightened,
/// or a zero-width `EllipticalArcTo` (an authored `D=0` draws its
/// endpoint chord, #233 — the ink is straight). Any other row — a
/// real arc or elliptical arc, splines, NURBS, polylines, infinite
/// lines, unknown `*To` rows drawn as chords — makes the shape
/// INELIGIBLE for the crossing set: Visio never hops a curved
/// connector and never hops over one (live-measured in Visio 2021,
/// mk-git 2026-11-25, #324). Stored hops need `strip_stored_jumps`
/// (the caller's radii) to classify; a walk without it counts every
/// `ArcTo` a curve. Rows past [`MAX_WALK_ROWS`] stay invisible to
/// the flag, as to the walk itself.
pub fn geometry_hit_segments_straight(
    s: &Shape,
    master_root: Option<&Shape>,
    frame: &HitFrame,
    strip_stored_jumps: Option<(f64, f64)>,
) -> (Vec<HitSeg>, bool) {
    let mut sections: Vec<&Section> = s.sheet.sections_named("Geometry");
    // (owning width/height for Rel rows, x/y scale). The scale only
    // applies when the WHOLE section is the master's (its cached
    // literals are in master units — those rows walk with
    // `master=None`, so the one scaling happens at `end`); per-cell
    // inheritance evaluates master formulas in instance terms and
    // scales only cached literals (#179) — the ratio below serves that
    // path in BOTH branches (CodeRabbit round 2 on #121: with an own
    // section, a master-fallback cached literal stayed in MASTER units
    // while the ink path's merge scaled it, so the two walks
    // disagreed on inherited inline arcs).
    let master_ratio = master_root.and_then(|_| {
        let ms = frame.master_size;
        if ms.0.abs() > 1e-6 && ms.1.abs() > 1e-6 {
            Some((frame.size.0 / ms.0, frame.size.1 / ms.1))
        } else {
            None
        }
    });
    let (own, scale) = if sections.is_empty() {
        let Some(root) = master_root else {
            return (Vec::new(), true);
        };
        sections = root.sheet.sections_named("Geometry");
        if sections.is_empty() {
            return (Vec::new(), true);
        }
        (frame.master_size, master_ratio.unwrap_or((1.0, 1.0)))
    } else {
        (frame.size, (1.0, 1.0))
    };
    // The master rows of the FIRST geometry section, for per-cell
    // inheritance and master-only rows (shape 1017-style connectors
    // inherit their MoveTo entirely).
    let master_rows: Vec<&Row> = master_root
        .and_then(|r| r.sheet.sections_named("Geometry").into_iter().next())
        .map(|g| g.rows.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let master_row = |ix: i64| master_rows.iter().find(|r| r.ix == ix).copied();
    // Per-cell master-fallback literals scale master→instance by the
    // size ratio in both branches (see the head note).
    let per_cell = master_ratio.unwrap_or((1.0, 1.0));
    let axis_scale = |cell: &str| {
        if cell == "X" || cell == "A" {
            per_cell.0
        } else {
            per_cell.1
        }
    };
    // The walk's OWN env copy: evaluated row coordinates feed back into
    // it section-qualified (`Geometry1.X2`), exactly like the ink path —
    // row-to-row references resolve against the LIVE route, not the
    // stale cached V (CodeRabbit round 2 on #121: the reroute engine's
    // `Geometry1.X2+…` controls kept the hit walk at the authored pose
    // while the SVG walked the resized route).
    let mut env = frame.env.clone();
    let cell =
        |env: &vsdx::formula::Env, row: &Row, master: Option<&Row>, name: &str| -> Option<f64> {
            row_cell_value(
                row.cell(name),
                master.and_then(|m| m.cell(name)),
                axis_scale(name),
                env,
            )
        };
    let to_page = |p: (f64, f64)| -> (f64, f64) { frame.to_page(p) };
    let quad = |c0: (f64, f64), ctrl: (f64, f64), c1: (f64, f64), u: f64| -> (f64, f64) {
        (
            (1.0 - u).powi(2) * c0.0 + 2.0 * u * (1.0 - u) * ctrl.0 + u * u * c1.0,
            (1.0 - u).powi(2) * c0.1 + 2.0 * u * (1.0 - u) * ctrl.1 + u * u * c1.1,
        )
    };
    let mut pts: Vec<(f64, f64)> = Vec::new();
    // Straight-run eligibility (#324 step 1): cleared by the first
    // row that draws real curve geometry (see the function doc).
    let mut straight = true;
    let mut cur: Option<(f64, f64)> = None; // shape-local
                                            // The same pen in the UNSCALED row frame — stored-jump
                                            // classification classifies there, matching the ink path (#294
                                            // review).
    let mut cur_local: Option<(f64, f64)> = None;
    // #188: the spline accumulator — SplineStart opens, SplineKnot
    // rows extend, anything else flushes; NURBSTo rows evaluate
    // standalone. The flattened samples ARE the hit polyline, so
    // hit-testing agrees with the rendered ink (crate::curve::flatten).
    let mut spline_acc: Option<crate::curve::SplineAcc> = None;
    // The ordered walk: the instance's rows, then master-only rows
    // (by ix) the instance never overrode. Each row carries its
    // section-qualified feed name (`Geometry1.X2`) — the ink path's
    // convention, by section enumeration order.
    let gnames: Vec<String> = (0..sections.len())
        .map(|si| format!("Geometry{}", si + 1))
        .collect();
    let mut walk: Vec<(&Row, Option<&Row>, &str)> = Vec::new();
    for (si, geo) in sections.iter().enumerate() {
        for row in geo.rows.iter().take(MAX_WALK_ROWS) {
            walk.push((row, master_row(row.ix), &gnames[si]));
        }
    }
    for r in master_rows.iter().take(MAX_WALK_ROWS) {
        if !walk.iter().any(|(row, _, _)| row.ix == r.ix) {
            walk.push((r, None, "Geometry1"));
        }
    }
    walk.sort_by_key(|(row, _, _)| row.ix);
    // `Del="1"` rows stay deleted (GitHub #293): the INSTANCE row's
    // marker wins when one is present, else the master-only row's own.
    // The #293 repro's connector inherits its master's hook row through
    // an instance `<Row Del="1" IX="3" T="LineTo"/>` with no cells.
    walk.retain(|(row, _, _)| !row.is_deleted());
    // Row kind: 0 = MoveTo (positions the pen, draws no run), 1 =
    // straight LineTo (the only run that counts as jump context —
    // curved neighbors' endpoint chords can lie about the route,
    // #294 review), 2 = anything else.
    let kind_of = |t: &str| -> u8 {
        if t == "MoveTo" || t == "RelMoveTo" {
            0u8
        } else if t == "LineTo" || t == "RelLineTo" {
            1
        } else {
            2
        }
    };
    // FIRST PASS — the ordered evaluation, the ink path's #249 rule:
    // each row's endpoint evaluates, feeds back into the env
    // section-qualified (`Geometry1.X2`), and only then do the row's
    // control cells read the env — a row sees the feeds of the rows
    // before it (and its own), never a later row's. The lookahead and
    // the classification both read these EVALUATED values, so
    // row-to-row references (`Geometry1.X2+0.083`, the reroute
    // engine's curved controls) resolve against the live route instead
    // of the stale cached V (CodeRabbit round 2 on #121: the ink walk
    // fed the env, the hit walk didn't, and a resized route stripped
    // in one walk but not the other).
    let mut evald: Vec<EvaldRow> = Vec::new();
    for (row, master, gname) in walk.iter().take(MAX_WALK_ROWS) {
        let t = row
            .t
            .as_deref()
            .or(master.and_then(|m| m.t.as_deref()))
            .unwrap_or("")
            .to_string();
        // Omitted geometry cells default to 0 (the corpus arcs omit Y
        // and inherit it); a row nothing resolves is still walked as a
        // 0-coordinate row — never dropped (#179).
        let x = cell(&env, row, *master, "X").unwrap_or(0.0);
        let y = cell(&env, row, *master, "Y").unwrap_or(0.0);
        let rel = t.starts_with("Rel");
        let end_local = if rel { (x * own.0, y * own.1) } else { (x, y) };
        // Master-section cached literals are in master units.
        let end = if scale != (1.0, 1.0) {
            (end_local.0 * scale.0, end_local.1 * scale.1)
        } else {
            end_local
        };
        env.insert(format!("{gname}.X{}", row.ix), end_local.0);
        env.insert(format!("{gname}.Y{}", row.ix), end_local.1);
        // The controls read NOW — the env carries feeds up to this
        // row's own endpoint, exactly the render's per-row order.
        let a = cell(&env, row, *master, "A");
        let b = cell(&env, row, *master, "B");
        let c = cell(&env, row, *master, "C");
        let d = cell(&env, row, *master, "D");
        evald.push((row, *master, gname, t, end_local, end, a, b, c, d));
    }
    // SECOND PASS — the geometry, against the evaluated values. The
    // route-context directions read LOCAL endpoints (the frame the
    // classification chord lives in) — the scaled frame would bend
    // collinearity under non-uniform instance scaling.
    // The master fallback rode the FIRST pass: every control read the
    // second pass makes is the captured per-row value (the round-3
    // review — a post-pass env read would sample later rows' feeds).
    for (k, (row, master, _gname, t, end_local, end, ca, cb, cc, cd)) in
        evald.iter().take(MAX_WALK_ROWS).enumerate()
    {
        let t = t.as_str();
        let end_local = *end_local;
        let end = *end;
        match t {
            "MoveTo" | "RelMoveTo" => {
                cur = Some(end);
                cur_local = Some(end_local);
                pts.push(to_page(end));
            }
            "ArcTo" | "RelArcTo" => {
                // |A| = sagitta, bulging right of travel; the
                // quadratic control that reproduces the apex.
                let raw_a = (*ca).unwrap_or(0.0);
                if let (Some(c0), Some(c0_local)) = (cur, cur_local) {
                    let d = (end.0 - c0.0, end.1 - c0.1);
                    // The classification chord in the LOCAL frame (the
                    // row coordinates before the section scale) — the
                    // same frame the ink path classifies in.
                    let d_class = (end_local.0 - c0_local.0, end_local.1 - c0_local.1);
                    // Route context (#294 review): the run continues
                    // collinear INTO the arc and OUT of it — Visio
                    // writes jumps mid-run. Only straight neighbors
                    // count (curved endpoint chords can lie about the
                    // route), and zero-length degenerate rows are
                    // skipped without crossing a MoveTo.
                    let mut prev_dir = None;
                    let mut pi = k as isize - 1;
                    while pi >= 1 {
                        let (_, _, _, pt, pe, _, _, _, _, _) = &evald[pi as usize];
                        if kind_of(pt.as_str()) != 1 {
                            break;
                        }
                        let (_, _, _, _, pp, _, _, _, _, _) = &evald[pi as usize - 1];
                        let pd = (pe.0 - pp.0, pe.1 - pp.1);
                        if pd.0.hypot(pd.1) > 1e-9 {
                            prev_dir = Some(pd);
                            break;
                        }
                        pi -= 1;
                    }
                    let mut next_dir = None;
                    let mut ni = k + 1;
                    while ni < evald.len() {
                        let (_, _, _, nt, ne, _, _, _, _, _) = &evald[ni];
                        if kind_of(nt.as_str()) != 1 {
                            break;
                        }
                        let p = if ni == k + 1 {
                            // The arc's LOCAL end — the lookahead must
                            // stay in the classification frame under
                            // instance scaling (#294 review).
                            end_local
                        } else {
                            evald[ni - 1].4
                        };
                        let nd = (ne.0 - p.0, ne.1 - p.1);
                        if nd.0.hypot(nd.1) > 1e-9 {
                            next_dir = Some(nd);
                            break;
                        }
                        ni += 1;
                    }
                    // #294: a stored jump arc straightens — the
                    // polyline follows the chord, so the crossing set
                    // never sees the stale bump. Classified in the
                    // LOCAL frame (unscaled row coordinates), the same
                    // frame the ink path classifies in — instance
                    // scaling must not make the walks disagree (#294
                    // review).
                    //
                    // The radius needs no frame conversion: every
                    // geometry frame is INCHES (master-local,
                    // instance-local, page — the instance/master ratios
                    // are geometric RESCALES applied after authoring),
                    // and the stored arc's A was authored at the page's
                    // radius. The comparison is authored inches against
                    // page inches in every frame.
                    let stored_jump = match strip_stored_jumps {
                        Some(r) => is_stored_jump_arc(raw_a, d_class, prev_dir, next_dir, r.0, r.1),
                        None => false,
                    };
                    if !stored_jump {
                        // A non-hop arc is real curve geometry: the
                        // connector leaves the crossing set's
                        // eligibility (#324 step 1).
                        straight = false;
                        // The bump is geometry in the scaled frame:
                        // the local sagitta scales with the local
                        // chord's perpendicular axis.
                        let a = raw_a
                            * if d_class.0.abs() >= d_class.1.abs() {
                                scale.1
                            } else {
                                scale.0
                            };
                        let len = d.0.hypot(d.1).max(1e-9);
                        let mid = (
                            (c0.0 + end.0) / 2.0 + d.1 / len * a,
                            (c0.1 + end.1) / 2.0 - d.0 / len * a,
                        );
                        let ctrl = (
                            2.0 * mid.0 - (c0.0 + end.0) / 2.0,
                            2.0 * mid.1 - (c0.1 + end.1) / 2.0,
                        );
                        for u in [0.25, 0.5, 0.75] {
                            pts.push(to_page(quad(c0, ctrl, end, u)));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "EllipticalArcTo" | "RelEllipticalArcTo" => {
                // (A,B) is an ON-curve point → quadratic control.
                // #233: an authored D=0 is a ZERO-WIDTH arc — the
                // renderer draws the endpoint chord, so the hit walk
                // follows the ink (a quadratic through an off-chord
                // control would test the doubled-back curve that is
                // never drawn; CodeRabbit, #278).
                let zero_width = (*cd).is_some_and(|d| d.abs() < 1e-9);
                let a = *ca;
                let b = *cb;
                if !zero_width {
                    // A real elliptical arc is curve geometry (#324
                    // step 1); a zero-width one draws its endpoint
                    // chord (#233) and stays straight.
                    straight = false;
                    if let (Some(a), Some(b), Some(c0)) = (a, b, cur) {
                        let ctrl = (
                            2.0 * a - (c0.0 + end.0) / 2.0,
                            2.0 * b - (c0.1 + end.1) / 2.0,
                        );
                        for u in [0.25, 0.5, 0.75] {
                            pts.push(to_page(quad(c0, ctrl, end, u)));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "SplineStart" => {
                // Spline rows are curve geometry (#324 step 1).
                straight = false;
                // #188: opens a spline (§2.4.2.15) — X/Y is the
                // second control point; A/B/C/D are second/first/
                // last knots and the degree.
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                spline_acc = Some(crate::curve::SplineAcc::start(
                    cur.unwrap_or(end),
                    end,
                    (*ca).unwrap_or(0.0),
                    (*cb).unwrap_or(0.0),
                    (*cc).unwrap_or(1.0),
                    (*cd).unwrap_or(3.0),
                ));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "SplineKnot" => {
                // Spline rows are curve geometry (#324 step 1) — a
                // stray knot without an open spline draws only its
                // endpoint, but the row type still says curve.
                straight = false;
                match spline_acc.as_mut() {
                    Some(acc) => {
                        acc.knot(end, (*ca).unwrap_or(0.0));
                        cur = Some(end);
                        cur_local = Some(end_local);
                    }
                    None => {
                        pts.push(to_page(end));
                        cur = Some(end);
                        cur_local = Some(end_local);
                    }
                }
            }
            "NURBSTo" => {
                // NURBS rows are curve geometry (#324 step 1).
                straight = false;
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                // §2.4.2.7: the E cell's NURBS() formula carries the
                // interior points (fractions of Width/Height when
                // xType/yType = 0) with their knots and weights; the
                // row cells frame them and X/Y is the endpoint.
                let e_text = row
                    .cell("E")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .unwrap_or_default();
                let mut drew = false;
                if let Some(f) = crate::curve::parse_nurbs(&e_text) {
                    // Per-axis, matching the renderer's corrected
                    // total (#218 review round): a fractional axis
                    // (type 0) lands directly in instance units —
                    // the frame's own dims, NO extra scale (`emit_pt`
                    // applies it exactly once); an absolute axis
                    // carries master-unit literals and takes only the
                    // scale. An inherited resized master keeps the
                    // hit polyline ON the rendered curve.
                    let (fx, fy) = (
                        if f.x_type == 0.0 {
                            frame.size.0
                        } else {
                            scale.0
                        },
                        if f.y_type == 0.0 {
                            frame.size.1
                        } else {
                            scale.1
                        },
                    );
                    let start = cur.unwrap_or(end);
                    let mut points = vec![start];
                    let mut knots = vec![(*cc).unwrap_or(0.0)];
                    let mut weights = vec![(*cd).unwrap_or(1.0)];
                    for (gx, gy, gk, gw) in &f.points {
                        points.push((gx * fx, gy * fy));
                        knots.push(*gk);
                        weights.push(*gw);
                    }
                    knots.push((*ca).unwrap_or(0.0));
                    knots.push(f.last_knot);
                    weights.push((*cb).unwrap_or(1.0));
                    points.push(end);
                    let built = crate::curve::SplineData {
                        degree: f.degree,
                        points,
                        weights,
                        knots,
                    }
                    .sanitized();
                    if let Some(data) = built {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                        cur = Some(end);
                        cur_local = Some(end_local);
                        drew = true;
                    }
                }
                if !drew {
                    pts.push(to_page(end));
                    cur = Some(end);
                    cur_local = Some(end_local);
                }
            }
            "PolylineTo" => {
                // The EFFECTIVE A: an instance row may omit it and
                // inherit the master's stored-hop controls — the same
                // chain fallback the reconcile reads. ONE read feeds
                // both the classifier and the vertex emission, so a
                // recognized hop straightens and an authored polyline
                // still draws its controls (CodeRabbit round on #324
                // phase 5).
                let a_text = row
                    .cell("A")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .or_else(|| {
                        (*master)
                            .and_then(|m| m.cell("A"))
                            .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    })
                    .unwrap_or_default();
                // A RECOGNIZED stored styled hop (#324 phase 4 —
                // percentage controls over a 2r slot, collinear runs
                // both sides) straightens like an arc row: the route
                // stays one straight run and the connector stays
                // eligible for the crossing set. Everything else is
                // authored multi-vertex geometry, not a routed
                // straight run (#324 step 1) — even when its vertices
                // happen to be straight.
                let mut stored = false;
                if strip_stored_jumps.is_some() {
                    // The classification runs in the frame the RENDER
                    // emits (instance dims, the ratio-scaled chord) —
                    // a resized master-only instance classifies
                    // against the same controls the ink draws
                    // (CodeRabbit round on #324 phase 5). The
                    // own-section path is scale-free: identical to the
                    // row-local frame.
                    if let (Some(_c0), Some(c0s)) = (cur_local, cur) {
                        let d_class = (end.0 - c0s.0, end.1 - c0s.1);
                        let mut prev_dir = None;
                        let mut pi = k as isize - 1;
                        while pi >= 1 {
                            let (_, _, _, pt, pe, _, _, _, _, _) = &evald[pi as usize];
                            if kind_of(pt.as_str()) != 1 {
                                break;
                            }
                            let (_, _, _, _, pp, _, _, _, _, _) = &evald[pi as usize - 1];
                            let pd = (pe.0 - pp.0, pe.1 - pp.1);
                            if pd.0.hypot(pd.1) > 1e-9 {
                                prev_dir = Some(pd);
                                break;
                            }
                            pi -= 1;
                        }
                        let mut next_dir = None;
                        let mut ni = k + 1;
                        while ni < evald.len() {
                            let (_, _, _, nt, ne, _, _, _, _, _) = &evald[ni];
                            if kind_of(nt.as_str()) != 1 {
                                break;
                            }
                            let p = if ni == k + 1 {
                                end_local
                            } else {
                                evald[ni - 1].4
                            };
                            let nd = (ne.0 - p.0, ne.1 - p.1);
                            if nd.0.hypot(nd.1) > 1e-9 {
                                next_dir = Some(nd);
                                break;
                            }
                            ni += 1;
                        }
                        let (rx, ry) = strip_stored_jumps.unwrap_or((0.0, 0.0));
                        stored = is_stored_jump_polyline(
                            &a_text, c0s, d_class, prev_dir, next_dir, frame.size, rx, ry,
                        );
                    }
                }
                if !stored {
                    straight = false;
                }
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                // The POLYLINE() interior vertices, with the renderer's
                // per-axis rule (the NURBSTo arm above): a fractional
                // axis (type 0) lands in the frame's own dims, an
                // absolute axis takes only the scale. Crossings then
                // see the bends the renderer draws, not the chord
                // (Greptile on PR #329). The SAME effective A the
                // classifier read — an inherited hop's controls
                // straighten in the hit walk exactly as they draw.
                if !stored {
                    if let Some((xt, yt, verts)) = crate::curve::parse_polyline(&a_text) {
                        let fx = if xt == 0.0 { frame.size.0 } else { scale.0 };
                        let fy = if yt == 0.0 { frame.size.1 } else { scale.1 };
                        for (vx, vy) in verts {
                            pts.push(to_page((vx * fx, vy * fy)));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            _ if t.ends_with("To") || t.contains("Spline") || t == "InfiniteLine" => {
                // LineTo plus the v0.2 infinite-line chord. Only the
                // KNOWN straight rows keep the eligibility: an unknown
                // `*To` row or an infinite line draws its endpoint
                // chord — the ink is straight, but the row type is
                // not a routed straight run (#324 step 1).
                if t != "LineTo" && t != "RelLineTo" {
                    straight = false;
                }
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            _ => {
                // Any row type the walk doesn't recognize as a
                // positioned or straight run — `Ellipse` and unknown
                // others — is curve geometry or draws nothing; either
                // way the connector is not an all-straight route
                // (#324 phase 2, CodeRabbit round on phase 3).
                straight = false;
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
            }
        }
    }
    if let Some(acc) = spline_acc.take() {
        if let Some(data) = acc.flush() {
            for p in crate::curve::SplineData::flatten(&data, 16)
                .into_iter()
                .skip(1)
            {
                pts.push(to_page(p));
            }
        }
    }
    (pts.windows(2).map(|w| (w[0], w[1])).collect(), straight)
}
