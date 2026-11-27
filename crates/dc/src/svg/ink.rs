//! Ink bounds: the geometry walks behind selection boxes, the canvas
//! damage region, and layer viewBoxes.
use super::*;
use std::collections::HashMap;
use std::fmt::Write as _;
/// Union sink for the ink-bounds walk with the #219 focus gate:
/// nodes contribute only once the walk is inside the focus subtree
/// (always, when unfocused). `enter` arms the gate at the focus node;
/// the walk saves/restores `inside` around each child so a later
/// sibling of the focus never leaks into the region.
struct Focus {
    acc: Option<(f64, f64, f64, f64)>,
    focus: Option<i64>,
    inside: bool,
}

impl Focus {
    fn enter(&mut self, id: i64) {
        if Some(id) == self.focus {
            self.inside = true;
        }
    }

    fn union(&mut self, r: (f64, f64, f64, f64)) {
        if !self.inside {
            return;
        }
        self.acc = Some(match self.acc {
            None => r,
            Some((ax, ay, aw, ah)) => {
                let x1 = (ax + aw).max(r.0 + r.2);
                let y1 = (ay + ah).max(r.1 + r.3);
                let nx = ax.min(r.0);
                let ny = ay.min(r.1);
                (nx, ny, x1 - nx, y1 - ny)
            }
        });
    }
}

/// The ink bounds of one top-level shape's subtree, in page
/// coordinates (inches, y-down like the SVG viewBox): the union of
/// every node's transformed [0..w]×[-h..0] box and text box, padded to
/// absorb stroke bleed, clamped to the page ± 2 in. This is the
/// viewBox (and the on-canvas quad) of a per-shape layer (#93).
pub fn shape_ink_bounds(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
) -> Option<(f64, f64, f64, f64)> {
    shape_ink_bounds_focused(doc, page, shape_id, None)
}

/// [`shape_ink_bounds`] restricted to one DESCENDANT's own subtree
/// (#219): `focus` names a sub-shape inside `shape_id`'s subtree and
/// only that sub-shape's subtree is unioned — the damage region a
/// localized edit re-rasters. The cell-resolution chain still runs
/// from the top-level root, so inherited/master cells resolve exactly
/// as the renderer resolves them. `None` is the whole-subtree bounds.
pub fn shape_ink_bounds_focused(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
    focus: Option<i64>,
) -> Option<(f64, f64, f64, f64)> {
    let page_h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);
    let root = page.shapes.iter().find(|s| s.id == shape_id)?;
    let mut u = Focus {
        acc: None,
        focus,
        inside: focus.is_none(),
    };
    fn walk(
        doc: &Document,
        s: &Shape,
        chain: &[&Shape],
        page_h: f64,
        top: bool,
        to_page: Affine,
        u: &mut Focus,
    ) {
        let mut chain: Vec<&Shape> = chain.to_vec();
        chain.push(s);
        let (node_box, a) = node_ink_box(doc, s, &chain, page_h, top, to_page);
        u.union(node_box);
        for sub in &s.sub_shapes {
            let saved = u.inside;
            u.enter(sub.id);
            walk(doc, sub, &chain, page_h, false, a, u);
            u.inside = saved;
        }
    }
    u.enter(root.id);
    walk(doc, root, &[], page_h, true, Affine::IDENTITY, &mut u);
    let (x, y, w, h) = u.acc?;
    // Stroke bleed margin; then clamp to the page ± 2 in so a rogue
    // formula cannot push a layer off to infinity.
    let pad = 0.08;
    let (x, y, w, h) = (x - pad, y - pad, w + 2.0 * pad, h + 2.0 * pad);
    let pw = num(page.sheet().and_then(|s| s.cell("PageWidth"))).unwrap_or(8.5);
    let (min_x, min_y) = (-2.0, -2.0);
    let (max_x, max_y) = (pw + 2.0, page_h + 2.0);
    let x = x.max(min_x);
    let y = y.max(min_y);
    let w = w.min(max_x - x);
    let h = h.min(max_y - y);
    Some((x, y, w.max(0.01), h.max(0.01)))
}

/// One node's own ink box in page space (inches, y-down), with the
/// affine that maps its local frame there: the per-node half of the
/// ink-bounds walk ([`shape_ink_bounds_focused`]) and of the #236
/// cull's subtree-box pass. `chain` includes `s`. The box unions the
/// transformed [0..w]×[-h..0] XForm box, every geometry row's real
/// extent (#202 — curve hulls and arc apexes), and the text block's
/// placement box; no padding, no clamping (the callers decide).
/// Mirrored master content maps into the instance box, so this box
/// bounds it too.
pub(crate) fn node_ink_box(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    page_h: f64,
    top: bool,
    to_page: Affine,
) -> ((f64, f64, f64, f64), Affine) {
    let env = shape_env(doc, s, chain);
    // Takes the env per call (not by capture) so `env` can move into
    // the shared frame env below (#218 review).
    let get = |n: &str, env: &vsdx::formula::Env| -> Option<f64> {
        doc.resolve_cell(s, chain, n)
            .and_then(|c| cell_num(&c, env))
    };
    let width = get("Width", &env).unwrap_or(1.0);
    let height = get("Height", &env).unwrap_or(1.0);
    // The exact transform string render_shape/render_shape_local
    // emit for this node, composed into one affine.
    let local = if top {
        let (sx, sy) = (
            if get("FlipX", &env).unwrap_or(0.0) != 0.0 {
                -1.0
            } else {
                1.0
            },
            if get("FlipY", &env).unwrap_or(0.0) != 0.0 {
                -1.0
            } else {
                1.0
            },
        );
        Affine::translate(
            get("PinX", &env).unwrap_or(0.0),
            page_h - get("PinY", &env).unwrap_or(0.0),
        )
        .then(&Affine::rotate_deg(
            -get("Angle", &env).unwrap_or(0.0).to_degrees(),
        ))
        .then(&Affine::scale(sx, sy))
        .then(&Affine::translate(
            -get("LocPinX", &env).unwrap_or(width / 2.0),
            get("LocPinY", &env).unwrap_or(height / 2.0),
        ))
    } else {
        let flip = |n: &str| {
            if get(n, &env).unwrap_or(0.0) != 0.0 {
                -1.0
            } else {
                1.0
            }
        };
        Affine::translate(
            get("PinX", &env).unwrap_or(0.0),
            -get("PinY", &env).unwrap_or(0.0),
        )
        .then(&Affine::rotate_deg(
            -get("Angle", &env).unwrap_or(0.0).to_degrees(),
        ))
        .then(&Affine::scale(flip("FlipX"), flip("FlipY")))
        .then(&Affine::translate(
            -get("LocPinX", &env).unwrap_or(width / 2.0),
            get("LocPinY", &env).unwrap_or(height / 2.0),
        ))
    };
    let a = to_page.then(&local);
    let mut u = Focus {
        acc: None,
        focus: None,
        inside: true,
    };
    // The node's own ink box (local y-down) — mirrored master
    // content maps into the instance box, so this bounds it too.
    let box_union = |x0: f64, y0: f64, x1: f64, y1: f64, a: &Affine, u: &mut Focus| {
        let c = [
            a.apply(x0, y0),
            a.apply(x1, y0),
            a.apply(x1, y1),
            a.apply(x0, y1),
        ];
        let xs = [c[0].0, c[1].0, c[2].0, c[3].0];
        let ys = [c[0].1, c[1].1, c[2].1, c[3].1];
        u.union((
            xs.iter().cloned().fold(f64::INFINITY, f64::min),
            ys.iter().cloned().fold(f64::INFINITY, f64::min),
            xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
                - xs.iter().cloned().fold(f64::INFINITY, f64::min),
            ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
                - ys.iter().cloned().fold(f64::INFINITY, f64::min),
        ));
    };
    box_union(0.0, -height, width, 0.0, &a, &mut u);
    // #202: the geometry rows' REAL extent — a curved connector's
    // arc belly leaves the begin-corner box (a 1D connector's
    // Height is 0), and a layer viewBox built from the box alone
    // clips the belly into a straight line (every arc-pair
    // connector in utilisation.vsdx rendered flat). Union every
    // row's endpoint and control points: every path this module
    // emits is a line/quad/cubic whose curve stays inside the
    // hull of those points, plus the elliptical arc's apex
    // (2·on-curve − chord mid) as the conservative bound.
    // What the renderer draws: the instance's own Geometry when it
    // carries any, else the MASTER source's rows scaled into the
    // instance frame (render_all_geometry's master path) — a fresh
    // stencil-drop instance renders entirely through it, and its
    // arrowheads and arc bellies must bound the ink too or the
    // layer viewBox clips them and the #236 cull skips them
    // (#218 review round 3).
    let own = s.sheet.sections_named("Geometry");
    // ONE env per frame, shared across every walked section (#218
    // review): the per-entry clones discarded row-coordinate
    // writebacks, so a Geometry2 referencing `Geometry1.X2` fell
    // back to the authored pose (#249's cross-row case, across
    // sections). A shape walks either its OWN sections (instance
    // env) or its master's (the prepared menv) — never both. Its own
    // sections walk MERGED over the master's rows, exactly as the
    // renderer paints them (#378, #302): the raw inline rows of a
    // partial override (Y cells only, X absent) bounded a looped
    // connector to a sliver and the canvas layer clipped the loop.
    let merged_own = if own.is_empty() {
        Vec::new()
    } else {
        super::shape_layer::merged_own_geometry(doc, s, chain, width, height, &env)
    };
    let mut walk: Vec<(&vsdx::Section, (f64, f64))> =
        merged_own.iter().map(|g| (g, (1.0, 1.0))).collect();
    // Starts as the instance env; the master branch below REPLACES it.
    // The text block after the walk still reads the INSTANCE env's
    // Txt* cells (they are the shape's own), so a copy survives the move.
    let text_env = env.clone();
    let mut frame_env = env;
    if own.is_empty() {
        if let Some(src) = doc.instance_source(s, chain) {
            let mw = src
                .sheet
                .cell("Width")
                .and_then(|c| c.numeric())
                .filter(|v| v.abs() > 1e-9);
            let mh = src
                .sheet
                .cell("Height")
                .and_then(|c| c.numeric())
                .filter(|v| v.abs() > 1e-9);
            let scale = (
                mw.map_or(1.0, |mw| width / mw),
                mh.map_or(1.0, |mh| height / mh),
            );
            // The master env builds ONCE with the instance's GLUED
            // Begin/End and the Scratch/Controls formulas
            // RE-EVALUATED against them — the cached V is the
            // authored pose, and the band masters derive their whole
            // vertical span from them (`Scratch.Y2 =
            // ABS(BeginY-EndY)`, #249).
            let mut menv = shape_env(doc, src, &[]);
            for n in ["BeginX", "BeginY", "EndX", "EndY"] {
                if let Some(v) = doc.resolve_cell(s, chain, n).and_then(|c| c.numeric()) {
                    menv.insert(n.to_string(), v);
                }
            }
            for sec_name in ["Scratch", "Controls"] {
                for sec in src.sheet.sections_named(sec_name) {
                    for row in &sec.rows {
                        for (name, cell) in &row.cells {
                            let key = match sec_name {
                                "Scratch" => format!("Scratch.{}{}", name, row.ix + 1),
                                _ => {
                                    let rn =
                                        row.n.clone().unwrap_or_else(|| format!("Row_{}", row.ix));
                                    format!("Controls.{}.{}", rn, name)
                                }
                            };
                            let v = cell
                                .f
                                .as_deref()
                                .filter(|f| *f != "Inh" && *f != "No Formula")
                                .and_then(|f| vsdx::formula::eval(f, &menv).ok())
                                .and_then(|v| v.as_f64())
                                .filter(|v| v.is_finite())
                                .or_else(|| cell.numeric());
                            if let Some(v) = v {
                                menv.insert(key, v);
                            }
                        }
                    }
                }
            }
            walk.extend(
                src.sheet
                    .sections_named("Geometry")
                    .into_iter()
                    .map(|g| (g, scale)),
            );
            frame_env = menv;
        }
    }
    // An instance PolylineTo row without its own POLYLINE() text draws
    // its master row's vertices (render_all_geometry: section by index,
    // row by IX, else by its occurrence among the PolylineTo rows), so
    // the bounds walk them too (Greptile on PR #329). Absolute axes
    // scale from master into instance units; fractional ones stay.
    let walks_own = !own.is_empty();
    let master_polyline = |walk_ix: usize, ix: i64, occ: usize| -> Option<String> {
        if !walks_own {
            return None;
        }
        let src = doc.instance_source(s, chain)?;
        let sec = *src.sheet.sections_named("Geometry").get(walk_ix)?;
        let is_poly = |r: &&vsdx::Row| r.t.as_deref() == Some("PolylineTo");
        let mrow = match sec.rows.iter().find(|r| ix > 0 && r.ix == ix) {
            Some(r) => Some(r).filter(is_poly)?,
            None => sec.rows.iter().filter(is_poly).nth(occ)?,
        };
        let text = mrow
            .cell("A")
            .and_then(|c| c.v.clone().or_else(|| c.f.clone()))?;
        let (xt, yt, pts) = crate::curve::parse_polyline(&text)?;
        let ratio = |cell: &str, dim: f64| {
            src.sheet
                .cell(cell)
                .and_then(|c| c.numeric())
                .filter(|v| v.abs() > 1e-9)
                .map_or(1.0, |m| dim / m)
        };
        let (sx, sy) = (ratio("Width", width), ratio("Height", height));
        let mut out = format!("POLYLINE({xt}, {yt}");
        for (px, py) in pts {
            let px = if xt == 0.0 { px } else { px * sx };
            let py = if yt == 0.0 { py } else { py * sy };
            let _ = write!(out, ", {px},{py}");
        }
        out.push(')');
        Some(out)
    };
    for (walk_ix, (geo, gscale)) in walk.into_iter().enumerate() {
        // Shared: writebacks persist for later sections of the frame.
        let genv = &mut frame_env;
        // A hidden section never paints, so it bounds nothing — an
        // inherited NoShow=1 kept its far-off endpoints in the layer
        // (CodeRabbit on PR #381).
        if super::shape_layer::section_flag(geo, "NoShow", genv) {
            continue;
        }
        let gname = format!("Geometry{}", walk_ix + 1);
        let mut cur = (0.0f64, 0.0f64);
        let mut first_pt: Option<(f64, f64)> = None;
        let mut poly_occ = 0usize;
        fn row_num_in(env: &vsdx::formula::Env, row: &vsdx::Row, n: &str) -> f64 {
            row.cell(n)
                .and_then(|c| {
                    c.f.as_deref()
                        .and_then(|f| vsdx::formula::eval(f, env).ok())
                        .and_then(|v| v.as_f64())
                        .filter(|v| v.is_finite())
                        .or(c.numeric())
                })
                .unwrap_or(0.0)
        }
        for row in &geo.rows {
            let Some(t) = row.t.as_deref() else { continue };
            if row.is_deleted() {
                continue; // deleted rows stay deleted (#293)
            }
            let rel = t.starts_with("Rel");
            let (mut x, mut y) = (row_num_in(genv, row, "X"), row_num_in(genv, row, "Y"));
            if rel {
                // Raw signed dims — rel_adjust (which the renderer
                // uses) multiplies raw, so the bound must match.
                x *= width;
                y *= height;
            }
            // Feed the section-local coordinates back for cross-row
            // references (`Geometry1.X2`, #249) — before the scale,
            // which maps onto the page but not into the next row's
            // frame.
            genv.insert(format!("{}.X{}", gname, row.ix), x);
            genv.insert(format!("{}.Y{}", gname, row.ix), y);
            // Master-frame rows scale into the instance like the
            // renderer's emit (every point, literals and evaluated
            // formulas alike).
            x *= gscale.0;
            y *= gscale.1;
            // The affine maps local y-DOWN ([0..w]×[-h..0]) like the
            // box union above; the row coordinates are y-UP.
            let pt = |x: f64, y: f64, u: &mut Focus| {
                let (px, py) = a.apply(x, -y);
                u.union((px, py, 0.0, 0.0));
            };
            pt(x, y, &mut u);
            match t {
                "ArcTo" | "RelArcTo" => {
                    // The apex: chord mid + A·right-of-travel.
                    let sag = row_num_in(genv, row, "A");
                    let (dx, dy) = (x - cur.0, y - cur.1);
                    let len = dx.hypot(dy).max(1e-9);
                    let mid = ((cur.0 + x) / 2.0, (cur.1 + y) / 2.0);
                    pt(mid.0 + dy / len * sag, mid.1 - dx / len * sag, &mut u);
                }
                "EllipticalArcTo" | "RelEllipticalArcTo" => {
                    let (mut mx, mut my) = (row_num_in(genv, row, "A"), row_num_in(genv, row, "B"));
                    if rel {
                        // Raw signed dims — rel_adjust multiplies
                        // raw (negative widths mirror the bound).
                        mx *= width;
                        my *= height;
                    }
                    mx *= gscale.0;
                    my *= gscale.1;
                    pt(mx, my, &mut u);
                    // The arc's apex beyond the on-curve point.
                    pt(
                        2.0 * mx - (cur.0 + x) / 2.0,
                        2.0 * my - (cur.1 + y) / 2.0,
                        &mut u,
                    );
                }
                "CubicBezTo" | "RelCubBezTo" => {
                    // (A,B) and (C,D): two control points.
                    let (mut a, mut b) = (row_num_in(genv, row, "A"), row_num_in(genv, row, "B"));
                    let (mut c, mut d) = (row_num_in(genv, row, "C"), row_num_in(genv, row, "D"));
                    if rel {
                        a *= width.max(1e-6);
                        b *= height.max(1e-6);
                        c *= width.max(1e-6);
                        d *= height.max(1e-6);
                    }
                    pt(a * gscale.0, b * gscale.1, &mut u);
                    pt(c * gscale.0, d * gscale.1, &mut u);
                }
                "QuadBezTo" | "RelQuadBezTo" => {
                    let (mut a, mut b) = (row_num_in(genv, row, "A"), row_num_in(genv, row, "B"));
                    if rel {
                        a *= width.max(1e-6);
                        b *= height.max(1e-6);
                    }
                    pt(a * gscale.0, b * gscale.1, &mut u);
                }
                "NURBSTo" => {
                    // #188: the interior control points live in the
                    // E formula's (x, y, knot, weight) groups.
                    let e_text = row
                        .cell("E")
                        .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                        .unwrap_or_default();
                    if let Some(f) = crate::curve::parse_nurbs(&e_text) {
                        // Per-axis: a fraction lands directly in
                        // instance units (the frame's own Width/
                        // Height); an absolute axis carries
                        // master-unit literals and takes the scale
                        // once — mirroring the render arm (#218
                        // review round: no squared stretch).
                        let fx = if f.x_type == 0.0 { width } else { gscale.0 };
                        let fy = if f.y_type == 0.0 { height } else { gscale.1 };
                        for (gx, gy, _, _) in &f.points {
                            pt(gx * fx, gy * fy, &mut u);
                        }
                    }
                }
                "PolylineTo" => {
                    // The POLYLINE() interior vertices (same per-axis
                    // rule as NURBSTo), the master's when the row has
                    // none of its own.
                    let a_text = row
                        .cell("A")
                        .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                        .unwrap_or_default();
                    let occ = poly_occ;
                    poly_occ += 1;
                    let a_text = match crate::curve::parse_polyline(&a_text) {
                        Some(_) => a_text,
                        None => master_polyline(walk_ix, row.ix, occ).unwrap_or_default(),
                    };
                    if let Some((xt, yt, pts)) = crate::curve::parse_polyline(&a_text) {
                        let fx = if xt == 0.0 { width } else { gscale.0 };
                        let fy = if yt == 0.0 { height } else { gscale.1 };
                        for (px, py) in pts {
                            pt(px * fx, py * fy, &mut u);
                        }
                    }
                }
                _ => {}
            }
            cur = (x, y);
            if first_pt.is_none() {
                first_pt = Some((x, y));
            }
        }
        // #185: arrowheads draw at physical size beyond the row-point
        // hull — a size-5/6 head on a horizontal 1D connector reaches
        // ±0.125/±0.25 in sideways against the 0.08 in bleed pad, so
        // union a conservative square (the gallery's largest reach) at
        // each end that carries a head. Same gate as the renderer's.
        let arrowed = |arrow: f64| arrow >= 0.5 && !(46.0..=50.5).contains(&arrow);
        if get("BeginArrow", &text_env).is_some_and(arrowed)
            || get("EndArrow", &text_env).is_some_and(arrowed)
        {
            let weight = get("LineWeight", &text_env).unwrap_or(0.75 / 72.0);
            let pad = |pt: (f64, f64), arrow: Option<f64>, sz: f64, u: &mut Focus| {
                if !arrow.is_some_and(arrowed) {
                    return;
                }
                let r = arrow_head_reach(weight, sz);
                // LOCAL y-down corners — box_union applies the affine
                // itself; pre-applying it here doubled the transform
                // and threw the pad square across the page (#218
                // review).
                let (lx, ly) = (pt.0, -pt.1);
                box_union(lx - r, ly - r, lx + r, ly + r, &a, u);
            };
            if let Some((x, y)) = first_pt {
                pad(
                    (x, y),
                    get("BeginArrow", &text_env),
                    get("BeginArrowSize", &text_env).unwrap_or(2.0),
                    &mut u,
                );
            }

            pad(
                cur,
                get("EndArrow", &text_env),
                get("EndArrowSize", &text_env).unwrap_or(2.0),
                &mut u,
            );
        }
    }
    // Text can overflow the shape box (TxtWidth/TxtHeight are
    // independent cells); bound it with emit_text's placement math.
    // The decision uses the RENDERED text: a self-closing `<fld/>`
    // marker substitutes a nonempty value the plain `text` cell does
    // not carry — omitting the frame clipped the substituted label
    // (CodeRabbit PR review).
    // An instance without a `<Text>` shows its master's (#378).
    let tsheet = crate::fields::text_sheet(doc, s, chain);
    let has_text = tsheet
        .text
        .as_deref()
        .map(str::trim)
        .is_some_and(|t| !t.is_empty())
        || tsheet
            .text_raw
            .as_deref()
            .is_some_and(|r| r.contains("<fld"));
    // Hidden text (#397) paints nothing, so it bounds nothing.
    if has_text && !super::text::text_hidden(doc, s, chain) {
        let tw = get("TxtWidth", &text_env).unwrap_or(width).max(0.02);
        let th = get("TxtHeight", &text_env).unwrap_or(height).max(0.02);
        let (tlx, tly) = (
            get("TxtLocPinX", &text_env).unwrap_or(tw / 2.0),
            get("TxtLocPinY", &text_env).unwrap_or(th / 2.0),
        );
        let (tx, ty) = (
            get("TxtPinX", &text_env).unwrap_or(width / 2.0),
            get("TxtPinY", &text_env).unwrap_or(height / 2.0),
        );
        // #231 outdented bullets hang LEFT of the text frame: a
        // negative IndFirst draws the glyph at the text area's left +
        // IndFirst, which can precede the frame (and the shape box) —
        // the frame-only bounds let that ink escape the ink box, so
        // the layer viewBox clips it and the #236 cull skips the
        // child (local CodeRabbit round 3 + final pass). Both branches
        // bound the EXTENDED frame (left edge out by the escape): the
        // unrotated branch through the shape transform directly; the
        // rotated branch through emit_text's own wrapper first — the
        // escape rotates with the block.
        let (x0, y0) = (tx - tlx, -(ty - tly + th));
        let tb = resolve_text_block(doc, s, chain, width, height);
        // Text taller than its frame overflows on the side opposite
        // the alignment: down for top, up for bottom, both ways for
        // middle. Bound the laid-out height (TEXTHEIGHT, the same
        // slots emit_text paints), not just the frame (CodeRabbit on
        // PR #325).
        let (ty0, ty1) = {
            let text = crate::fields::substituted_text(doc, s, chain, None)
                .or_else(|| tsheet.text.clone())
                .unwrap_or_default();
            let m = crate::textfunc::SheetTextMeasure::new(doc, s, chain, text.clone());
            let h = {
                use vsdx::formula::TextMeasure as _;
                m.text_height(&text, tw)
            }
            .unwrap_or(th)
            .max(th);
            match tb.valign {
                1 => (y0, y0 + h),
                3 => (y0 + th - h, y0 + th),
                _ => (y0 + (th - h) / 2.0, y0 + (th + h) / 2.0),
            }
        };
        let bullet_escape = tb.bullet.as_ref().map_or(0.0, |b| {
            let escape = tb.insets.0 + tb.ind_first;
            if escape < 0.0 {
                -escape + bullet_advance_in(&tb, b)
            } else {
                0.0
            }
        });
        if get("TxtAngle", &text_env).unwrap_or(0.0) != 0.0 {
            // Rotated text: push the extended frame's corners through
            // emit_text's own wrapper — the pin rotation (flips
            // composed, net = −sx·sy·TxtAngle) in the shape-local
            // y-down frame — then the shape transform. The old
            // centered circumscribed square under-covered BOTH the
            // one-sided bullet escape (half of it — Greptile round 8)
            // and oblique rotations of the frame itself.
            let angle = get("TxtAngle", &text_env).unwrap_or(0.0);
            let (fsx, fsy) = (
                if get("FlipX", &text_env).unwrap_or(0.0) != 0.0 {
                    -1.0
                } else {
                    1.0
                },
                if get("FlipY", &text_env).unwrap_or(0.0) != 0.0 {
                    -1.0
                } else {
                    1.0
                },
            );
            let net_deg = -(fsx * fsy) * angle.to_degrees();
            let text_tf = Affine::translate(tx, -ty)
                .then(&Affine::scale(fsx, fsy))
                .then(&Affine::rotate_deg(net_deg))
                .then(&Affine::translate(-tx, ty));
            // `then` applies its argument FIRST (self · next, the SVG
            // transform-list reading) — so the text rotation runs on
            // the shape-LOCAL corner and `a` carries the result to
            // the page. The round-8 form composed them backwards and
            // rotated page coordinates around a shape-local pivot,
            // landing the hull far from the shape (Greptile round 9).
            let a_text = a.then(&text_tf);
            box_union(x0 - bullet_escape, ty0, x0 + tw, ty1, &a_text, &mut u);
        } else {
            box_union(x0 - bullet_escape, ty0, x0 + tw, ty1, &a, &mut u);
        }
    }
    (u.acc.unwrap_or((0.0, 0.0, 0.0, 0.0)), a)
}

/// The page-space ink box of every shape in `shape_id`'s subtree —
/// each entry unions the node's own box with all its descendants'
/// ([`node_ink_box`], one shared walk). This is the #236 patch cull's
/// predicate source: a node whose SUBTREE box misses the padded
/// damage rect paints nothing inside it and is omitted from the patch
/// serialization. The top-level root's own entry covers its whole
/// subtree.
pub fn subtree_ink_boxes(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
) -> Option<HashMap<i64, (f64, f64, f64, f64)>> {
    let page_h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);
    let root = page.shapes.iter().find(|s| s.id == shape_id)?;
    let mut map: HashMap<i64, (f64, f64, f64, f64)> = HashMap::new();
    fn walk(
        doc: &Document,
        s: &Shape,
        chain: &[&Shape],
        page_h: f64,
        top: bool,
        to_page: Affine,
        map: &mut HashMap<i64, (f64, f64, f64, f64)>,
    ) -> (f64, f64, f64, f64) {
        let mut chain: Vec<&Shape> = chain.to_vec();
        chain.push(s);
        let (own, a) = node_ink_box(doc, s, &chain, page_h, top, to_page);
        let mut acc = own;
        for sub in &s.sub_shapes {
            let sub_box = walk(doc, sub, &chain, page_h, false, a, map);
            acc = union_box(acc, sub_box);
        }
        map.insert(s.id, acc);
        acc
    }
    walk(doc, root, &[], page_h, true, Affine::IDENTITY, &mut map);
    Some(map)
}

/// Union two (x, y, w, h) page rects — the bounds-walk's accumulator
/// as a plain function (the #236 subtree-box pass unions bottom-up).
fn union_box(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let x1 = (a.0 + a.2).max(b.0 + b.2);
    let y1 = (a.1 + a.3).max(b.1 + b.3);
    let nx = a.0.min(b.0);
    let ny = a.1.min(b.1);
    (nx, ny, x1 - nx, y1 - ny)
}
