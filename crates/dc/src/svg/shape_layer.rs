//! Shape-layer serialization and the per-shape render walk: one
//! top-level shape as a standalone layer SVG (#93), plus everything
//! the walk emits — master mirroring, foreign media, geometry rows,
//! arrowheads, and the paint/gradient/hatch/dash helpers.
use super::*;
use std::collections::HashMap;
use std::fmt::Write as _;
/// One top-level shape's subtree as a standalone layer SVG (#93): the
/// same absolute page coordinates and the same `render_shape` walk as
/// [`render_page_with`] — pixel-identical ink — but the viewBox is the
/// subtree's [`shape_ink_bounds`] and no page chrome/background is
/// emitted (the canvas composites its own paper under the layer
/// quads). The canvas, not the export path, is the consumer.
pub fn render_shape_layer(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
    hidden: &[i64],
    bounds: (f64, f64, f64, f64),
) -> anyhow::Result<String> {
    render_shape_layer_opts(doc, page, shape_id, hidden, bounds, None, None, None)
        .map(|(_, svg)| svg)
}

/// [`render_shape_layer`] with the canvas-only options (GitHub #22,
/// #124): `skip_text_shape` suppresses one shape's text while the
/// GUI's in-place editor is open on it, and `greek_ppi` turns on text
/// greeking for layers rasterized at `ppi` — text blocks whose em
/// height would fall under [`TEXT_GREEK_PX`] device pixels emit gray
/// bars instead of glyphs. Returns the SVG plus every text size the
/// walk met (inches), the cache's greek profile for deciding whether
/// a ppi-bucket change re-renders or re-serializes.
///
/// `fields` shares one page walk's [`crate::fields::FieldsCtx`]
/// across roots (its `Sheet.N!` layer caches per CONTEXT — a
/// per-shape context rebuilds it every call, the quadratic cost the
/// sharing exists to remove); `None` builds a fresh one for one-shot
/// renders.
#[allow(clippy::too_many_arguments)] // the layer's full identity + the shared field walk
pub fn render_shape_layer_opts(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
    hidden: &[i64],
    bounds: (f64, f64, f64, f64),
    skip_text: Option<i64>,
    greek_ppi: Option<f32>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
) -> anyhow::Result<(Vec<f64>, String)> {
    render_shape_layer_inner(
        doc, page, shape_id, hidden, bounds, skip_text, greek_ppi, None, fields,
    )
}

/// [`render_shape_layer_opts`] for the #236 damage-region patch: only
/// the subtree nodes whose ink box intersects `cull_rect` (page
/// inches) are serialized. Omitted nodes lie entirely outside the
/// patch rect (the caller pads it past stroke bleed and anti-aliasing
/// reach), so pixels inside the patch are unchanged — the
/// `layers::culled_patch_pixels_match_the_full_layer_render` pin
/// holds the patch-vs-full byte comparison for the culled tree.
/// `bounds` stays the FULL layer's viewBox: the patch renders through
/// the same grid as the existing texture.
#[allow(clippy::too_many_arguments)] // the layer's full identity + the cull rect (#236)
pub fn render_shape_layer_opts_culled(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
    hidden: &[i64],
    bounds: (f64, f64, f64, f64),
    skip_text: Option<i64>,
    greek_ppi: Option<f32>,
    cull_rect: (f64, f64, f64, f64),
    fields: Option<&crate::fields::FieldsCtx<'_>>,
) -> anyhow::Result<(Vec<f64>, String)> {
    let boxes = subtree_ink_boxes(doc, page, shape_id)
        .ok_or_else(|| anyhow::anyhow!("shape {shape_id} not found on page {}", page.id))?;
    let cull = InkCull {
        rect: cull_rect,
        boxes: &boxes,
    };
    render_shape_layer_inner(
        doc,
        page,
        shape_id,
        hidden,
        bounds,
        skip_text,
        greek_ppi,
        Some(&cull),
        fields,
    )
}

/// The #236 cull predicate: a shape id → its subtree ink box, plus the
/// padded damage rect. `skips` is the render walk's per-node test — a
/// node whose SUBTREE box misses the rect paints nothing inside it
/// (the full render's own viewBox clips anything beyond the box), so
/// omitting it changes no pixel the patch will show.
pub(crate) struct InkCull<'a> {
    rect: (f64, f64, f64, f64),
    boxes: &'a HashMap<i64, (f64, f64, f64, f64)>,
}

impl InkCull<'_> {
    fn skips(&self, id: i64) -> bool {
        // A node missing from the map cannot happen (the pass walks
        // the same subtree); keep it if it does — conservative.
        self.boxes
            .get(&id)
            .is_some_and(|b| !boxes_intersect(*b, self.rect))
    }
}

/// Two (x, y, w, h) page rects intersect (closed test — a shared edge
/// counts, so the caller's padding is what creates distance).
pub(crate) fn boxes_intersect(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 <= b.0 + b.2 && b.0 <= a.0 + a.2 && a.1 <= b.1 + b.3 && b.1 <= a.1 + a.3
}

#[allow(clippy::too_many_arguments)] // shared body of the plain and culled serializers + the field walk
fn render_shape_layer_inner(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
    hidden: &[i64],
    bounds: (f64, f64, f64, f64),
    skip_text: Option<i64>,
    greek_ppi: Option<f32>,
    cull: Option<&InkCull>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
) -> anyhow::Result<(Vec<f64>, String)> {
    // A caller-supplied context shares the page walk's `Sheet.N!`
    // layer across roots; a fresh one serves one-shot renders.
    let owned_fields;
    let fields = match fields {
        Some(f) => f,
        None => {
            owned_fields = crate::fields::FieldsCtx::for_page(doc, page);
            &owned_fields
        }
    };
    let page_h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);
    let Some(shape) = page.shapes.iter().find(|s| s.id == shape_id) else {
        anyhow::bail!("shape {shape_id} not found on page {}", page.id);
    };
    let hidden_member = |s: &Shape| -> bool {
        s.sheet
            .cell("LayerMember")
            .and_then(|c| c.numeric())
            .is_some_and(|m| hidden.contains(&(m as i64)))
    };
    let greek = greek_ppi.map(GreekCtx::new);
    let mut svg = String::new();
    let (bx, by, bw, bh) = bounds;
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{bx:.5} {by:.5} {bw:.5} {bh:.5}" width="{pw:.0}" height="{ph:.0}">"#,
        pw = bw * PX_PER_IN,
        ph = bh * PX_PER_IN,
    );
    if !hidden_member(shape) {
        render_shape(
            doc,
            shape,
            page_h,
            &[],
            &page.part,
            skip_text,
            greek.as_ref(),
            cull,
            Some(fields),
            // The canvas jump overlay draws exactly this walk's
            // top-level connectors (#294 review). The page's own
            // radii (X and Y — a stored jump on a vertical run was
            // authored at the Y spacing) are the stored-jump evidence.
            crate::geom::jump_radii_in(doc, page),
            &mut svg,
        );
    }
    let _ = writeln!(svg, "</svg>");
    let sizes = greek.map(|g| g.sizes.into_inner()).unwrap_or_default();
    Ok((sizes, svg))
}

/// Render a shape in absolute page coordinates: the XForm transform
/// wraps geometry, master mirroring, sub-shape recursion, and the
/// text block.
#[allow(clippy::too_many_arguments)] // the walk carries its whole environment
/// The page-level transform context `render_shape`'s head derives: the
/// emitted `<g transform>` string plus the width/height and flip signs
/// `emit_text` needs. Shared with `render_page_full`, which re-opens
/// an identical group for a jumping connector's TEXT outside its jump
/// clips (#324 phase 3: the clip cuts the shaft, never the label).
pub(crate) struct ShapeXform {
    pub transform: String,
    pub width: f64,
    pub height: f64,
    pub sx: f64,
    pub sy: f64,
}

/// The same reads `render_shape`'s head makes, factored out for
/// [`crate::svg::page`]'s label-twin group.
pub(crate) fn shape_xform(doc: &Document, s: &Shape, chain: &[&Shape], page_h: f64) -> ShapeXform {
    let mut chain: Vec<&Shape> = chain.to_vec();
    chain.push(s);
    let env = shape_env(doc, s, &chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    let (sx, sy) = (
        if get("FlipX").unwrap_or(0.0) != 0.0 {
            -1.0
        } else {
            1.0
        },
        if get("FlipY").unwrap_or(0.0) != 0.0 {
            -1.0
        } else {
            1.0
        },
    );
    let transform = format!(
        "translate({px:.6},{py:.6}) rotate({deg:.6}) scale({sx},{sy}) translate({lx:.6},{ly:.6})",
        px = get("PinX").unwrap_or(0.0),
        py = page_h - get("PinY").unwrap_or(0.0),
        deg = -get("Angle").unwrap_or(0.0).to_degrees(),
        lx = -get("LocPinX").unwrap_or(width / 2.0),
        ly = get("LocPinY").unwrap_or(height / 2.0),
    );
    ShapeXform {
        transform,
        width,
        height,
        sx,
        sy,
    }
}

#[allow(clippy::too_many_arguments)] // the walk carries its whole environment
pub(crate) fn render_shape<'a>(
    doc: &'a Document,
    s: &'a Shape,
    page_h: f64,
    chain: &[&'a Shape],
    part: &str,
    skip_text: Option<i64>,
    greek: Option<&GreekCtx>,
    cull: Option<&InkCull>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
    // The page's own jump radius (`geom::jump_radius_in`) in page
    // inches — the stored-jump evidence (#294; 0 = never strip: master
    // contact sheets carry no page context, and an authored arc that
    // matches no radius is preserved).
    jump_radius: (f64, f64),
    out: &mut String,
) {
    let mut chain: Vec<&'a Shape> = chain.to_vec();
    chain.push(s);
    let env = shape_env(doc, s, &chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let (pin_x, pin_y) = (get("PinX").unwrap_or(0.0), get("PinY").unwrap_or(0.0));
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    let (loc_x, loc_y) = (
        get("LocPinX").unwrap_or(width / 2.0),
        get("LocPinY").unwrap_or(height / 2.0),
    );
    let angle = get("Angle").unwrap_or(0.0);
    let flip_x = get("FlipX").unwrap_or(0.0) != 0.0;
    let flip_y = get("FlipY").unwrap_or(0.0) != 0.0;
    let (sx, sy) = (
        if flip_x { -1.0 } else { 1.0 },
        if flip_y { -1.0 } else { 1.0 },
    );

    let transform = format!(
        "translate({px:.6},{py:.6}) rotate({deg:.6}) scale({sx},{sy}) translate({lx:.6},{ly:.6})",
        px = pin_x,
        py = page_h - pin_y,
        deg = -angle.to_degrees(),
        lx = -loc_x,
        ly = loc_y,
    );

    let _ = writeln!(out, r#"<g transform="{t}">"#, t = escape_attr(&transform));

    // A multi-root master (several shapes on the master canvas, e.g. a
    // 13-shape header cluster) is drawn entirely by the mirroring below;
    // rendering the instance's own/effective geometry too would double
    // the first root.
    let multi_root = multi_root_master(doc, s);
    if !multi_root {
        render_all_geometry(doc, s, &chain, width, height, jump_radius, out);
        // Embedded image (ForeignData → media part via this part's rels).
        if let Some(uri) = foreign_image_uri(doc, part, s, &chain) {
            let _ = writeln!(
                out,
                r#"<image x="0" y="{y:.5}" width="{w:.5}" height="{h:.5}" href="{u}" preserveAspectRatio="none"/>"#,
                y = -height,
                w = width,
                h = height,
                u = escape_attr(&uri),
            );
        } else if let Some(pic) = foreign_emf_svg(doc, part, s, &chain, width, height) {
            out.push_str(&pic);
        }
    }
    // Sub-shapes of a group live in the parent's local frame.
    for sub in &s.sub_shapes {
        // #236: a sub whose subtree box misses the cull rect paints
        // nothing inside the patch — skip it and its whole subtree.
        if cull.is_some_and(|c| c.skips(sub.id)) {
            continue;
        }
        render_shape_local(doc, sub, &chain, part, skip_text, greek, cull, fields, out);
    }
    mirror_master(doc, s, &chain, width, height, greek, fields, out);

    emit_text(
        doc,
        s,
        &chain,
        width,
        height,
        sx,
        sy,
        skip_text == Some(s.id),
        greek,
        fields,
        out,
    );
    let _ = writeln!(out, "</g>");
}

/// Render a shape whose XForm is relative to its parent's local frame
/// (used for sub-shapes of groups).
#[allow(clippy::too_many_arguments)] // the render walk's context rides one call (#236 cull)
fn render_shape_local<'a>(
    doc: &'a Document,
    s: &'a Shape,
    chain: &[&'a Shape],
    part: &str,
    skip_text: Option<i64>,
    greek: Option<&GreekCtx>,
    cull: Option<&InkCull>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
    out: &mut String,
) {
    let mut chain: Vec<&'a Shape> = chain.to_vec();
    chain.push(s);
    let env = shape_env(doc, s, &chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    // A group member mirrors about its own LocPin like a page shape
    // (Visio 2021 witness: the network Router master's arrow sub-shapes
    // carry FlipX=1, and drawn unflipped two of the four arrows pointed
    // the wrong way).
    let flip = |n: &str| {
        if get(n).unwrap_or(0.0) != 0.0 {
            -1.0
        } else {
            1.0
        }
    };
    let (sx, sy) = (flip("FlipX"), flip("FlipY"));
    let transform = format!(
        "translate({px:.6},{py:.6}) rotate({deg:.6}) scale({sx},{sy}) translate({lx:.6},{ly:.6})",
        px = get("PinX").unwrap_or(0.0),
        // group-local Y is already flipped by the parent group's scale
        py = -get("PinY").unwrap_or(0.0),
        deg = -get("Angle").unwrap_or(0.0).to_degrees(),
        lx = -get("LocPinX").unwrap_or(width / 2.0),
        ly = get("LocPinY").unwrap_or(height / 2.0),
    );
    let _ = writeln!(out, r#"<g transform="{}">"#, escape_attr(&transform));
    let multi_root = multi_root_master(doc, s);
    if !multi_root {
        // Group children aren't in the jump overlay's crossing set
        // (page_jumps collects top-level connectors only, #294
        // review): their stored jump arcs keep rendering as ink.
        render_all_geometry(doc, s, &chain, width, height, (0.0, 0.0), out);
        if let Some(uri) = foreign_image_uri(doc, part, s, &chain) {
            let _ = writeln!(
                out,
                r#"<image x="0" y="{y:.5}" width="{w:.5}" height="{h:.5}" href="{u}" preserveAspectRatio="none"/>"#,
                y = -height,
                w = width,
                h = height,
                u = escape_attr(&uri),
            );
        } else if let Some(pic) = foreign_emf_svg(doc, part, s, &chain, width, height) {
            out.push_str(&pic);
        }
    }
    for sub in &s.sub_shapes {
        // #236: the cull test, at every subtree level.
        if cull.is_some_and(|c| c.skips(sub.id)) {
            continue;
        }
        render_shape_local(doc, sub, &chain, part, skip_text, greek, cull, fields, out);
    }
    mirror_master(doc, s, &chain, width, height, greek, fields, out);
    // The text keeps its glyphs upright inside the member's flip, as on
    // the page.
    emit_text(
        doc,
        s,
        &chain,
        width,
        height,
        sx,
        sy,
        skip_text == Some(s.id),
        greek,
        fields,
        out,
    );
    let _ = writeln!(out, "</g>");
}

/// True when `s` is an instance (no inline children) of a master whose
/// canvas carries several root shapes — the whole master drawing must be
/// mirrored into the instance rect.
fn multi_root_master(doc: &Document, s: &Shape) -> bool {
    s.sub_shapes.is_empty()
        && s.master
            .and_then(|id| doc.master(id))
            .map(|m| m.shapes.len() > 1)
            .unwrap_or(false)
}

/// Master mirroring for an instance with no inline children.
/// - multi-root master: every root shape renders, positioned by its own
///   master-canvas Xform, with the union bbox mapped into the instance
///   rect (Visio draws the master's whole canvas).
/// - single-root master: the instance/effective sheet already draws the
///   root's own shape; mirror the root's sub-shape tree. Each sub-shape's
///   XForm is re-evaluated against the *instance* group size (Visio
///   semantics: cells whose formulas reference the parent stretch with
///   it; `GUARD(4MM)`-style absolute cells keep their size — issue #15),
///   emitting a corrective transform per child. Cells we cannot
///   evaluate fall back to the uniform instance/master scale, so masters
///   without authored formulas keep their historical rendering.
#[allow(clippy::too_many_arguments)] // the render walk's context rides one flat set
fn mirror_master(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    width: f64,
    height: f64,
    greek: Option<&GreekCtx>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
    out: &mut String,
) {
    if !s.sub_shapes.is_empty() {
        return;
    }
    let Some(m) = s.master.and_then(|id| doc.master(id)) else {
        return;
    };
    let mpart = &m.part;
    if m.shapes.len() > 1 {
        // Union bbox (y-up master canvas) over all roots.
        let rect = |r: &Shape| {
            let c = |n: &str, d: f64| r.sheet.cell(n).and_then(|x| x.numeric()).unwrap_or(d);
            let w = c("Width", 1.0).max(1e-6);
            let h = c("Height", 1.0).max(1e-6);
            let px = c("PinX", 0.0);
            let py = c("PinY", 0.0);
            let lx = c("LocPinX", w / 2.0);
            let ly = c("LocPinY", h / 2.0);
            (px - lx, py - ly, w, h)
        };
        let mut bbox: Option<(f64, f64, f64, f64)> = None;
        for r in &m.shapes {
            let (x0, y0, w, h) = rect(r);
            bbox = Some(match bbox {
                None => (x0, y0, w, h),
                Some((bx, by, bw, bh)) => {
                    let x1 = (bx + bw).max(x0 + w);
                    let y1 = (by + bh).max(y0 + h);
                    let nx = bx.min(x0);
                    let ny = by.min(y0);
                    (nx, ny, x1 - nx, y1 - ny)
                }
            });
        }
        let Some((bx, by, bw, bh)) = bbox else { return };
        let kx = width / bw.max(1e-6);
        let ky = height / bh.max(1e-6);
        // Root emitted coords are (x, -py) (local y-flip); this wrapper
        // maps the bbox to the instance rect (0..width, 0..height).
        let _ = writeln!(
            out,
            r#"<g transform="scale({kx:.6},{ky:.6}) translate({tx:.6},{ty:.6})">"#,
            tx = -bx,
            ty = -(by + bh),
        );
        for r in &m.shapes {
            // Master-canvas roots of a multi-root master: the union
            // bbox maps into the instance rect, which the node's own
            // ink box bounds — no cull of their own (#236).
            render_shape_local(doc, r, &[], mpart, None, greek, None, fields, out);
        }
        let _ = writeln!(out, "</g>");
    } else if let Some(root) = m.shapes.first() {
        let mw = root
            .sheet
            .cell("Width")
            .and_then(|c| c.numeric())
            .filter(|v| v.abs() > 1e-9);
        let mh = root
            .sheet
            .cell("Height")
            .and_then(|c| c.numeric())
            .filter(|v| v.abs() > 1e-9);
        let (kx, ky) = (
            mw.map_or(1.0, |mw| width / mw),
            mh.map_or(1.0, |mh| height / mh),
        );
        let env = mirror_env(root, s, width, height);
        for sub in &root.sub_shapes {
            let wrapper = mirror_child_transform(sub, &env, (kx, ky));
            if let Some(t) = &wrapper {
                let _ = writeln!(out, r#"<g transform="{}">"#, escape_attr(t));
            }
            // Mirrored master children map into the instance's local
            // frame, bounded by the node's own ink box — no cull of
            // their own (#236).
            render_shape_local(doc, sub, chain, mpart, None, greek, None, fields, out);
            if wrapper.is_some() {
                let _ = writeln!(out, "</g>");
            }
        }
    }
}

/// Evaluation environment for re-evaluating a master root's sub-shape
/// XForms at instance size (issue #15). Shared implementation in
/// `vsdx::model::instance_env` — the editor's cache refresh (dc-ui,
/// issue #59) computes from the same environment, so a refreshed cache
/// and a mirrored placement agree.
pub(crate) fn mirror_env(
    root: &Shape,
    instance: &Shape,
    width: f64,
    height: f64,
) -> vsdx::formula::Env {
    vsdx::model::instance_env(root, instance, width, height)
}

/// Corrective transform for one mirrored sub-shape: maps where
/// [`render_shape_local`] places it from its master XForm (`M`) to where
/// Visio places it at the instance's group size (`E`, from re-evaluated
/// formulas, falling back to uniform `k`·master per axis). Result:
/// `Some("identity-like scale")` / `Some(matrix)` / `None` when no
/// correction is needed.
pub(crate) fn mirror_child_transform(
    sub: &Shape,
    root_env: &vsdx::formula::Env,
    (kx, ky): (f64, f64),
) -> Option<String> {
    let num = |n: &str| sub.sheet.cell(n).and_then(|c| c.numeric());
    let m_w = num("Width").filter(|v| *v > 1e-9).unwrap_or(1.0);
    let m_h = num("Height").filter(|v| *v > 1e-9).unwrap_or(1.0);
    let m = |n: &str, d: f64| num(n).unwrap_or(d);
    let (m_px, m_py) = (m("PinX", 0.0), m("PinY", 0.0));
    let (m_lx, m_ly) = (m("LocPinX", m_w / 2.0), m("LocPinY", m_h / 2.0));
    let ang = m("Angle", 0.0);

    // Effective values: formula first (Visio re-evaluates against the
    // instance group), then the uniform-scale fallback.
    // Seed the env with only the root's QUALIFIED `Sheet.N!` spellings
    // (`vsdx::model::qualified_env`): a bare name in a child formula
    // means the child's own cell, so the root's unqualified
    // Width/Height/User.* must not leak in — without the filter a bare
    // reference silently reads the group's value and skips the per-axis
    // fallback below. (Child sheets are not in this env; bare
    // child-cell references fail to resolve and fall back.)
    let mut env: vsdx::formula::Env = vsdx::model::qualified_env(root_env);
    let eff = |name: &str, axis: f64, env: &vsdx::formula::Env| -> Option<f64> {
        let c = sub.sheet.cell(name)?;
        if let Some(f) = c.f.as_deref() {
            if f != "Inh" && f != "No Formula" {
                if let Some(v) = vsdx::formula::eval(f, env)
                    .ok()
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite())
                {
                    return Some(v);
                }
            }
        }
        c.numeric().map(|v| v * axis)
    };
    let e_w = eff("Width", kx, &env).unwrap_or(kx * m_w);
    let e_h = eff("Height", ky, &env).unwrap_or(ky * m_h);
    // Pin/LocPin formulas may reference the shape's own effective size.
    env.insert("Width".into(), e_w);
    env.insert("Height".into(), e_h);
    let e_px = eff("PinX", kx, &env).unwrap_or(0.0);
    let e_py = eff("PinY", ky, &env).unwrap_or(0.0);
    let e_lx = eff("LocPinX", kx, &env).unwrap_or(e_w / 2.0);
    let e_ly = eff("LocPinY", ky, &env).unwrap_or(e_h / 2.0);

    // Matrices in SVG space (y-down), row-vector convention:
    // (a b c d e f) ≡ x' = a·x + c·y + e ; y' = b·x + d·y + f.
    let ident = Mat::identity();
    let rot = Mat::rotation(-ang);
    // Where render_shape_local places the shape from master values.
    let placed = Mat::translate(m_px, -m_py)
        .mul(&rot)
        .mul(&Mat::translate(-m_lx, m_ly));
    // Where Visio places it at instance size: same pin convention, the
    // effective LocPin, and the local frame scaled to the effective size.
    let wanted = Mat::translate(e_px, -e_py)
        .mul(&rot)
        .mul(&Mat::translate(-e_lx, e_ly))
        .mul(&Mat::scale(e_w / m_w, e_h / m_h));
    let w = wanted.mul(&placed.inv()?);
    const EPS: f64 = 1e-9;
    if w.near(&ident, EPS) {
        return None;
    }
    if w.0[1].abs() < EPS && w.0[2].abs() < EPS && w.0[4].abs() < EPS && w.0[5].abs() < EPS {
        return Some(format!("scale({:.6},{:.6})", w.0[0], w.0[3]));
    }
    Some(format!(
        "matrix({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
        w.0[0], w.0[1], w.0[2], w.0[3], w.0[4], w.0[5]
    ))
}

/// 2D affine transform as SVG `matrix(a,b,c,d,e,f)`.
#[derive(Debug, Clone)]
pub(crate) struct Mat(pub(crate) [f64; 6]);

impl Mat {
    pub(crate) fn identity() -> Mat {
        Mat([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }
    pub(crate) fn translate(tx: f64, ty: f64) -> Mat {
        Mat([1.0, 0.0, 0.0, 1.0, tx, ty])
    }
    pub(crate) fn scale(sx: f64, sy: f64) -> Mat {
        Mat([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }
    pub(crate) fn rotation(rad: f64) -> Mat {
        let (s, c) = rad.sin_cos();
        Mat([c, s, -s, c, 0.0, 0.0])
    }
    /// self ∘ other (apply `other` first).
    pub(crate) fn mul(&self, other: &Mat) -> Mat {
        let a = &self.0;
        let b = &other.0;
        Mat([
            a[0] * b[0] + a[2] * b[1],
            a[1] * b[0] + a[3] * b[1],
            a[0] * b[2] + a[2] * b[3],
            a[1] * b[2] + a[3] * b[3],
            a[0] * b[4] + a[2] * b[5] + a[4],
            a[1] * b[4] + a[3] * b[5] + a[5],
        ])
    }
    fn inv(&self) -> Option<Mat> {
        let m = &self.0;
        let det = m[0] * m[3] - m[1] * m[2];
        if det.abs() < 1e-12 {
            return None;
        }
        let inv00 = m[3] / det;
        let inv01 = -m[1] / det;
        let inv10 = -m[2] / det;
        let inv11 = m[0] / det;
        Some(Mat([
            inv00,
            inv01,
            inv10,
            inv11,
            -(inv00 * m[4] + inv10 * m[5]),
            -(inv01 * m[4] + inv11 * m[5]),
        ]))
    }
    fn near(&self, other: &Mat, eps: f64) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .all(|(a, b)| (a - b).abs() <= eps)
    }
}

/// Data URI for a shape's embedded image (`ForeignData`), resolved through
/// the owning part's relationships to the media part. EMF/WMF cannot be
/// embedded in SVG and are skipped.
fn foreign_image_uri(doc: &Document, part: &str, s: &Shape, chain: &[&Shape]) -> Option<String> {
    // A master instance carries no ForeignData of its own: the picture
    // lives on the master shape, its media relationship on the MASTER
    // part (the network LLDs' icons — every one of them).
    let (src_shape, part) = foreign_data_owner(doc, part, s, chain)?;
    let s = src_shape;
    let part = part.as_str();
    let attr = |n: &str| -> Option<&str> {
        s.raw_children
            .iter()
            .find(|r| r.name == "ForeignData")?
            .attrs
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    if attr("ForeignType") != Some("Bitmap") {
        return None; // OLE objects / metafiles: not image parts
    }
    let mime = match attr("CompressionType") {
        Some("PNG") => "image/png",
        Some("JPEG") | Some("JPG") => "image/jpeg",
        Some("GIF") => "image/gif",
        // No CompressionType: a plain BMP (Visio 2021 witness b03) —
        // re-encoded as PNG below, SVG viewers cannot show BMP.
        None | Some("BMP") | Some("DIB") => "image/bmp",
        _ => return None, // TIFF and unknowns stay unrendered
    };
    let fd = s.raw_children.iter().find(|r| r.name == "ForeignData")?;
    // The rel id lives either on ForeignData itself or in a <Rel r:id=…/>
    // child (the shipped layout).
    let rid = fd
        .attrs
        .iter()
        .find(|(k, _)| k == "r:id")
        .map(|(_, v)| v.clone())
        .or_else(|| extract_rid(&fd.inner))?;
    // Runtime-inserted media (#113) resolves through `extra_media`
    // by the owning page's part — no retained package needed; the
    // package relationship wins when it carries this rid.
    let target: String = doc
        .package
        .as_ref()
        .and_then(|pkg| {
            pkg.relationships(part)
                .into_iter()
                .find(|(id, ty, _)| id == &rid && ty.ends_with("/image"))
                .map(|(_, _, t)| t)
        })
        .or_else(|| {
            let page_id = doc.pages.iter().find(|p| p.part == part).map(|p| p.id)?;
            doc.extra_media
                .iter()
                .find(|m| m.page == page_id && m.rid == rid)
                .map(|m| m.part.clone())
        })?;
    // `relationships` returns the target already resolved to an absolute
    // package part name. Runtime-inserted media (#113) lives in
    // `extra_media`, not the retained package.
    let name = target.trim_start_matches('/');
    let bytes = doc
        .package
        .as_ref()
        .and_then(|pkg| pkg.part(name))
        .or_else(|| {
            doc.extra_media
                .iter()
                .find(|m| m.part == name)
                .map(|m| m.bytes.as_slice())
        })?;
    if mime == "image/bmp" {
        let png = crate::emf::bmp_to_png(bytes)?;
        return Some(format!("data:image/png;base64,{}", base64(&png)));
    }
    Some(format!("data:{mime};base64,{}", base64(bytes)))
}

/// A shape's embedded EMF picture (`ForeignType='EnhMetaFile'`) replayed
/// as SVG elements in the box `(0,-h)…(w,0)` — network-stencil icons and
/// pasted clip art (Visio 2021 witness b03; ~150 icons in the network
/// LLDs rendered empty).
fn foreign_emf_svg(
    doc: &Document,
    part: &str,
    s: &Shape,
    chain: &[&Shape],
    w: f64,
    h: f64,
) -> Option<String> {
    let (s, part) = foreign_data_owner(doc, part, s, chain)?;
    let part = part.as_str();
    let fd = s.raw_children.iter().find(|r| r.name == "ForeignData")?;
    let ty = fd
        .attrs
        .iter()
        .find(|(k, _)| k == "ForeignType")
        .map(|(_, v)| v.as_str());
    if ty != Some("EnhMetaFile") {
        return None;
    }
    let rid = fd
        .attrs
        .iter()
        .find(|(k, _)| k == "r:id")
        .map(|(_, v)| v.clone())
        .or_else(|| extract_rid(&fd.inner))?;
    let pkg = doc.package.as_ref()?;
    let target = pkg
        .relationships(part)
        .into_iter()
        .find(|(id, ty, _)| id == &rid && ty.ends_with("/image"))
        .map(|(_, _, t)| t)?;
    let bytes = pkg.part(target.trim_start_matches('/'))?;
    // Legacy stencils store WMF bytes under the EnhMetaFile type (the
    // network LLDs' bridge/ONT icons).
    crate::emf::emf_to_svg(bytes, w, h).or_else(|| crate::emf::wmf_to_svg(bytes, w, h))
}

/// The shape that owns a picture's `ForeignData` and the package part
/// whose relationships resolve its media: the shape itself on `part`, or
/// — for a master instance without its own ForeignData — the master
/// shape on the master's part.
fn foreign_data_owner<'a>(
    doc: &'a Document,
    part: &str,
    s: &'a Shape,
    chain: &[&'a Shape],
) -> Option<(&'a Shape, String)> {
    if s.raw_children.iter().any(|r| r.name == "ForeignData") {
        return Some((s, part.to_string()));
    }
    let src = doc.instance_source(s, chain)?;
    if !src.raw_children.iter().any(|r| r.name == "ForeignData") {
        return None;
    }
    let mid = s
        .master
        .or_else(|| chain.iter().rev().find_map(|a| a.master))?;
    let m = doc.master(mid)?;
    Some((src, m.part.clone()))
}

/// `<Rel r:id='rId9'/>` → `rId9` (both quote styles).
fn extract_rid(inner: &str) -> Option<String> {
    let i = inner.find("r:id")?;
    let rest = &inner[i + 5..];
    let q = rest.chars().next()?;
    let end = rest[1..].find(q)? + 1;
    Some(rest[1..end].to_string())
}

pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A Geometry section's NoFill/NoLine/NoShow flag as the painter reads
/// it: the formula evaluated in `env` first, the cached V as fallback.
/// The ink-bounds walk skips a NoShow section through the same rule
/// (CodeRabbit on PR #381).
pub(crate) fn section_flag(section: &vsdx::Section, name: &str, env: &vsdx::formula::Env) -> bool {
    section.cell(name).is_some_and(|c| {
        let by_formula =
            c.f.as_deref()
                .and_then(|f| {
                    if f == "Inh" || f == "No Formula" {
                        None
                    } else {
                        vsdx::formula::eval(f, env).ok().and_then(|v| v.as_f64())
                    }
                })
                .filter(|v| v.is_finite());
        by_formula.or_else(|| c.numeric()).is_some_and(|v| v != 0.0)
    })
}

/// A shape's OWN Geometry sections merged over its master source's, in
/// instance units: what [`render_all_geometry`] paints for a shape that
/// carries inline geometry. Inline copies often keep only some cells
/// (Y with `F='Inh'`) or drop whole rows; the missing cells and rows
/// come from the master source's matching row (by IX, else occurrence),
/// scaled into the instance frame. The ink-bounds walk reads the same
/// merge (#378, #302): reading the raw inline rows gave a looped
/// connector a sliver of bounds, and the canvas layer clipped the loop.
pub(crate) fn merged_own_geometry(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    width: f64,
    height: f64,
    env: &vsdx::formula::Env,
) -> Vec<vsdx::Section> {
    let own: Vec<&vsdx::Section> = s.sheet.sections_named("Geometry");
    // Row-cell inheritance: inline copies often keep only the Y cells
    // (F='Inh') and inherit X/A/B/C/D from the master source's
    // corresponding row. Inherited values are master-local and are
    // scaled into instance units here.
    let src = doc.instance_source(s, chain);
    let mut merged: Vec<vsdx::Section> = Vec::with_capacity(own.len());
    for geo in own.iter() {
        let mut m = (*geo).clone();
        if let Some(src) = src {
            let src_secs = src.sheet.sections_named("Geometry");
            // The master section of the SAME IX: an inline copy that
            // keeps only Geometry IX=1 pairs with the master's IX=1,
            // not with whatever sits first (CodeRabbit on PR #381).
            let src_geo = src_secs.iter().find(|g| g.ix == geo.ix);
            // Section-level cells (Geometry NoFill/NoLine/NoShow)
            // inherit from the master section just like rows do:
            // inline copies keep only row caches, and a lost NoShow
            // draws sections Visio hides — e.g. every heading-band
            // geometry of the "Plain" container style (#12).
            if let Some(sg) = src_geo {
                for (name, cell) in &sg.cells {
                    if !m.cells.iter().any(|(n, _)| n == name) {
                        m.cells.push((name.clone(), cell.clone()));
                    }
                }
            }
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
            let (sx, sy) = (
                mw.map_or(1.0, |mw| width / mw),
                mh.map_or(1.0, |mh| height / mh),
            );
            let scaled = |name: &str, v: f64| match name {
                "X" | "A" | "C" => v * sx,
                "Y" | "B" | "D" => v * sy,
                _ => v,
            };
            // EllipticalArcTo's C is the major-axis ANGLE and D the
            // axis RATIO — neither is a coordinate, so an inherited
            // (omitted = unchanged) value never scales with the
            // instance size (a stretched U-Turn arrow turned D=1
            // into a 9:1 ratio and its quarter circles into blobs).
            let not_coord = |t: Option<&str>, name: &str| {
                matches!(t, Some("EllipticalArcTo") | Some("RelEllipticalArcTo"))
                    && matches!(name, "C" | "D")
            };
            // A Rel* row's coordinates are FRACTIONS of Width/Height
            // that the painter scales itself: an inherited literal
            // fraction stays as it is (scaling it placed the point
            // Width times too far), while an inherited formula still
            // evaluates live in the instance env (CodeRabbit on
            // PR #381).
            //
            // Which cells are fractions follows the painter: every Rel*
            // coordinate goes through `rel_adjust` except a
            // RelEllipticalArcTo's A/B control point, which it reads as
            // is, so that one keeps the dimensional scaling (CodeRabbit
            // on PR #381, third round).
            let rel_fraction = |t: Option<&str>, name: &str| {
                t.is_some_and(|t| t.starts_with("Rel"))
                    && !(t == Some("RelEllipticalArcTo") && matches!(name, "A" | "B"))
            };
            // Pair each inline row with the master row of the SAME
            // IX. Inline copies drop rows whose cells are fully
            // inherited-identical, keeping the survivors' original
            // IX values — so occurrence pairing can match the wrong
            // row (diagonal wedges, giant arcs). Occurrence within a
            // row type is the fallback when IX is absent/ambiguous.
            let mut src_occ: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            // Rows the inline copy dropped entirely (all cells
            // inherited-identical) are re-inserted from the master,
            // scaled, at their IX position — otherwise the path
            // shortcuts across the gap (diagonal wedges).
            if let (Some(sg), true) = (src_geo, m.rows.iter().any(|r| r.ix > 0)) {
                let have: Vec<i64> = m.rows.iter().map(|r| r.ix).collect();
                let mut inserted: Vec<(usize, Row)> = Vec::new();
                for src_row in &sg.rows {
                    if src_row.ix > 0
                        && !have.contains(&src_row.ix)
                        && src_row.t.is_some()
                        && !src_row.is_deleted()
                    {
                        let mut nr = src_row.clone();
                        // scale per cell name
                        // The same rule as the omitted-CELL fallback
                        // below (Greptile on PR #343): a size formula
                        // evaluates in the instance env; one the env
                        // cannot evaluate keeps the master's cache as
                        // is, since an omitted row is unchanged.
                        let bound = 3.0 * width.max(height).max(0.02);
                        for (cname, cell) in nr.cells.iter_mut() {
                            let proportional = cell.f.as_deref().map(refs_own_size).unwrap_or(true);
                            let has_formula = cell
                                .f
                                .as_deref()
                                .is_some_and(|f| f != "Inh" && f != "No Formula");
                            if let Some(v) = cell.numeric() {
                                let placed =
                                    if !proportional || not_coord(src_row.t.as_deref(), cname) {
                                        v
                                    } else if has_formula {
                                        cell.f
                                            .as_deref()
                                            .and_then(|f| vsdx::formula::eval(f, env).ok())
                                            .and_then(|e| e.as_f64())
                                            .filter(|e| e.is_finite() && e.abs() <= bound)
                                            .unwrap_or(v)
                                    } else if rel_fraction(src_row.t.as_deref(), cname) {
                                        v
                                    } else {
                                        scaled(cname, v)
                                    };
                                cell.v = Some(format!("{placed}"));
                            }
                            cell.f = None;
                        }
                        // position: after the last own row with smaller IX
                        let pos = m
                            .rows
                            .iter()
                            .rposition(|r| r.ix > 0 && r.ix < src_row.ix)
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        inserted.push((pos, nr));
                    }
                }
                for (pos, nr) in inserted.into_iter().rev() {
                    let pos = pos.min(m.rows.len());
                    m.rows.insert(pos, nr);
                }
            }
            for row in m.rows.iter_mut() {
                let Some(t) = row.t.clone() else { continue };
                let by_ix =
                    src_geo.and_then(|sg| sg.rows.iter().find(|r| r.ix == row.ix && r.ix > 0));
                let occ = {
                    let c = src_occ.entry(t.clone()).or_insert(0);
                    *c += 1;
                    *c
                };
                let by_occ = src_geo.and_then(|sg| {
                    sg.rows
                        .iter()
                        .filter(|r| r.t.as_deref() == Some(t.as_str()))
                        .nth(occ - 1)
                });
                let src_row = by_ix.or(by_occ);
                let Some(src_row) = src_row else { continue };
                let bound = 3.0 * width.max(height).max(0.02);
                let mut extra: Vec<(String, Cell)> = Vec::new();
                for (cname, scell) in &src_row.cells {
                    // A PolylineTo's A cell is formula TEXT, not a
                    // number: the numeric merge below would drop it
                    // and the row would draw a chord. An instance row
                    // without its own POLYLINE text takes the
                    // master's; absolute axes (type 1) are master
                    // units and scale like X/Y, fractional ones stay
                    // (CodeRabbit on PR #329). Only a PolylineTo
                    // instance row takes it: an instance that turned
                    // the row into an ArcTo keeps its own numeric A
                    // (Greptile on PR #329).
                    if cname == "A" && src_row.t.as_deref() == Some("PolylineTo") {
                        if row.t.as_deref() != Some("PolylineTo") {
                            continue;
                        }
                        let own_text = row.cells.iter().any(|(n, c)| {
                            n == "A"
                                && c.v
                                    .as_deref()
                                    .or(c.f.as_deref())
                                    .is_some_and(|t| t.trim_start().starts_with("POLYLINE"))
                        });
                        let master_text = scell
                            .v
                            .as_deref()
                            .or(scell.f.as_deref())
                            .unwrap_or_default();
                        if !own_text {
                            if let Some((xt, yt, verts)) = crate::curve::parse_polyline(master_text)
                            {
                                let mut text = format!("POLYLINE({xt}, {yt}");
                                for (vx, vy) in verts {
                                    let vx = if xt == 0.0 { vx } else { scaled("X", vx) };
                                    let vy = if yt == 0.0 { vy } else { scaled("Y", vy) };
                                    let _ = write!(text, ", {vx},{vy}");
                                }
                                text.push(')');
                                let cell = Cell {
                                    v: Some(text),
                                    ..Cell::default()
                                };
                                match row.cells.iter_mut().find(|(n, _)| n == "A") {
                                    Some((_, c)) => *c = cell,
                                    None => extra.push(("A".to_string(), cell)),
                                }
                            }
                        }
                        continue;
                    }
                    if !matches!(cname.as_str(), "X" | "Y" | "A" | "B" | "C" | "D") {
                        continue;
                    }
                    // Master cells proportional to Width/Height scale
                    // into instance units; User.*-driven cells (corner
                    // radii and the like) stay absolute — instances
                    // keep those unscaled while the body stretches.
                    let proportional = scell.f.as_deref().map(refs_own_size).unwrap_or(true);
                    let placed = |v: f64| {
                        if proportional
                            && !not_coord(src_row.t.as_deref(), cname)
                            && !rel_fraction(src_row.t.as_deref(), cname)
                        {
                            scaled(cname, v)
                        } else {
                            v
                        }
                    };
                    // Formula-authoritative for PROPORTIONAL master
                    // cells (the same rule the row walk uses):
                    // corpus connector masters carry stale zero
                    // caches (`Y V=0 F=Height/2` — utilisation
                    // master3) that would collapse the inherited
                    // arcs to flat lines (#202). The formula
                    // evaluates against the INSTANCE dims — for a
                    // proportional formula that IS the
                    // master→instance scaling. Non-proportional
                    // formulas (`Controls.Row_1`, `User.*`) have no
                    // instance meaning; their cached value (if any)
                    // stands, unfilled otherwise.
                    // Formula-authoritative cells evaluate against
                    // the INSTANCE env — the value is already in
                    // instance units and must NOT be placed-scaled
                    // again (#203 review). Cached master-unit
                    // numerics are master-unit and keep placed().
                    // A value-only master cell (no formula — the
                    // common plain-geometry case) or a formula the
                    // instance env can't evaluate falls back to the
                    // scaled cache; skipping would let row_num
                    // default the axis to 0 and collapse the
                    // inherited geometry (#185 review).
                    // A master FORMULA the instance env cannot
                    // evaluate (`Height-User.BarWidth` with the
                    // master's User row out of reach) keeps the
                    // master's cached value AS IS: Visio writes every
                    // instance cell whose value changed (F='Inh' plus
                    // the new V), so an omitted cell is unchanged —
                    // scaling it bent the U-Turn arrow's return leg
                    // into a blob (Visio 2021 witness, the network
                    // LLD's U-Turn arrows stretched to 9.7 × 1 in).
                    let has_formula = scell
                        .f
                        .as_deref()
                        .is_some_and(|f| f != "Inh" && f != "No Formula");
                    let sv = if proportional {
                        scell
                            .f
                            .as_deref()
                            .and_then(|f| vsdx::formula::eval(f, env).ok())
                            .and_then(|v| v.as_f64())
                            .filter(|v| v.is_finite() && v.abs() <= bound)
                            .or_else(|| {
                                let v = scell.numeric()?;
                                let v = if has_formula { v } else { placed(v) };
                                Some(v).filter(|v| v.abs() <= bound)
                            })
                    } else {
                        scell.numeric().map(placed).filter(|v| v.abs() <= bound)
                    };
                    let Some(sv) = sv else {
                        continue;
                    };
                    let patched = Cell::value(format!("{sv}"));
                    match row.cells.iter_mut().find(|(n, _)| n == cname) {
                        Some((_, cell)) => {
                            // A local numeric V wins even under F='Inh':
                            // Visio refreshes cached values at save, so
                            // e.g. a legend member's flattened arc B=0 is
                            // the live evaluation, not a stale snapshot.
                            if cell.v.is_none() || (cell.is_inherited() && cell.numeric().is_none())
                            {
                                *cell = patched;
                            }
                        }
                        None => extra.push((cname.clone(), patched)),
                    }
                }
                row.cells.extend(extra);
            }
        }
        merged.push(m);
    }
    merged
}

/// Per-section master inheritance (Visio semantics): geometry comes from
/// the instance's own Geometry sections when present (authored in
/// instance units), else from the master source shape's Geometry (master-
/// local units, scaled to the instance size). The instance often carries
/// other inline sections (Character/Paragraph) — that must not mask the
/// inherited geometry.
pub(crate) fn render_all_geometry(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    width: f64,
    height: f64,
    jump_radius: (f64, f64),
    out: &mut String,
) {
    // Groups usually don't self-draw (children + the master drawing
    // carry the visuals) but container groups DO carry a visible rect,
    // so group geometry renders too; degenerate leftover rows collapse
    // to invisible zero-area paths after the row-cell merge below.
    let env = shape_env(doc, s, chain);
    // #228: both frames below accumulate into ONE shape-level paint —
    // the alternate-fill union flushes under the per-section strokes.
    let mut paint = ShapePaint::default();
    // #294: a connector's stored jump ArcTo rows straighten — the
    // jump overlay owns the one arc at the current crossing, and a
    // stored arc stops lingering after a move. `strip_jumps` arrives
    // from the caller (the overlay covers exactly the top-level
    // shapes of a full-page render — #294 review); connector-ness
    // gates the shape kind.
    // Connector-ness gates at the strip site (render_geometry): artwork
    // arcs never strip even with a resolved radius.
    let own: Vec<&vsdx::Section> = s.sheet.sections_named("Geometry");
    if !own.is_empty() {
        let merged = merged_own_geometry(doc, s, chain, width, height, &env);
        for (i, geo) in merged.iter().enumerate() {
            // Instance frame: formulas and cached values are both in
            // instance units.
            let gname = format!("Geometry{}", i + 1);
            render_geometry(
                doc,
                s,
                chain,
                geo,
                (1.0, 1.0),
                Frame::Instance,
                &gname,
                jump_radius,
                out,
                &mut paint,
            );
        }
        flush_shape_paint(&mut paint, out);
        return;
    }
    let Some(src) = doc.instance_source(s, chain) else {
        return;
    };
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
    // The master env builds ONCE for all sections (knee rows
    // cross-reference sibling sections) and RE-EVALUATES the master's
    // Scratch/Controls against the instance's glued Begin/End — their
    // cached V is the authored pose, and the band masters derive
    // their whole vertical span from them (`Scratch.Y2 =
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
                            let rn = row.n.clone().unwrap_or_else(|| format!("Row_{}", row.ix));
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
    for (i, geo) in src.sheet.sections_named("Geometry").into_iter().enumerate() {
        // Master frame: rows are master-local; their formulas reference
        // the MASTER's dims and must not be re-evaluated in instance
        // units (that skews rectangles into triangles).
        render_geometry(
            doc,
            s,
            chain,
            geo,
            scale,
            Frame::Master(&mut menv),
            &format!("Geometry{}", i + 1),
            jump_radius,
            out,
            &mut paint,
        );
    }
    flush_shape_paint(&mut paint, out);
}

/// Which frame geometry rows live in — picks the formula environment.
/// The master variant carries the PREPARED env (master cells, the
/// instance's glued Begin/End seeded in).
enum Frame<'a> {
    Instance,
    /// The PREPARED env (master cells, the instance's glued Begin/End
    /// seeded in), SHARED by reference across every Geometry section
    /// of the frame (#218 review): render_geometry feeds evaluated row
    /// coordinates back into it (`Geometry1.X2`), and per-section
    /// clones discarded those writes — a Geometry2 referencing
    /// Geometry1 fell back to the authored pose.
    Master(&'a mut vsdx::formula::Env),
}

#[allow(clippy::too_many_arguments)] // the walk's natural inputs (+gname, #249)
fn render_geometry(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    section: &vsdx::Section,
    scale: (f64, f64),
    frame: Frame<'_>,
    gname: &str,
    jump_radius: (f64, f64),
    out: &mut String,
    paint: &mut ShapePaint,
) {
    let rows = &section.rows;
    let (width, height) = shape_size(doc, s, chain);
    // Formulas evaluate in the geometry's own frame: instance dims for
    // inline geometry, the master source's dims for inherited geometry.
    // The cached V already lives in that frame; evaluating a formula in
    // the WRONG frame (e.g. master rows against instance Width) skews
    // rectangles into triangles.
    let inherited_frame = matches!(frame, Frame::Master(_));
    let mut inst_env;
    let env: &mut vsdx::formula::Env = match frame {
        Frame::Instance => {
            inst_env = shape_env_for(doc, s, chain, rows);
            &mut inst_env
        }
        Frame::Master(env) => env,
    };
    // Section-level cells carry NoFill/NoLine/NoShow. Formula first,
    // cached V as fallback (the row-cell convention): the master's
    // cache reflects its default style while the formula re-evaluates
    // per instance — NoShow=OR(User.hidden,User.CStyle<>7) gates the
    // container heading's geometry bands by container style.
    let flag = |name: &str| -> bool { section_flag(section, name, env) };
    let no_fill = flag("NoFill");
    let no_show = flag("NoShow");
    if no_show {
        return;
    }
    // Visio hides the outline when LinePattern = 0 (e.g. legend label
    // boxes carry THEMEGUARD(0)); the geometry still exists for hit
    // testing but must not stroke.
    let line_pattern = doc
        .resolve_cell(s, chain, "LinePattern")
        .and_then(|c| cell_num(&c, env));
    let no_line = flag("NoLine") || line_pattern.is_some_and(|p| p == 0.0);
    fn row_num(env: &vsdx::formula::Env, r: &Row, c: &str) -> Option<f64> {
        r.cell(c).and_then(|cell| {
            // Geometry rows: the formula is authoritative (Visio evaluates
            // F at load; page copies can carry a stale cached V — e.g.
            // V='0' with F='Width*1' after a resize). Fall back to the
            // cache when there is no evaluable formula.
            let by_formula = cell
                .f
                .as_deref()
                .and_then(|f| {
                    if f == "Inh" || f == "No Formula" {
                        None
                    } else {
                        vsdx::formula::eval(f, env).ok().and_then(|v| v.as_f64())
                    }
                })
                .filter(|v| v.is_finite());
            by_formula.or_else(|| cell.numeric())
        })
    }

    let fill_color = themed_color_string(doc, s, chain, "FillForegnd");
    let line_color = themed_color_string(doc, s, chain, "LineColor");
    let fill_bkgnd = themed_color_string(doc, s, chain, "FillBkgnd");
    let line_weight = doc
        .resolve_cell(s, chain, "LineWeight")
        .and_then(|c| cell_num(&c, env))
        .unwrap_or(0.0104);
    // A 1-D shape's OPEN path (a line, an arc) has no interior: some
    // line masters carry decorative FillPattern cells (28/30) that must
    // be ignored, or the arc-chord region paints over everything. Its
    // CLOSED sections fill like any 2-D shape's — the block-arrow
    // masters (Simple/Notched/Double Arrow) are 1-D shapes whose one
    // closed outline Visio paints with FillForegnd (Visio 2021
    // witness, t04). The open/closed gate sits at the fill join below.
    let one_d = doc.resolve_cell_explicit(s, chain, "BeginX").is_some()
        || doc.resolve_cell_explicit(s, chain, "EndX").is_some();
    let fill_pattern = doc
        .resolve_cell_explicit(s, chain, "FillPattern")
        .and_then(|c| cell_num(&c, env))
        .unwrap_or(1.0);
    // Gradient fills: Visio theme fill patterns 24-40 are gradients
    // (FillForegnd → FillBkgnd along FillGradientAngle, default 90°
    // top-to-bottom). Everything else patterned >1.5 is a hatch: the
    // background dominates, so it renders as the background color.
    // Visio 2013+ gradients (FillGradientEnabled): the FillGradient
    // stop rows, FillGradientDir (0 linear, 1–7 radial, 8–13
    // rectangular) and FillGradientAngle — see `gradient_paint`. They
    // win over the legacy pattern gradients below.
    let fill_gradient = if no_fill || fill_pattern == 0.0 {
        None
    } else {
        gradient_paint(
            doc,
            s,
            chain,
            env,
            "Fill",
            (width, height),
            (&fill_color, &fill_bkgnd),
            out,
        )
    };
    let fill = if no_fill || fill_pattern == 0.0 {
        "none".to_string()
    } else if let Some(g) = &fill_gradient {
        match g {
            GradientPaint::Url(u) => u.clone(),
            // Rectangular: the solid color stands in on the path; the
            // clipped tiles replace it at flush time.
            GradientPaint::Tiles(_) => fill_color.clone(),
        }
    } else if (24.0..=40.0).contains(&fill_pattern) {
        let angle_deg = doc
            .resolve_cell_explicit(s, chain, "FillGradientAngle")
            .and_then(|c| cell_num(&c, env))
            .map(|rad| rad.to_degrees())
            .unwrap_or(90.0);
        let id = format!(
            "dc-grad{:x}",
            fnv(&format!("{fill_color}|{fill_bkgnd}|{angle_deg:.0}"))
        );
        if !out.contains(&format!(r#"id="{id}""#)) {
            // Angle from north, clockwise → SVG gradient vector.
            let rad = angle_deg.to_radians();
            let (x1, y1) = (0.5 - rad.sin() / 2.0, 0.5 + rad.cos() / 2.0);
            let (x2, y2) = (0.5 + rad.sin() / 2.0, 0.5 - rad.cos() / 2.0);
            let _ = writeln!(
                out,
                r#"<defs><linearGradient id="{id}" x1="{x1:.4}" y1="{y1:.4}" x2="{x2:.4}" y2="{y2:.4}"><stop offset="0" stop-color="{fg}"/><stop offset="1" stop-color="{bg}"/></linearGradient></defs>"#,
                fg = escape_attr(&fill_color),
                bg = escape_attr(&fill_bkgnd),
            );
        }
        format!("url(#{id})")
    } else if fill_pattern > 1.5 {
        match hatch_fill(fill_pattern, &fill_color, &fill_bkgnd, (width, height)) {
            Some(Hatch::Mixed(c)) => c,
            // Line patterns: the background color on the path; the
            // clipped lines replace it at flush time (`hatch_tiles`).
            _ => fill_bkgnd.clone(),
        }
    } else {
        fill_color.clone()
    };
    let hatch_tiles = if no_fill || fill_gradient.is_some() {
        None
    } else {
        match hatch_fill(fill_pattern, &fill_color, &fill_bkgnd, (width, height)) {
            Some(Hatch::Lines(t)) => Some(t),
            _ => None,
        }
    };
    // Fill/line transparency: every *Trans cell is a fraction (the
    // file stores 50 % as V='0.5'). A solid fill takes
    // FillForegndTrans, a pattern's dominant background color
    // FillBkgndTrans; the stroke (and its arrowheads) LineColorTrans.
    // Fully transparent paint is dropped outright — the network
    // stencils' magenta helper boxes carry Trans 100 % and must not
    // show (Visio 2021 witness).
    let trans = |n: &str| -> f64 {
        doc.resolve_cell(s, chain, n)
            .and_then(|c| cell_num(&c, env))
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    };
    let fill_alpha = 1.0
        - if fill_gradient.is_some() {
            0.0 // per-stop GradientStopColorTrans instead
        } else if fill_pattern > 1.5 && !(24.0..=40.0).contains(&fill_pattern) {
            trans("FillBkgndTrans")
        } else {
            trans("FillForegndTrans")
        };
    let line_alpha = 1.0 - trans("LineColorTrans");
    // LineGradientEnabled: the stroke paints with the LineGradient
    // stops (rectangular directions have no stroke form and fall back
    // to linear); arrowheads keep the solid LineColor.
    let stroke_paint = match gradient_paint(
        doc,
        s,
        chain,
        env,
        "Line",
        (width, height),
        (&line_color, &line_color),
        out,
    ) {
        Some(GradientPaint::Url(u)) => u,
        _ => line_color.clone(),
    };
    let no_fill = no_fill || fill_alpha <= 0.0;
    let no_line = no_line || line_alpha <= 0.0;
    let fill = if no_fill { "none".to_string() } else { fill };
    let dash = line_pattern.and_then(|p| line_dasharray(p, line_weight));
    // #228: the section's own element is STROKE-only (fill
    // participation joins the shape's one even-odd path — Ellipse rows
    // included).
    // LineCap: 0 round (Visio's default — round ends AND round
    // corners), 1 flat at the endpoint, 2 square-extended (Visio 2021
    // witness, b04: 12 pt lines and corners).
    // A shape-level cell: its formula evaluates in the INSTANCE env
    // first, also for inherited master geometry, and the cache (the
    // master's, for an inherited cell) is the fallback — the Rounding
    // rule (Greptile on PR #342).
    let cap_own_env;
    let cap_env: &vsdx::formula::Env = if inherited_frame {
        cap_own_env = shape_env(doc, s, chain);
        &cap_own_env
    } else {
        env
    };
    let line_cap = doc
        .resolve_cell(s, chain, "LineCap")
        .and_then(|c| {
            c.f.as_deref()
                .filter(|f| *f != "Inh" && *f != "No Formula")
                .and_then(|f| vsdx::formula::eval(f, cap_env).ok())
                .and_then(|v| v.as_f64())
                .or_else(|| cell_num(&c, cap_env))
        })
        .map_or(0, |v| v.round() as i64);
    let stroke_a = stroke_attr(
        no_line,
        &stroke_paint,
        line_weight,
        dash.as_deref(),
        line_alpha,
    ) + cap_attr(line_cap, dash.is_some()).as_str();

    // Master-space (unscaled) path state: geometry rows are authored in the
    // master's local units and scaled to the instance afterwards.
    let mut d = String::new();
    let mut cur = (0.0f64, 0.0f64); // master-space current point
    let sc = |p: (f64, f64)| -> (f64, f64) { (p.0 * scale.0, p.1 * scale.1) };
    // Emit a path coordinate pair with the local Y flip (Visio Y-up → SVG Y-down).
    let emit_pt = |p: (f64, f64)| -> String {
        let p = sc(p);
        format!("{:.5} {:.5} ", p.0, -p.1)
    };
    // #188: the spline-family accumulator — a SplineStart row opens a
    // spline, SplineKnot rows extend it, and any other row (or the
    // section's end) flushes the evaluated curve. 16 samples per knot
    // span: smooth at print scale, and the hit walk flattens with the
    // same constant so hit-testing agrees with the ink.
    let mut spline_acc: Option<crate::curve::SplineAcc> = None;
    macro_rules! flush_spline {
        () => {
            if let Some(acc) = spline_acc.take() {
                if let Some(data) = acc.flush() {
                    let pts = data.flatten(16);
                    for p in pts.iter().skip(1) {
                        let _ = write!(d, "L {}", emit_pt(*p));
                    }
                    if let Some(last) = pts.last() {
                        cur = *last;
                    }
                }
            }
        };
    }

    // The section-end flush: no further rows follow, so the walk's
    // current point is not updated (that assignment would be dead).
    macro_rules! flush_spline_tail {
        () => {
            if let Some(acc) = spline_acc.take() {
                if let Some(data) = acc.flush() {
                    let pts = data.flatten(16);
                    for p in pts.iter().skip(1) {
                        let _ = write!(d, "L {}", emit_pt(*p));
                    }
                }
            }
        };
    }

    // Pre-evaluate every typed row's endpoint ONCE, in row order (the
    // env feeds keep their order — #249), so the stored-jump
    // classification can see the route's neighboring runs (#294
    // review).
    // Control cells (A/B/C/D) evaluate at the row's ORIGINAL point in
    // the ordered walk — each row sees exactly the feeds of the rows
    // before it, never a later row's evaluated coordinate (#294
    // review; #249 covers the back-reference case the feeds exist
    // for).
    // A walked row: the row, its type, the evaluated endpoint, and the
    // control cells captured at the row's own point in the walk.
    type EvaldRow<'a> = (&'a vsdx::Row, &'a str, (f64, f64), [Option<f64>; 4]);
    let evald: Vec<EvaldRow<'_>> = rows
        .iter()
        .filter(|r| r.t.is_some() && !r.is_deleted())
        .map(|row| {
            let t = row.t.as_deref().unwrap_or("");
            let (mx, my) = (
                row_num(env, row, "X").unwrap_or(0.0),
                row_num(env, row, "Y").unwrap_or(0.0),
            );
            let (x, y) = rel_adjust(t, mx, my, width, height);
            // Feed the evaluated coordinates back into the env (#249):
            // later rows reference earlier ones section-qualified
            // (`Geometry1.X2`), and the stale cached V would keep the
            // whole chain at the authored pose.
            env.insert(format!("{}.X{}", gname, row.ix), x);
            env.insert(format!("{}.Y{}", gname, row.ix), y);
            let ctrl = ["A", "B", "C", "D"].map(|n| row_num(env, row, n));
            (row, t, (x, y), ctrl)
        })
        .collect();
    for (ri, (row, t, xy, ctrl)) in evald.iter().enumerate() {
        let (x, y) = *xy;
        // Any row type except SplineKnot ends an open spline (#188,
        // #185 review): flush the accumulated curve BEFORE the row's
        // own command so the path keeps its drawn order — a LineTo
        // after a knot used to land in `d` first, jumping the pen
        // back when the spline finally flushed.
        if *t != "SplineKnot" {
            flush_spline!();
        }
        match *t {
            "MoveTo" | "RelMoveTo" => {
                let _ = write!(d, "M {}", emit_pt((x, y)));
                cur = (x, y);
            }
            "LineTo" | "RelLineTo" => {
                let _ = write!(d, "L {}", emit_pt((x, y)));
                cur = (x, y);
            }
            "ArcTo" => {
                // Circular arc: |A| = sagitta (height of arc above the
                // chord). Positive A bulges RIGHT of the travel direction —
                // verified against the Process master's rounded rect, which
                // traverses CCW (y-up, interior on the left) with positive-A
                // corner arcs bulging outward.
                let h = ctrl[0].unwrap_or(0.0);
                let (sx, sy) = cur;
                let chord = ((x - sx).powi(2) + (y - sy).powi(2)).sqrt();
                // Route context (#294 review): the run continues
                // collinear INTO the arc and OUT of it — Visio writes
                // jumps mid-run. Only straight LineTo neighbors count
                // (curved endpoint chords can lie about the route),
                // and zero-length degenerate rows are skipped without
                // crossing a MoveTo; an isolated authored semicircle
                // stays a curve.
                let kind_of = |tt: &str| -> u8 {
                    if tt == "MoveTo" || tt == "RelMoveTo" {
                        0
                    } else if tt == "LineTo" || tt == "RelLineTo" {
                        1
                    } else {
                        2
                    }
                };
                let mut prev_dir = None;
                let mut pi = ri as isize - 1;
                while pi >= 1 {
                    let (_, pt, pp, _) = evald[pi as usize];
                    if kind_of(pt) != 1 {
                        break;
                    }
                    let (_, _, p0, _) = evald[pi as usize - 1];
                    let pd = (pp.0 - p0.0, pp.1 - p0.1);
                    if pd.0.hypot(pd.1) > 1e-9 {
                        prev_dir = Some(pd);
                        break;
                    }
                    pi -= 1;
                }
                let mut next_dir = None;
                let mut ni = ri + 1;
                while ni < evald.len() {
                    let (_, nt, ne, _) = evald[ni];
                    if kind_of(nt) != 1 {
                        break;
                    }
                    let p = if ni == ri + 1 {
                        (x, y)
                    } else {
                        evald[ni - 1].2
                    };
                    let nd = (ne.0 - p.0, ne.1 - p.1);
                    if nd.0.hypot(nd.1) > 1e-9 {
                        next_dir = Some(nd);
                        break;
                    }
                    ni += 1;
                }
                // The radius needs no frame conversion: every geometry
                // frame is INCHES (master-local, instance-local, page —
                // the frame scales are geometric RESCALES applied after
                // authoring), and the stored arc's A was authored at
                // the page's radius.
                let radius_in_frame = jump_radius;
                // PR #299 review: BOTH radii gate the strip — the
                // classifier itself selects the arc's axis radius, so a
                // zero on one axis only kills that axis's strips.
                if (jump_radius.0 > 0.0 || jump_radius.1 > 0.0)
                    // Jump CANDIDACY gates the kind (#324 phase 2): a
                    // stored arc straightens only on a line the crossing
                    // set would hop — an authored arc on a plain glued
                    // 1D line (`ObjType 0`) is artwork, and artwork arcs
                    // never strip even with a resolved radius (#294
                    // review).
                    && crate::geom::is_jump_candidate(doc, chain, s)
                    && crate::geom::is_stored_jump_arc(
                        h,
                        (x - sx, y - sy),
                        prev_dir,
                        next_dir,
                        radius_in_frame.0,
                        radius_in_frame.1,
                    )
                {
                    // #294: a stored jump arc straightens — the ink
                    // follows the run, and the jump overlay draws the
                    // ONE arc at the current crossing with the page's
                    // own radius. A stored arc stops lingering after a
                    // move.
                    let _ = write!(d, "L {}", emit_pt((x, y)));
                } else if h.abs() < 1e-9 || chord < 1e-9 {
                    let _ = write!(d, "L {}", emit_pt((x, y)));
                } else {
                    // On-arc point from the sagitta.
                    let mid = ((sx + x) / 2.0, (sy + y) / 2.0);
                    let dir = ((x - sx) / chord, (y - sy) / chord);
                    let right = (dir.1, -dir.0); // rot90 CW: right of travel
                    let m = (mid.0 + h * right.0, mid.1 + h * right.1);
                    let radius = (chord.powi(2) + 4.0 * h * h) / (8.0 * h.abs());
                    let large = h.abs() > chord / 2.0;
                    // cross > 0 (y-up frame) = CCW; path frame is mirrored → sweep 0.
                    let cross = (m.0 - sx) * (y - m.1) - (m.1 - sy) * (x - m.0);
                    let sweep = if cross > 0.0 { 0 } else { 1 };
                    let r = sc((radius, radius)).0.max(1e-6);
                    let e = emit_pt((x, y));
                    let _ = write!(
                        d,
                        "A {r:.5} {r:.5} 0 {la} {sw} {e}",
                        la = large as u8,
                        sw = sweep,
                        e = e.trim_end()
                    );
                }
                cur = (x, y);
            }
            "EllipticalArcTo" | "RelEllipticalArcTo" => {
                // (Authored unit codes for future writers: D carries
                // unit 32, C the angle unit 81 — MisterDiagram #233
                // rider; we only read today, preserve-bytes covers
                // round-trips.)
                let m = (
                    ctrl[0].unwrap_or((cur.0 + x) / 2.0),
                    ctrl[1].unwrap_or((cur.1 + y) / 2.0),
                );
                let c_ang = ctrl[2].unwrap_or(0.0);
                let d_cell = ctrl[3];
                // #233: an authored D=0 is a ZERO-WIDTH arc — the
                // minor axis collapses onto the major, so the arc IS
                // the endpoint chord. Visio draws the chord even when
                // the A/B control sits off it (MisterDiagram M5.50);
                // the old fallback's quadratic-through-m doubled back.
                // Corpus-common: 887 masters / 18,860 visible rows.
                if d_cell.is_some_and(|d| d.abs() < 1e-9) {
                    let _ = write!(d, "L {}", emit_pt((x, y)));
                    cur = (x, y);
                    continue;
                }
                // D is the ratio of the axis ALONG C to the axis across
                // it ([MS-VSDX] EllipticalArcTo; Visio 2021 witness t14:
                // through (0,.5)-(1,1)-(2,.5) in a 2×1 box, D=2 draws the
                // flat half-ellipse the box encloses, D=0.5 a tall one).
                // The solver takes across/along.
                let ratio = 1.0 / d_cell.unwrap_or(1.0).abs().max(1e-6);
                // Sane bounds: the arc must fit (loosely) inside the
                // shape's scaled extent — a solved ellipse or on-arc point
                // far outside means the numeric solver diverged on a tiny
                // or near-degenerate arc; fall back to the quadratic.
                let (ex, ey) = (x * scale.0, y * scale.1);
                let (mx, my) = (m.0 * scale.0, m.1 * scale.1);
                let bound_x = 3.0 * (width * scale.0).max(1e-6);
                let bound_y = 3.0 * (height * scale.1).max(1e-6);
                let in_bounds = |v: f64, b: f64| v.abs() <= b;
                match solve_ellipse_arc(cur, m, (x, y), c_ang, ratio, scale) {
                    Some((rx, ry, rot_deg, large, sweep))
                        if rx.max(ry)
                            <= 20.0
                                * ((x - cur.0).powi(2) + (y - cur.1).powi(2)).sqrt().max(1e-9)
                            && in_bounds(ex, bound_x)
                            && in_bounds(ey, bound_y)
                            && in_bounds(mx, bound_x)
                            && in_bounds(my, bound_y)
                            && rx.max(ry) <= bound_x.max(bound_y) =>
                    {
                        let e = emit_pt((x, y));
                        let _ = write!(
                            d,
                            "A {rx:.5} {ry:.5} {rot_deg:.4} {la} {sw} {e}",
                            la = large as u8,
                            sw = sweep,
                            e = e.trim_end()
                        );
                    }
                    _ => {
                        // The solver diverged on a genuinely 2-D arc
                        // (tiny or wild): quadratic Bézier through m.
                        // Authored zero-width (D=0) never reaches here
                        // — it short-circuits to the chord (#233).
                        let q = emit_pt(m);
                        let e = emit_pt((x, y));
                        let _ = write!(d, "Q {} {}", q.trim_end(), e.trim_end());
                    }
                }
                cur = (x, y);
            }
            "Ellipse" => {
                // (X,Y) center, (A,B) major-axis endpoint, (C,D) minor-axis endpoint.
                let a = ctrl[0].unwrap_or(x + 1.0);
                let b = ctrl[1].unwrap_or(y);
                let c2 = ctrl[2].unwrap_or(x);
                let d2 = ctrl[3].unwrap_or(y + 1.0);
                let rx = ((a - x).powi(2) + (b - y).powi(2)).sqrt().max(1e-6);
                let ry = ((c2 - x).powi(2) + (d2 - y).powi(2)).sqrt().max(1e-6);
                let ang = (b - y).atan2(a - x); // major-axis angle, y-up frame
                                                // Anisotropic instance scaling of the ellipse.
                let (rx2, ry2, ang2) =
                    transform_ellipse(rx.max(ry), (rx.min(ry)) / rx.max(ry), ang, scale);
                let (rx2, ry2) = if rx >= ry { (rx2, ry2) } else { (ry2, rx2) };
                let p = sc((x, y));
                // A full-ellipse row is one closed subpath (two half
                // arcs) of the section's path, so it joins the shape's
                // even-odd fill like any other closed contour — an
                // ellipse section inside a rectangle section punches a
                // hole, two concentric ellipse sections make a ring
                // (Visio 2021 witness, t07; the "no entry" signs of the
                // network stencils). The stroke stays per section.
                let (cx, cy) = (p.0, -p.1);
                let (rx, ry) = (rx2.max(1e-6), ry2.max(1e-6));
                let rot = -ang2; // mirror frame
                let (ex, ey) = (rx * rot.cos(), rx * rot.sin());
                let deg = rot.to_degrees();
                let _ = write!(
                    d,
                    "M {:.5} {:.5} A {rx:.5} {ry:.5} {deg:.4} 1 0 {:.5} {:.5} A {rx:.5} {ry:.5} {deg:.4} 1 0 {:.5} {:.5} Z ",
                    cx + ex,
                    cy + ey,
                    cx - ex,
                    cy - ey,
                    cx + ex,
                    cy + ey,
                );
                cur = (x, y);
            }
            "CubicBezTo" | "RelCubBezTo" => {
                // Controls (A,B) and (C,D); end (X,Y) — all possibly
                // relative fractions of Width/Height.
                let (a, b) = rel_adjust(
                    t,
                    ctrl[0].unwrap_or(cur.0),
                    ctrl[1].unwrap_or(cur.1),
                    width,
                    height,
                );
                let (c2, d2) =
                    rel_adjust(t, ctrl[2].unwrap_or(x), ctrl[3].unwrap_or(y), width, height);
                let _ = write!(
                    d,
                    "C {}{}{}",
                    emit_pt((a, b)),
                    emit_pt((c2, d2)),
                    emit_pt((x, y)).trim_end()
                );
                cur = (x, y);
            }
            "QuadBezTo" | "RelQuadBezTo" => {
                let (a, b) = rel_adjust(
                    t,
                    ctrl[0].unwrap_or((cur.0 + x) / 2.0),
                    ctrl[1].unwrap_or((cur.1 + y) / 2.0),
                    width,
                    height,
                );
                let _ = write!(
                    d,
                    "Q {} {}",
                    emit_pt((a, b)).trim_end(),
                    emit_pt((x, y)).trim_end()
                );
                cur = (x, y);
            }
            "SplineStart" => {
                // #188: a SplineStart opens a spline (§2.4.2.15): X/Y
                // is the SECOND control point (the previous row's end
                // is the first); A/B/C/D are second/first/last knots
                // and the degree. Any non-SplineKnot row flushes it
                // (the loop-top flush).
                spline_acc = Some(crate::curve::SplineAcc::start(
                    cur,
                    (x, y),
                    ctrl[0].unwrap_or(0.0),
                    ctrl[1].unwrap_or(0.0),
                    ctrl[2].unwrap_or(1.0),
                    ctrl[3].unwrap_or(3.0),
                ));
                cur = (x, y);
            }
            "SplineKnot" => {
                match spline_acc.as_mut() {
                    Some(acc) => {
                        acc.knot((x, y), ctrl[0].unwrap_or(0.0));
                        cur = (x, y);
                    }
                    None => {
                        // A knot row without an opener degenerates to
                        // the chord (the v0.2 behavior).
                        let _ = write!(d, "L {}", emit_pt((x, y)));
                        cur = (x, y);
                    }
                }
            }
            "NURBSTo" => {
                // #188: one self-contained rational B-spline row
                // (§2.4.2.7). The E cell's NURBS() formula carries the
                // interior points/knots/weights; the row cells frame
                // them (C/D first knot/weight, A/B second-to-last
                // knot/last weight, X/Y the endpoint). The loop-top
                // flush already ended any open spline.
                let e_text = row
                    .cell("E")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .unwrap_or_default();
                let mut drew = false;
                if let Some(f) = crate::curve::parse_nurbs(&e_text) {
                    // Fractions are of the GEOMETRY frame's own size:
                    // the master's dims in the master frame (`width/
                    // scale` recovers them; `emit_pt` applies the
                    // instance scale exactly once), the instance's
                    // own dims inline (scale = 1). Scaling by the
                    // instance dims here too would square the stretch
                    // (#218 review round).
                    let fx = if f.x_type == 0.0 {
                        frame_dim(width, scale.0)
                    } else {
                        1.0
                    };
                    let fy = if f.y_type == 0.0 {
                        frame_dim(height, scale.1)
                    } else {
                        1.0
                    };
                    let mut points = vec![cur];
                    let mut knots = vec![ctrl[2].unwrap_or(0.0)];
                    let mut weights = vec![ctrl[3].unwrap_or(1.0)];
                    for (gx, gy, gk, gw) in &f.points {
                        points.push((gx * fx, gy * fy));
                        knots.push(*gk);
                        weights.push(*gw);
                    }
                    knots.push(ctrl[0].unwrap_or(0.0));
                    knots.push(f.last_knot);
                    weights.push(ctrl[1].unwrap_or(1.0));
                    points.push((x, y));
                    let built = crate::curve::SplineData {
                        degree: f.degree,
                        points,
                        weights,
                        knots,
                    }
                    .sanitized();
                    if let Some(data) = built {
                        let pts = data.flatten(16);
                        for p in pts.iter().skip(1) {
                            let _ = write!(d, "L {}", emit_pt(*p));
                        }
                        if let Some(last) = pts.last() {
                            cur = *last;
                        }
                        drew = true;
                    }
                }
                if !drew {
                    let _ = write!(d, "L {}", emit_pt((x, y)));
                    cur = (x, y);
                }
            }
            "PolylineTo" => {
                // The A cell's POLYLINE() formula carries the interior
                // vertices; fractions are of the geometry frame's own
                // size (master dims in the master frame — the same
                // per-axis rule as NURBSTo above), and the row's X/Y
                // is the final vertex. Unparseable text keeps the
                // straight chord.
                let a_text = row
                    .cell("A")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .unwrap_or_default();
                // Repeated points (a vertex on the previous point or on
                // the endpoint) would leave zero-length segments, and
                // `path_frames` takes no tangent from those: an open
                // connector lost its arrowheads (CodeRabbit on PR #329).
                let same = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) < 1e-9;
                let mut last = cur;
                if let Some((xt, yt, pts)) = crate::curve::parse_polyline(&a_text) {
                    let fx = if xt == 0.0 {
                        frame_dim(width, scale.0)
                    } else {
                        1.0
                    };
                    let fy = if yt == 0.0 {
                        frame_dim(height, scale.1)
                    } else {
                        1.0
                    };
                    for (px, py) in pts {
                        let v = (px * fx, py * fy);
                        if same(v, last) {
                            continue;
                        }
                        let _ = write!(d, "L {}", emit_pt(v));
                        last = v;
                    }
                }
                // Any number of trailing endpoint copies collapse into
                // the endpoint (Greptile on PR #329).
                if !same(last, (x, y)) {
                    let _ = write!(d, "L {}", emit_pt((x, y)));
                }
                cur = (x, y);
            }
            "InfiniteLine" => {
                // v0.2: approximate infinite lines with a chord.
                let _ = write!(d, "L {}", emit_pt((x, y)));
                cur = (x, y);
            }
            _ => {}
        }
    }
    // A spline open through the section's end flushes here.
    flush_spline_tail!();
    // Rounding (Line Format): Visio rounds every corner of a straight
    // polyline walk with this radius — rounded boxes, pill-shaped
    // container headings, connector bends (Visio 2021 witness, t06).
    // `Rounding` is a shape-level cell, so a formula evaluates in the
    // INSTANCE env even when the rows come from the master: the path is
    // already in instance units (Greptile on PR #344). Rows that sample
    // a curve into `L` segments (NURBS, splines) are curves, not
    // corners, and stay as drawn.
    let own_env;
    let rounding_env: &vsdx::formula::Env = if inherited_frame {
        own_env = shape_env(doc, s, chain);
        &own_env
    } else {
        env
    };
    // Deleted rows draw nothing, so they never block the rounding
    // (Greptile on PR #344).
    let sampled_curve = rows.iter().any(|r| {
        !r.is_deleted()
            && matches!(
                r.t.as_deref(),
                Some("NURBSTo" | "SplineStart" | "SplineKnot")
            )
    });
    // Formula first: an inherited cell's cached value is the MASTER's
    // (`Width/10` cached as 0.1 on a 1 in master), so the instance env
    // decides and the cache is only the fallback (Greptile on PR #344).
    let rounding = doc
        .resolve_cell(s, chain, "Rounding")
        .and_then(|c| {
            c.f.as_deref()
                .filter(|f| *f != "Inh" && *f != "No Formula")
                .and_then(|f| vsdx::formula::eval(f, rounding_env).ok())
                .and_then(|v| v.as_f64())
                .or_else(|| cell_num(&c, rounding_env))
        })
        .filter(|r| r.is_finite() && *r > 0.0 && !sampled_curve);
    if let Some(r) = rounding {
        if let Some(rounded) = round_corners(&d, r) {
            d = rounded;
        }
    }
    if !d.is_empty() {
        // Connector arrowheads: BeginArrow/EndArrow cells (0 = none, ≥1 an
        // arrow; sizes 0–6, 2 = medium).
        let begin_arrow = doc
            .resolve_cell(s, chain, "BeginArrow")
            .and_then(|c| cell_num(&c, env))
            .unwrap_or(0.0);
        let end_arrow = doc
            .resolve_cell(s, chain, "EndArrow")
            .and_then(|c| cell_num(&c, env))
            .unwrap_or(0.0);
        let bsz = doc
            .resolve_cell(s, chain, "BeginArrowSize")
            .and_then(|c| cell_num(&c, env))
            .unwrap_or(2.0);
        let esz = doc
            .resolve_cell(s, chain, "EndArrowSize")
            .and_then(|c| cell_num(&c, env))
            .unwrap_or(2.0);
        // #185/#170: physical per-value arrow geometry. The head unit is
        // LineWeight + C[EndArrowSize] (C = [1.44, 1.8, 2.52, 3.24, 3.96,
        // 9, 18] pt — witnessed across all 45 types and sizes 0–6), each
        // gallery kind draws at its measured proportions in that unit
        // ([`GALLERY`]), and the shaft trims back by the per-kind
        // setback ([`arrow_setback`]) so hollow heads keep their
        // interiors clear — the open diamonds trim two pens short of
        // their rear vertex ([`diamond_shaft_trim`], #239). Evidence:
        // the owner-authored 600 dpi print (Sample/test files/
        // arrows.vsdx + arrows.png — the authoritative oracle)
        // cross-checked against the #223 parity witnesses; see
        // Analysis/arrow-gallery-visio2016.md.
        let kind = |a: f64| -> Option<i64> {
            if a < 0.5 || (46.0..=50.5).contains(&a) {
                None
            } else {
                Some(a.round() as i64)
            }
        };
        // Zero-weight exception (the SVG rule): the head draws with the
        // 0.75 pt nominal pen, so its unit is C[size] + 0.75 pt.
        let head_weight = if line_weight > 0.0 {
            line_weight
        } else {
            0.75 / 72.0
        };
        let head_unit = |sz: f64| -> f64 {
            let i = (sz.round() as i64).clamp(0, 6) as usize;
            head_weight + ARROW_SIZE_PT[i] / 72.0
        };
        let end_setback = kind(end_arrow).map_or(0.0, |k| {
            diamond_shaft_trim(k, arrow_setback(k) * head_unit(esz), head_weight)
        });
        let begin_setback = kind(begin_arrow).map_or(0.0, |k| {
            arrow_begin_setback(diamond_shaft_trim(
                k,
                arrow_setback(k) * head_unit(bsz),
                head_weight,
            ))
        });
        // The shaft stops at (or under) the head's rear; the heads
        // themselves anchor at the ORIGINAL endpoints, so the trim must
        // not move them. Closed loops and multi-subpath data trim
        // never (Visio gives them no heads either — #227 tracks that).
        let shaft = if end_setback > 0.0 || begin_setback > 0.0 {
            trim_path(&d, begin_setback, end_setback)
        } else {
            d.clone()
        };
        // #228: a fill-eligible section's `d` (UNtrimmed — setbacks
        // trim the shaft only) joins the shape's ONE alternate-fill
        // path; NoFill sections opt out here, NoLine changes nothing
        // for the fill, and the implicit close SVG already applies to
        // open subpaths leaves the stroke open per section.
        // 1-D shapes fill their closed subpaths only (see `one_d`
        // above); closure is judged per subpath, not per section
        // (Greptile on PR #328).
        let fill_part = if one_d {
            closed_subpaths(&d)
        } else {
            d.clone()
        };
        if !no_fill && fill != "none" && !fill_part.trim().is_empty() {
            paint.fill_d.push_str(&fill_part);
            paint.fill_d.push(' ');
            paint.fill = fill.clone();
            paint.fill_opacity = fill_alpha;
            paint.fill_tiles = match &fill_gradient {
                Some(GradientPaint::Tiles(t)) => Some(t.clone()),
                _ => hatch_tiles.clone(),
            };
        }
        if !no_line {
            let _ = writeln!(
                paint.strokes,
                r#"<path d="{}" fill="none" {stroke_a}/>"#,
                escape_attr(close_returning_path(shaft.trim()).trim())
            );
        }
        // Heads as inline translate·rotate·scale groups of the
        // unit-space parts (begin faces outward along the reversed
        // start tangent, end along the end tangent) — no marker defs,
        // so the size law holds without markerUnits scaling. #227: a
        // CLOSED walk gets no frames — and so no heads — at all. A
        // fully transparent line (LineColorTrans = 1) hides its heads
        // with the shaft (Greptile on PR #327).
        if let Some((begin, end)) = path_frames(&d).filter(|_| line_alpha > 0.0) {
            // LineColorTrans fades the heads with the shaft (the head
            // parts set no opacity of their own, so the group's
            // inherited fill-/stroke-opacity applies).
            let faded = line_alpha < 1.0
                && !no_line
                && (kind(begin_arrow).is_some() || kind(end_arrow).is_some());
            if faded {
                let _ = writeln!(
                    paint.strokes,
                    r#"<g fill-opacity="{line_alpha:.3}" stroke-opacity="{line_alpha:.3}">"#
                );
            }
            if let Some(k) = kind(begin_arrow) {
                emit_arrow_head(
                    &mut paint.strokes,
                    k,
                    bsz,
                    head_weight,
                    &line_color,
                    begin.0,
                    begin.1,
                );
            }
            if let Some(k) = kind(end_arrow) {
                emit_arrow_head(
                    &mut paint.strokes,
                    k,
                    esz,
                    head_weight,
                    &line_color,
                    end.0,
                    end.1,
                );
            }
            if faded {
                paint.strokes.push_str("</g>\n");
            }
        }
    }
}

/// One drawable piece of an arrow head (#199, #185): a path — filled
/// when `filled`, stroked (fill="none") otherwise — or a circle
/// (filled dot vs open ring). Composite heads (the dimension set's
/// tick + glyph rows) are just several parts in one head.
/// Coordinates live in MARKER UNITS with the tip at the origin, +x
/// pointing PAST the tip (the body extends into -x) and +y down; the
/// head frame is placed with a translate·rotate·scale group, so the
/// same table serves both ends without mirroring.
pub(crate) struct HeadPart {
    pub(crate) d: &'static str,
    pub(crate) circle: Option<(f64, f64, f64)>,
    pub(crate) filled: bool,
}

/// A filled path part.
const fn fp(d: &'static str) -> HeadPart {
    HeadPart {
        d,
        circle: None,
        filled: true,
    }
}

/// A stroked (open) path part.
const fn sp(d: &'static str) -> HeadPart {
    HeadPart {
        d,
        circle: None,
        filled: false,
    }
}

/// An open ring (stroke-only circle).
const fn ring(cx: f64, cy: f64, r: f64) -> HeadPart {
    HeadPart {
        d: "",
        circle: Some((cx, cy, r)),
        filled: false,
    }
}

/// A filled dot (circle).
const fn dot(cx: f64, cy: f64, r: f64) -> HeadPart {
    HeadPart {
        d: "",
        circle: Some((cx, cy, r)),
        filled: true,
    }
}

/// The Visio 2016 Begin/EndArrow gallery in unit space (#185): every
/// kind at its measured proportions in the head unit
/// LineWeight + C[EndArrowSize]. Pinned against the owner-authored
/// 600 dpi print (`Sample/test files/arrows.png` — the fixture
/// `arrows.vsdx` carries EndArrow = n per labeled line; the print is
/// the authoritative oracle) and cross-checked against the #223
/// parity witnesses; artwork ours, no Microsoft assets.
///
/// Geometry conventions: the tip sits at the origin, the body
/// extends into -x, +y is down. Per-kind proportions are the
/// measured ones — triangles 2 units tall with per-kind lengths
/// (2/15: 1, 3/4/16: 2, 12/13/14: 3), ticks/bars 1.5 tall, circles
/// r 0.75 (the 35–37 dots 0.825), the diamond 4.5 × 2.25 — and the
/// 5/6/8/18 rears are circular arcs (concave for 5/8, convex —
/// centered at the tip, R = √5 — for 6/18) emitted as exact cubic
/// approximations (k = 4/3·(√5−2)). Corrections the print forced
/// against the earlier table: 12 is ONE wide open V (len 3) and 43
/// is the double chevron (len 4); 10/11/20/21 straddle the tip
/// centered; 9's tick leans "/" through the tip while 23's leans
/// "\" behind it.
const GALLERY: &[(i64, &[HeadPart])] = &[
    // 1 — narrow open V (unfilled 2-prong), len 1.
    (1, &[sp("M-1,-1 L0,0 L-1,1")]),
    // 2 — small filled triangle, len 1.
    (2, &[fp("M0,0 L-1,-1 L-1,1 Z")]),
    // 3 — open wide V, len 2.
    (3, &[sp("M-2,-1 L0,0 L-2,1")]),
    // 4 — the classic filled triangle (the corpus default), len 2.
    (4, &[fp("M0,0 L-2,-1 L-2,1 Z")]),
    // 5 — filled triangle, concave circular rear (the stealth-dart
    // family; the native SVG alias 8→5 and the print agree).
    (
        5,
        &[fp("M0,0 L-2,-1 C-1.68524,-0.37049 -1.68524,0.37049 -2,1 Z")],
    ),
    // 6 — filled triangle with a convex rear arc centered at the
    // tip: reads "long" next to 4's flat base.
    (
        6,
        &[fp("M0,0 L-2,-1 C-2.31476,-0.37049 -2.31476,0.37049 -2,1 Z")],
    ),
    // 7 — open fork (curved barbs).
    (
        7,
        &[sp("M-2,-1 C-1.5,-0.3 -0.7,0 0,0 C-0.7,0 -1.5,0.3 -2,1")],
    ),
    // 8 — the stealth dart: the concave-rear dart (the 5 glyph).
    (
        8,
        &[fp("M0,0 L-2,-1 C-1.68524,-0.37049 -1.68524,0.37049 -2,1 Z")],
    ),
    // 9 — the slanted dimension tick: 45° through the tip ("/").
    (9, &[sp("M1,-1 L-1,1")]),
    // 10 — the filled circle, centered ON the endpoint.
    (10, &[dot(0.0, 0.0, 0.75)]),
    // 11 — the filled square, centered ON the endpoint.
    (
        11,
        &[fp("M-0.75,-0.75 L0.75,-0.75 L0.75,0.75 L-0.75,0.75 Z")],
    ),
    // 12 — ONE wide open V (len 3; the old "double chevron" label
    // was a print misread).
    (12, &[sp("M-3,-1 L0,0 L-3,1")]),
    // 13 — single filled triangle, plain base, len 3 (NOT a double —
    // the #169 report).
    (13, &[fp("M0,0 L-3,-1 L-3,1 Z")]),
    // 14–18 — the open-head set: CLOSED triangle outlines whose
    // interiors stay hollow (the shaft trims at the base).
    (14, &[sp("M0,0 L-3,-1 L-3,1 Z")]),
    (15, &[sp("M0,0 L-1,-1 L-1,1 Z")]),
    (16, &[sp("M0,0 L-2,-1 L-2,1 Z")]),
    // 17 — open triangle, concave rear (cubic through ±0.33).
    (17, &[sp("M0,0 L-2,-1 C-1.55,-0.33 -1.55,0.33 -2,1 Z")]),
    // 18 — open triangle, convex rear arc (centered at the tip).
    (
        18,
        &[sp("M0,0 L-2,-1 C-2.31476,-0.37049 -2.31476,0.37049 -2,1 Z")],
    ),
    // 19 — open barbed head: curved barbs closed by a concave rear.
    (
        19,
        &[sp(
            "M-2,-1 C-1.5,-0.3 -0.7,0 0,0 C-0.7,0 -1.5,0.3 -2,1 C-1.55,0.3 -1.55,-0.3 -2,-1 Z",
        )],
    ),
    // 20 — the open ring, centered ON the endpoint.
    (20, &[ring(0.0, 0.0, 0.75)]),
    // 21 — the open square, centered ON the endpoint.
    (
        21,
        &[sp("M-0.75,-0.75 L0.75,-0.75 L0.75,0.75 L-0.75,0.75 Z")],
    ),
    // 22 — the open diamond, 4.5 × 2.25.
    (22, &[sp("M0,0 L-2.25,-1.125 L-4.5,0 L-2.25,1.125 Z")]),
    // 23 — single slanted tick: 45° behind the tip ("\"), shaft
    // through.
    (23, &[sp("M-2.9,-1 L-0.9,1")]),
    // 24–26 — perpendicular ticks at 1.5 + 0.75·i behind the tip.
    (24, &[sp("M-1.5,-0.75 L-1.5,0.75")]),
    (25, &[sp("M-1.5,-0.75 L-1.5,0.75 M-2.25,-0.75 L-2.25,0.75")]),
    (
        26,
        &[sp(
            "M-1.5,-0.75 L-1.5,0.75 M-2.25,-0.75 L-2.25,0.75 M-3,-0.75 L-3,0.75",
        )],
    ),
    // 27 — the three-pronged open fork (crow's foot).
    (27, &[sp("M0,-0.75 L-1.5,0 L0,0.75")]),
    // 28 — crow's-foot fork + perpendicular tick behind it.
    (
        28,
        &[sp("M0,-0.75 L-1.5,0 L0,0.75"), sp("M-2,-0.75 L-2,0.75")],
    ),
    // 29 — crow's-foot fork, then the open ring at 2.25.
    (
        29,
        &[sp("M0,-0.75 L-1.5,0 L0,0.75"), ring(-2.25, 0.0, 0.75)],
    ),
    // 30 — open ring at 2.25, then a tick toward the tip.
    (
        30,
        &[ring(-2.25, 0.0, 0.75), sp("M-0.75,-0.75 L-0.75,0.75")],
    ),
    // 31–33 — 1–3 ticks behind an open ring at the tip.
    (
        31,
        &[sp("M-2.25,-0.75 L-2.25,0.75"), ring(-0.75, 0.0, 0.75)],
    ),
    (
        32,
        &[
            sp("M-2.25,-0.75 L-2.25,0.75 M-3,-0.75 L-3,0.75"),
            ring(-0.75, 0.0, 0.75),
        ],
    ),
    (
        33,
        &[
            sp("M-2.25,-0.75 L-2.25,0.75 M-3,-0.75 L-3,0.75 M-3.75,-0.75 L-3.75,0.75"),
            ring(-0.75, 0.0, 0.75),
        ],
    ),
    // 34 — open diamond (1.5–6) + open ring at the tip.
    (
        34,
        &[
            ring(-0.75, 0.0, 0.75),
            sp("M-1.5,0 L-3.75,-1.125 L-6,0 L-3.75,1.125 Z"),
        ],
    ),
    // 35–37 — 1–3 ticks behind a filled 0.825-radius dot at the tip.
    (
        35,
        &[dot(-0.75, 0.0, 0.825), sp("M-2.25,-0.75 L-2.25,0.75")],
    ),
    (
        36,
        &[
            dot(-0.75, 0.0, 0.825),
            sp("M-2.25,-0.75 L-2.25,0.75 M-3,-0.75 L-3,0.75"),
        ],
    ),
    (
        37,
        &[
            dot(-0.75, 0.0, 0.825),
            sp("M-2.25,-0.75 L-2.25,0.75 M-3,-0.75 L-3,0.75 M-3.75,-0.75 L-3.75,0.75"),
        ],
    ),
    // 38 — OPEN diamond (1.5–6) + the filled dot. #239's fixture pass:
    // the 600 dpi print's center-line runs a gap through the diamond
    // interior (no fill between the apex and the shaft stub) — the
    // earlier "filled" gallery read was the low-res screenshot's.
    (
        38,
        &[
            dot(-0.75, 0.0, 0.825),
            sp("M-1.5,0 L-3.75,-1.125 L-6,0 L-3.75,1.125 Z"),
        ],
    ),
    // 39 — double filled triangle (two solid heads).
    (39, &[fp("M0,0 L-2,-1 L-2,1 Z M-2,0 L-4,-1 L-4,1 Z")]),
    // 40 — double open triangle (two hollow heads).
    (40, &[sp("M0,0 L-2,-1 L-2,1 Z M-2,0 L-4,-1 L-4,1 Z")]),
    // 41 — the open ring (front edge at the tip).
    (41, &[ring(-0.75, 0.0, 0.75)]),
    // 42 — the filled dot (front edge at the tip).
    (42, &[dot(-0.75, 0.0, 0.75)]),
    // 43 — the open double chevron (two 2-unit V's, len 4; each V is
    // corner→apex→corner so no base edge closes it).
    (43, &[sp("M-2,-1 L0,0 L-2,1 M-4,-1 L-2,0 L-4,1")]),
    // 44 — perpendicular tick behind a single open V.
    (44, &[sp("M-2,-1 L0,0 L-2,1"), sp("M-3,-0.75 L-3,0.75")]),
    // 45 — perpendicular tick behind the double chevron.
    (
        45,
        &[
            sp("M-2,-1 L0,0 L-2,1 M-4,-1 L-2,0 L-4,1"),
            sp("M-5,-0.75 L-5,0.75"),
        ],
    ),
];

/// The unmapped-value fallback: the classic filled triangle.
const FALLBACK_TRIANGLE: &[HeadPart] = &[fp("M0,0 L-2,-1 L-2,1 Z")];

/// The gallery lookup: `None` = no head (values 46–50 print bare
/// line ends); unmapped values fall back to the filled triangle.
pub(crate) fn arrow_spec(kind: i64) -> Option<&'static [HeadPart]> {
    if (46..=50).contains(&kind) {
        return None;
    }
    Some(
        GALLERY
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(FALLBACK_TRIANGLE, |(_, parts)| *parts),
    )
}

/// EndArrowSize 0–6, in points: the physical head unit is
/// LineWeight + C[size] — witnessed across all 45 head types and
/// sizes 0–6, independent of export scale, group resize, drawing
/// scale and dash pattern (#185).
const ARROW_SIZE_PT: [f64; 7] = [1.44, 1.8, 2.52, 3.24, 3.96, 9.0, 18.0];

/// The gallery's largest head reach in units (the 34/38 diamond's
/// rear vertex; kind 9 reaches 1 unit forward and the diamonds 1.125
/// sideways, both inside this square). The conservative bound the
/// ink-box walk pads around arrowed endpoints.
const ARROW_MAX_REACH_UNITS: f64 = 6.0;

/// The physical half-extent an arrowhead reaches beyond a line
/// endpoint: `reach × (LineWeight + C[size])` with the zero-weight
/// SVG floor — the same unit law [`emit_arrow_head`] draws with, so
/// the ink-box padding never clips a head.
pub(crate) fn arrow_head_reach(weight: f64, sz: f64) -> f64 {
    let head_weight = if weight > 0.0 { weight } else { 0.75 / 72.0 };
    let i = (sz.round() as i64).clamp(0, 6) as usize;
    ARROW_MAX_REACH_UNITS * (head_weight + ARROW_SIZE_PT[i] / 72.0)
}

/// Per-kind shaft setback in head units (#185): how far back from the
/// line end the shaft stops. Closed/hollow heads trim to their rear
/// (the measured values ARE the glyph rear extents); pass-through
/// heads — the open V/barbs (1, 3, 7), the ticks (9, 23–26), the
/// crow's-foot set (27/28), the chevrons (12, 43–45) — ride 0 and the
/// shaft runs beneath them to the tip. Filled heads trim under their
/// own fill, so the value only has to reach covered ground.
fn arrow_setback(kind: i64) -> f64 {
    match kind {
        2 | 15 => 1.0,
        4 | 16 => 2.0,
        5 | 8 | 17 => 1.75,
        6 | 18 => 2.25,
        10 | 11 | 20 | 21 | 42 => 0.75,
        13 | 14 | 29 | 30 => 3.0,
        19 => 1.65,
        22 => 4.5,
        31..=33 | 35..=37 | 41 => 1.5,
        34 | 38 => 6.0,
        39 | 40 => 4.0,
        _ => 0.0,
    }
}

/// The BEGIN head's setback: the end setback minus max(1% of it,
/// 0.36 pt) — Visio's begin end trims marginally less — floored at
/// zero (#185).
fn arrow_begin_setback(end_setback: f64) -> f64 {
    (end_setback - (end_setback * 0.01).max(0.36 / 72.0)).max(0.0)
}

/// #239: the open diamonds (22, 34/38) trim their shaft TWO PENS short
/// of the rear vertex, not at it. The 600 dpi fixture's ink shows the
/// stop where the shaft's ink would first touch the outline's ink —
/// the outline center one full pen off-axis, which the slope-½ rear
/// halves doubles: `rear − 2 × pen` (22's on-axis ink ends ≈3.79 u of
/// its 4.5 rear; 34/38's ≈5.45 of 6 — the model puts the geometric
/// stops at 3.77/5.27 and the butt-cap/AA residuals land inside
/// ±0.2 u; the flat-rear hollow heads keep the full extent because
/// their trim hides under the rear stroke). Floored at half the
/// setback — never in front of the diamond's widest point, so a fat
/// pen cannot push the stop into the wide half.
fn diamond_shaft_trim(kind: i64, setback: f64, head_weight: f64) -> f64 {
    if matches!(kind, 22 | 34 | 38) {
        (setback - 2.0 * head_weight).max(0.5 * setback)
    } else {
        setback
    }
}

fn path_command_arity(c: char) -> Option<usize> {
    match c {
        'M' | 'L' => Some(2),
        'Q' => Some(4),
        'C' => Some(6),
        'A' => Some(7),
        _ => None,
    }
}

/// Parse emitted path data into (letter, floats) commands; None when
/// anything deviates from the emitter's strict absolute M/L/Q/C/A
/// shape (the caller keeps the untrimmed path).
fn parse_path_commands(d: &str) -> Option<Vec<(char, Vec<f64>)>> {
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let mut cur: Option<(char, Vec<f64>)> = None;
    for tok in d.split_whitespace() {
        let mut chars = tok.chars();
        let first = chars.next().unwrap_or(' ');
        if chars.count() == 0 && first.is_ascii_alphabetic() {
            if let Some((c, f)) = cur.take() {
                if path_command_arity(c) != Some(f.len()) {
                    return None;
                }
                cmds.push((c, f));
            }
            cur = Some((first, Vec::new()));
        } else {
            cur.as_mut()?.1.push(tok.parse::<f64>().ok()?);
        }
    }
    if let Some((c, f)) = cur.take() {
        if path_command_arity(c) != Some(f.len()) {
            return None;
        }
        cmds.push((c, f));
    }
    Some(cmds)
}

/// One drawing segment: (letter, params, start, end). `M` resets the
/// current point without drawing.
type PathSeg = (char, Vec<f64>, (f64, f64), (f64, f64));

fn path_segments(cmds: &[(char, Vec<f64>)]) -> Option<Vec<PathSeg>> {
    let mut segs: Vec<PathSeg> = Vec::new();
    let mut cur: Option<(f64, f64)> = None;
    for (c, f) in cmds {
        match c {
            'M' => cur = Some((f[0], f[1])),
            'L' | 'Q' | 'C' | 'A' => {
                let s = cur?;
                let e = match c {
                    'Q' => (f[2], f[3]),
                    'C' => (f[4], f[5]),
                    'A' => (f[5], f[6]),
                    _ => (f[0], f[1]),
                };
                segs.push((*c, f.clone(), s, e));
                cur = Some(e);
            }
            _ => return None,
        }
    }
    Some(segs)
}

/// An SVG arc's center-form parameters: (rx, ry, φ rad, θ1, Δθ,
/// center). `s` is the arc's start point, `f` the full A parameters.
type ArcGeo = (f64, f64, f64, f64, f64, (f64, f64));

fn arc_geo(f: &[f64], s: (f64, f64)) -> Option<ArcGeo> {
    let (mut rx, mut ry) = (f[0], f[1]);
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let phi = f[2].to_radians();
    let (sp, cp) = phi.sin_cos();
    let e = (f[5], f[6]);
    let (dx2, dy2) = ((s.0 - e.0) / 2.0, (s.1 - e.1) / 2.0);
    let x1 = cp * dx2 + sp * dy2;
    let y1 = -sp * dx2 + cp * dy2;
    let lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lam > 1.0 {
        let r = lam.sqrt();
        rx *= r;
        ry *= r;
    }
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    if den <= 1e-18 {
        return None; // coincident endpoints
    }
    let num = rx * rx * ry * ry - den;
    let sign = if f[3] != f[4] { 1.0 } else { -1.0 };
    let co = sign * (num / den).max(0.0).sqrt();
    let (cxp, cyp) = (co * rx * y1 / ry, -co * ry * x1 / rx);
    let center = (
        cp * cxp - sp * cyp + (s.0 + e.0) / 2.0,
        sp * cxp + cp * cyp + (s.1 + e.1) / 2.0,
    );
    let th1 = ((y1 - cyp) / ry).atan2((x1 - cxp) / rx);
    let mut dth = ((-y1 - cyp) / ry).atan2((-x1 - cxp) / rx) - th1;
    if f[4] == 0.0 && dth > 0.0 {
        dth -= std::f64::consts::TAU;
    }
    if f[4] != 0.0 && dth < 0.0 {
        dth += std::f64::consts::TAU;
    }
    Some((rx, ry, phi, th1, dth, center))
}

/// The point and tangent direction at parameter t ∈ [0, 1] of an arc.
fn arc_point_tangent(g: &ArcGeo, t: f64) -> ((f64, f64), (f64, f64)) {
    let (rx, ry, phi, th1, dth, c) = *g;
    let th = th1 + t * dth;
    let (st, ct) = th.sin_cos();
    let (sp, cp) = phi.sin_cos();
    let p = (
        c.0 + cp * rx * ct - sp * ry * st,
        c.1 + sp * rx * ct + cp * ry * st,
    );
    let dir = (-cp * rx * st - sp * ry * ct, -sp * rx * st + cp * ry * ct);
    let dir = if dth < 0.0 { (-dir.0, -dir.1) } else { dir };
    (p, dir)
}

/// Chord length of a sample polyline.
pub(crate) fn polyline_length(samples: &[(f64, f64)]) -> f64 {
    samples
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum()
}

/// Polyline approximation of one segment (16 chords) for arc-length
/// walks.
pub(crate) fn segment_samples(c: char, f: &[f64], s: (f64, f64)) -> Vec<(f64, f64)> {
    match c {
        'L' => vec![s, (f[0], f[1])],
        'Q' => (0..=16)
            .map(|i| {
                let t = i as f64 / 16.0;
                let mt = 1.0 - t;
                (
                    mt * mt * s.0 + 2.0 * mt * t * f[0] + t * t * f[2],
                    mt * mt * s.1 + 2.0 * mt * t * f[1] + t * t * f[3],
                )
            })
            .collect(),
        'C' => (0..=16)
            .map(|i| {
                let t = i as f64 / 16.0;
                let mt = 1.0 - t;
                (
                    mt * mt * mt * s.0
                        + 3.0 * mt * mt * t * f[0]
                        + 3.0 * mt * t * t * f[2]
                        + t * t * t * f[4],
                    mt * mt * mt * s.1
                        + 3.0 * mt * mt * t * f[1]
                        + 3.0 * mt * t * t * f[3]
                        + t * t * t * f[5],
                )
            })
            .collect(),
        'A' => arc_geo(f, s)
            .map(|g| {
                (0..=16)
                    .map(|i| arc_point_tangent(&g, i as f64 / 16.0).0)
                    .collect()
            })
            .unwrap_or_else(|| vec![s, (f[5], f[6])]),
        _ => vec![s, s],
    }
}

/// Parameter t for the arc-length `target` along a segment's sample
/// polyline (exact for lines, sub-chord for curves).
pub(crate) fn sample_time_at_length(samples: &[(f64, f64)], target: f64) -> f64 {
    let n = samples.len().saturating_sub(1) as f64;
    if n <= 0.0 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..samples.len() - 1 {
        let (a, b) = (samples[i], samples[i + 1]);
        let l = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if l <= 0.0 {
            continue;
        }
        if acc + l >= target {
            let frac = ((target - acc) / l).clamp(0.0, 1.0);
            return (i as f64 + frac) / n;
        }
        acc += l;
    }
    1.0
}

pub(crate) fn quad_at(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), t: f64) -> (f64, f64) {
    let mt = 1.0 - t;
    (
        mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0,
        mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1,
    )
}

fn cubic_at(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), t: f64) -> (f64, f64) {
    let mt = 1.0 - t;
    (
        mt * mt * mt * p0.0 + 3.0 * mt * mt * t * p1.0 + 3.0 * mt * t * t * p2.0 + t * t * t * p3.0,
        mt * mt * mt * p0.1 + 3.0 * mt * mt * t * p1.1 + 3.0 * mt * t * t * p2.1 + t * t * t * p3.1,
    )
}

/// The point at parameter t on one segment (t=0 start, t=1 end).
fn segment_point(c: char, f: &[f64], s: (f64, f64), t: f64) -> (f64, f64) {
    match c {
        'Q' => quad_at(s, (f[0], f[1]), (f[2], f[3]), t),
        'C' => cubic_at(s, (f[0], f[1]), (f[2], f[3]), (f[4], f[5]), t),
        'A' => arc_geo(f, s).map_or(s, |g| arc_point_tangent(&g, t).0),
        _ => (s.0 + (f[0] - s.0) * t, s.1 + (f[1] - s.1) * t),
    }
}

/// Quadratic restricted to [t1, t2] → (start, control, end); the
/// control comes from B'(t1)·(t2−t1)/2.
fn quad_sub(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    t1: f64,
    t2: f64,
) -> ((f64, f64), (f64, f64), (f64, f64)) {
    let h = t2 - t1;
    let start = quad_at(p0, p1, p2, t1);
    let end = quad_at(p0, p1, p2, t2);
    let d = (
        (1.0 - t1) * (p1.0 - p0.0) + t1 * (p2.0 - p1.0),
        (1.0 - t1) * (p1.1 - p0.1) + t1 * (p2.1 - p1.1),
    );
    (start, (start.0 + d.0 * h, start.1 + d.1 * h), end)
}

/// Cubic restricted to [t1, t2] → (start, c1, c2, end); the handles
/// come from C'(t1)·(t2−t1)/3 and −C'(t2)·(t2−t1)/3.
type CubicSub = ((f64, f64), (f64, f64), (f64, f64), (f64, f64));

/// One head frame: an anchor point and its direction angle (radians).
type HeadFrame = ((f64, f64), f64);

/// Cubic restricted to [t1, t2] → (start, c1, c2, end); the handles
/// come from C'(t1)·(t2−t1)/3 and −C'(t2)·(t2−t1)/3.
fn cubic_sub(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t1: f64,
    t2: f64,
) -> CubicSub {
    let h = t2 - t1;
    let start = cubic_at(p0, p1, p2, p3, t1);
    let end = cubic_at(p0, p1, p2, p3, t2);
    let deriv = |t: f64| {
        let mt = 1.0 - t;
        (
            mt * mt * (p1.0 - p0.0) + 2.0 * mt * t * (p2.0 - p1.0) + t * t * (p3.0 - p2.0),
            mt * mt * (p1.1 - p0.1) + 2.0 * mt * t * (p2.1 - p1.1) + t * t * (p3.1 - p2.1),
        )
    };
    let d1 = deriv(t1);
    let d2 = deriv(t2);
    (
        start,
        (start.0 + d1.0 * h, start.1 + d1.1 * h),
        (end.0 - d2.0 * h, end.1 - d2.1 * h),
        end,
    )
}

/// Trim one open contour of an emitted path by physical lengths at
/// each end (#185): `begin` off the start, `end` off the end, in user
/// units. Only a single open contour trims — closed loops and
/// multi-subpath data come back untouched (Visio gives them no
/// arrowheads either), and a cut that would consume the whole path
/// keeps the original. Curves split exactly (Blossom
/// re-parameterization for Q/C, the angular subrange for A); the cut
/// parameter comes from a 16-sample chord walk, exact for lines.
pub(crate) fn trim_path(d: &str, begin: f64, end: f64) -> String {
    let keep = |d: &str| d.to_string();
    let Some(cmds) = parse_path_commands(d) else {
        return keep(d);
    };
    let Some(segs) = path_segments(&cmds) else {
        return keep(d);
    };
    if segs.is_empty() || cmds.iter().filter(|(c, _)| *c == 'M').count() != 1 {
        return keep(d);
    }
    let start = segs[0].2;
    let last_end = segs[segs.len() - 1].3;
    if ((last_end.0 - start.0).powi(2) + (last_end.1 - start.1).powi(2)).sqrt() < 1e-9 {
        return keep(d); // closed loop
    }
    let polys: Vec<Vec<(f64, f64)>> = segs
        .iter()
        .map(|(c, f, s, _)| segment_samples(*c, f, *s))
        .collect();
    let lens: Vec<f64> = polys.iter().map(|p| polyline_length(p)).collect();
    let total: f64 = lens.iter().sum();
    let cut_end = total - end;
    if cut_end <= begin + 1e-9 {
        return keep(d); // nothing remains
    }
    let fmt = |p: (f64, f64)| format!("{:.5} {:.5}", p.0, p.1);
    let mut out_d = String::new();
    let mut offset = 0.0;
    for (i, (c, f, s, _)) in segs.iter().enumerate() {
        let len = lens[i];
        let seg_start = offset;
        offset += len;
        let a = begin.max(seg_start);
        let b = cut_end.min(offset);
        if b <= a + 1e-12 {
            continue;
        }
        let poly = &polys[i];
        let t1 = sample_time_at_length(poly, a - seg_start);
        let t2 = sample_time_at_length(poly, b - seg_start);
        if out_d.is_empty() {
            out_d = format!("M {}", fmt(segment_point(*c, f, *s, t1)));
        }
        match c {
            'L' => {
                out_d += &format!(
                    " L {}",
                    fmt((s.0 + (f[0] - s.0) * t2, s.1 + (f[1] - s.1) * t2))
                );
            }
            'Q' => {
                let (_, p1, p2) = quad_sub(*s, (f[0], f[1]), (f[2], f[3]), t1, t2);
                out_d += &format!(" Q {} {}", fmt(p1), fmt(p2));
            }
            'C' => {
                let (_, p1, p2, p3) =
                    cubic_sub(*s, (f[0], f[1]), (f[2], f[3]), (f[4], f[5]), t1, t2);
                out_d += &format!(" C {} {} {}", fmt(p1), fmt(p2), fmt(p3));
            }
            'A' => {
                if let Some(g) = arc_geo(f, *s) {
                    let (p, _) = arc_point_tangent(&g, t2);
                    let large = ((t2 - t1) * g.4.abs() > std::f64::consts::PI) as u8;
                    out_d += &format!(
                        " A {:.5} {:.5} {:.4} {} {} {}",
                        g.0,
                        g.1,
                        f[2],
                        large,
                        f[4] as u8,
                        fmt(p)
                    );
                }
            }
            _ => {}
        }
    }
    if out_d.is_empty() {
        return keep(d);
    }
    out_d
}

/// A segment's start tangent (the true curve tangent where the
/// segment has one).
fn seg_start_tangent(c: char, f: &[f64], s: (f64, f64)) -> Option<(f64, f64)> {
    let d = match c {
        'A' => arc_geo(f, s).map(|g| arc_point_tangent(&g, 0.0).1)?,
        _ => (f[0] - s.0, f[1] - s.1),
    };
    (d.0 * d.0 + d.1 * d.1 > 1e-18).then_some(d)
}

/// A segment's end tangent.
fn seg_end_tangent(c: char, f: &[f64], s: (f64, f64)) -> Option<(f64, f64)> {
    let d = match c {
        'L' => (f[0] - s.0, f[1] - s.1),
        'Q' => (f[2] - f[0], f[3] - f[1]),
        'C' => (f[4] - f[2], f[5] - f[3]),
        'A' => arc_geo(f, s).map(|g| arc_point_tangent(&g, 1.0).1)?,
        _ => return None,
    };
    (d.0 * d.0 + d.1 * d.1 > 1e-18).then_some(d)
}

/// The path's two head frames: the BEGIN tip with its OUTWARD
/// direction (the reversed start tangent) and the END tip with the
/// travel direction. True segment tangents where available, the
/// chord otherwise; needs two distinct points. `M`-only, malformed,
/// or CLOSED single-subpath data returns None (the caller skips the
/// heads but keeps the shaft — #227: a closed geometry draws no line
/// ends).
fn path_frames(d: &str) -> Option<(HeadFrame, HeadFrame)> {
    let cmds = parse_path_commands(d)?;
    let segs = path_segments(&cmds)?;
    let eps = 1e-12;
    let first = segs.first()?;
    let last = segs.last()?;
    // #227: a geometry whose walk closes draws no line ends whatever
    // the arrow cells say — a closed rect with EndArrow=5 gets a clean
    // corner in Visio, not an arrowhead (MisterDiagram #105 witness;
    // the cells stay in the file, only rendering is gated). Closure =
    // the single-subpath walk returning to its start (the same test
    // `trim_path` uses to refuse trimming a loop). Multi-subpath
    // sections take no heads either — the same evidence note
    // `trim_path` carries (Visio gives them no heads; CodeRabbit
    // flagged the last-subpath-closed case, #278).
    if cmds.iter().filter(|(c, _)| *c == 'M').count() != 1 {
        return None;
    }
    let start = first.2;
    let last_end = last.3;
    if ((last_end.0 - start.0).powi(2) + (last_end.1 - start.1).powi(2)).sqrt() < 1e-9 {
        return None;
    }
    let begin = seg_start_tangent(first.0, &first.1, first.2)
        .or_else(|| {
            let (dx, dy) = (first.3 .0 - first.2 .0, first.3 .1 - first.2 .1);
            (dx * dx + dy * dy > eps).then_some((dx, dy))
        })
        .map(|d| (first.2, (-d.1).atan2(-d.0)))?;
    let end = seg_end_tangent(last.0, &last.1, last.2)
        .or_else(|| {
            let (dx, dy) = (last.3 .0 - last.2 .0, last.3 .1 - last.2 .1);
            (dx * dx + dy * dy > eps).then_some((dx, dy))
        })
        .map(|d| (last.3, d.1.atan2(d.0)))?;
    Some((begin, end))
}

/// Emit one arrow head at `tip` facing along `dir` (radians; the +x
/// of the head frame): a translate·rotate·scale group of the
/// unit-space gallery parts. Open parts stroke with the head pen
/// (the line weight, or the 0.75 pt zero-weight nominal) with round
/// caps/joins like the print; filled parts fill only — the print
/// shows no pen inflation on filled glyphs (#185).
fn emit_arrow_head(
    out: &mut String,
    kind: i64,
    sz: f64,
    weight: f64,
    color: &str,
    tip: (f64, f64),
    dir: f64,
) {
    let i = (sz.round() as i64).clamp(0, 6) as usize;
    let unit = weight + ARROW_SIZE_PT[i] / 72.0;
    if !unit.is_finite() || unit <= 0.0 {
        return;
    }
    let Some(parts) = arrow_spec(kind) else {
        return;
    };
    // The group scales unit-space geometry by `unit`; open strokes
    // divide the pen back out so the ink is `weight` wide on paper.
    let pen = weight / unit;
    let color = escape_attr(color);
    let mut body = String::new();
    for part in parts {
        if let Some((cx, cy, r)) = part.circle {
            body += &if part.filled {
                format!(r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{color}"/>"#)
            } else {
                format!(
                    r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{color}" stroke-width="{pen:.5}"/>"#
                )
            };
        } else if part.filled {
            body += &format!(r#"<path d="{}" fill="{color}"/>"#, part.d);
        } else {
            body += &format!(
                r#"<path d="{}" fill="none" stroke="{color}" stroke-width="{pen:.5}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                part.d
            );
        }
    }
    let _ = writeln!(
        out,
        r#"<g class="dc-ah" transform="translate({:.5} {:.5}) rotate({:.4}) scale({:.5})">{}</g>"#,
        tip.0 + 0.0, // normalize -0.0
        tip.1 + 0.0,
        dir.to_degrees() + 0.0,
        unit,
        body
    );
}
/// Tiny FNV-1a for stable marker-id hashing (no external deps).
pub(crate) fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Solve an `EllipticalArcTo`: start `s`, on-arc point `m`, end `e` (all in
/// master-space, y-up), major-axis angle `c_ang` (radians) and axis ratio
/// `ratio` (ry/rx). Returns SVG `A` parameters: rx, ry, x-axis-rotation in
/// degrees (already mirrored for the Y-flipped path frame), large-arc and
/// sweep flags — with the endpoint scaled by `scale`.
///
/// The ellipse center satisfies, in the axis-rotated frame, the linear
/// system derived from `k²(x-ox)² + (y-oy)² = k²·rx²` for each of the three
/// points.
pub(crate) fn solve_ellipse_arc(
    s: (f64, f64),
    m: (f64, f64),
    e: (f64, f64),
    c_ang: f64,
    ratio: f64,
    scale: (f64, f64),
) -> Option<(f64, f64, f64, bool, u8)> {
    let k = ratio;
    let (cos_c, sin_c) = (c_ang.cos(), c_ang.sin());
    let rot =
        |p: (f64, f64)| -> (f64, f64) { (cos_c * p.0 + sin_c * p.1, -sin_c * p.0 + cos_c * p.1) };
    let ps = rot(s);
    let pm = rot(m);
    let pe = rot(e);

    // 2·k²(Px-Qx)·ox + 2(Py-Qy)·oy = k²(Px²-Qx²) + (Py²-Qy²)
    let eqn = |p: (f64, f64), q: (f64, f64)| -> ((f64, f64), f64) {
        (
            (2.0 * k * k * (p.0 - q.0), 2.0 * (p.1 - q.1)),
            k * k * (p.0 * p.0 - q.0 * q.0) + (p.1 * p.1 - q.1 * q.1),
        )
    };
    let ((a1, b1), c1) = eqn(ps, pm);
    let ((a2, b2), c2) = eqn(pm, pe);
    let det = a1 * b2 - a2 * b1;
    if det.abs() < 1e-12 {
        return None;
    }
    let ox = (c1 * b2 - c2 * b1) / det;
    let oy = (a1 * c2 - a2 * c1) / det;

    // Semi-axes: k²(Sx-ox)² + (Sy-oy)² = k²·rx²  →  rx = √(…)/k, ry = k·rx.
    let rx = ((k * k * (ps.0 - ox).powi(2) + (ps.1 - oy).powi(2)) / (k * k)).sqrt();
    if !rx.is_finite() || rx < 1e-9 {
        return None;
    }
    let ry = k * rx;

    // Traversal direction and sweep magnitude from parametric angles.
    let ang = |p: (f64, f64)| -> f64 { ((p.1 - oy) / ry).atan2((p.0 - ox) / rx) };
    let norm = |a: f64| -> f64 {
        let mut a = a;
        while a > std::f64::consts::PI {
            a -= 2.0 * std::f64::consts::PI;
        }
        while a < -std::f64::consts::PI {
            a += 2.0 * std::f64::consts::PI;
        }
        a
    };
    let t_s = ang(ps);
    let t_m = ang(pm);
    let t_e = ang(pe);
    let dir = norm(t_m - t_s).signum();
    let mut delta = norm(t_e - t_s);
    if delta * dir < 0.0 {
        delta += dir * 2.0 * std::f64::consts::PI;
    }
    let large = delta.abs() > std::f64::consts::PI;

    // Sweep flag from the traversal s → m → e: the path frame is
    // Y-mirrored and displayed y-down, so a counter-clockwise turn in
    // the y-up frame stays counter-clockwise on the page — SVG's
    // negative-angle direction (sweep 0); a clockwise one is sweep 1.
    // (The old `cross * dir` product was positive for every well-formed
    // arc, so clockwise arcs bulged to the wrong side of their chord —
    // Visio 2021 witness t14.)
    let cross = (m.0 - s.0) * (e.1 - m.1) - (m.1 - s.1) * (e.0 - m.0);
    let sweep = if cross > 0.0 { 0 } else { 1 };

    // Scale the ellipse anisotropically to the instance frame.
    let (rx2, ry2, ang2) = transform_ellipse(rx, k, c_ang, scale);
    Some((
        rx2.max(1e-6),
        ry2.max(1e-6),
        -ang2.to_degrees(),
        large,
        sweep,
    ))
}

/// Transform an ellipse (semi-major `r_major`, ratio `k = minor/major`, axis
/// angle `ang`) by an anisotropic scale `scale`. Returns the new semi-axes
/// (major, minor) and the new major-axis angle.
///
/// The image ellipse's squared semi-axes are the eigenvalues of
/// `D·R·diag(r², (k·r)²)·Rᵀ·D` with `D = diag(scale)`.
pub(crate) fn transform_ellipse(
    r_major: f64,
    k: f64,
    ang: f64,
    scale: (f64, f64),
) -> (f64, f64, f64) {
    let (c, si) = (ang.cos(), ang.sin());
    let a = r_major * r_major;
    let b = (k * r_major) * (k * r_major);
    // Q = R·diag(a,b)·Rᵀ
    let q11 = a * c * c + b * si * si;
    let q12 = (a - b) * c * si;
    let q22 = a * si * si + b * c * c;
    // B = D·Q·D
    let (sx, sy) = scale;
    let m11 = q11 * sx * sx;
    let m12 = q12 * sx * sy;
    let m22 = q22 * sy * sy;
    // Eigenvalues of symmetric [[m11, m12], [m12, m22]].
    let tr = m11 + m22;
    let det = m11 * m22 - m12 * m12;
    let disc = ((tr * tr / 4.0) - det).max(0.0).sqrt();
    let lam1 = tr / 2.0 + disc; // major²
    let lam2 = (tr / 2.0 - disc).max(1e-12); // minor²
                                             // Eigenvector for lam1: B·(λ-m22, m12) = λ·(λ-m22, m12) for symmetric B.
    let (vx, vy) = if m12.abs() > 1e-12 {
        (lam1 - m22, m12)
    } else if m11 >= m22 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };
    let mut new_ang = vy.atan2(vx);
    if new_ang > std::f64::consts::FRAC_PI_2 {
        new_ang -= std::f64::consts::PI;
    } else if new_ang < -std::f64::consts::FRAC_PI_2 {
        new_ang += std::f64::consts::PI;
    }
    (lam1.sqrt(), lam2.sqrt(), new_ang)
}

/// Rel* rows express coordinates as fractions of Width/Height.
fn rel_adjust(t: &str, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    if t.starts_with("Rel") {
        (x * w, y * h)
    } else {
        (x, y)
    }
}

/// The stroke half of a style attribute (#228 split): dashed strokes
/// carry `stroke-linecap="round"` — the cap the dash table's
/// ink→geometry conversion assumes (#235).
fn stroke_attr(
    no_line: bool,
    line_color: &str,
    line_weight: f64,
    dash: Option<&str>,
    alpha: f64,
) -> String {
    if no_line {
        r#"stroke="none""#.to_string()
    } else {
        let dash = dash
            .map(|d| format!(r#" stroke-dasharray="{d}" stroke-linecap="round""#))
            .unwrap_or_default();
        // #235: an explicit-zero LineWeight stays zero — SVG paints
        // nothing at stroke-width 0, exactly the Visio behavior (a
        // fallback pen drew stray hairlines; MisterDiagram witness).
        // The old `.max(0.001)` fallback pen is gone.
        format!(
            r#"stroke="{}" stroke-width="{:.5}"{dash}{}"#,
            escape_attr(line_color),
            line_weight,
            opacity_attr("stroke-opacity", alpha)
        )
    }
}

/// A Visio 2013+ gradient as SVG paint: a `url(#…)` linear/radial
/// gradient, or — for the rectangular directions SVG has no gradient
/// for — tiles (four triangles, each a linear gradient from the
/// origin to one side) the caller clips to the fill path.
#[derive(Clone)]
enum GradientPaint {
    Url(String),
    Tiles(String),
}

/// A legacy fill pattern (FillPattern 2–23): Visio draws them as 8×8
/// device-pixel bitmaps at 96 dpi — period 1/12 in, 1/96 in lines in
/// FillForegnd over FillBkgnd (Visio 2021 witness t12). The line
/// patterns become clipped strokes; the dot/dither patterns a mixed
/// color (8 ≈ ¾ foreground, the rest ≈ ½ — their bitmaps are not
/// modeled).
enum Hatch {
    Lines(String),
    Mixed(String),
}

fn hatch_fill(pattern: f64, fg: &str, bg: &str, (w, h): (f64, f64)) -> Option<Hatch> {
    let pat = pattern.round() as i64;
    if !(2..=23).contains(&pat) {
        return None;
    }
    if pat >= 8 {
        let (a, b) = (hex_rgb(fg), hex_rgb(bg));
        let k = if pat == 8 { 0.75 } else { 0.5 };
        let mix = |x: u8, y: u8| (f64::from(x) * k + f64::from(y) * (1.0 - k)).round() as u8;
        return Some(Hatch::Mixed(format!(
            "#{:02x}{:02x}{:02x}",
            mix(a.0, b.0),
            mix(a.1, b.1),
            mix(a.2, b.2)
        )));
    }
    let period = 1.0 / 12.0;
    let pen = 1.0 / 96.0;
    // The local box spans x 0…|w|, y −|h|…0 (y down); lines cover it
    // generously and the fill path clips them.
    let (w, h) = (w.abs(), h.abs());
    let span = w + h + period;
    let mut d = String::new();
    let mut lines = |dir: u8| {
        let n = (span / period).ceil() as i64 + 1;
        for k in -n..=n {
            let c = k as f64 * period;
            let (a, b) = match dir {
                // horizontal / vertical
                0 => ((-period, -c), (w + period, -c)),
                1 => ((c, -h - period), (c, period)),
                // "/" (x + y = c) and "\" (x − y = c), y down
                2 => ((c + h + period, -h - period), (c - period, period)),
                _ => ((c - h - period, -h - period), (c + period, period)),
            };
            let _ = write!(d, "M {:.4} {:.4} L {:.4} {:.4} ", a.0, a.1, b.0, b.1);
        }
    };
    match pat {
        2 => lines(2),
        3 => {
            lines(0);
            lines(1);
        }
        4 => {
            lines(2);
            lines(3);
        }
        5 => lines(3),
        6 => lines(0),
        _ => lines(1),
    }
    Some(Hatch::Lines(format!(
        r#"<rect x="0" y="{:.5}" width="{w:.5}" height="{h:.5}" fill="{}"/><path d="{}" fill="none" stroke="{}" stroke-width="{pen:.5}"/>"#,
        -h,
        escape_attr(bg),
        d.trim_end(),
        escape_attr(fg)
    )))
}

/// One gradient stop: color, alpha, position 0–1.
pub(crate) type Stop = (String, f64, f64);

/// The `<prefix>Gradient` stop rows (FillGradient / LineGradient)
/// through the instance → master → style chain; per-row cells the
/// instance row omits come from the master's row of the same IX.
fn gradient_stops(doc: &Document, s: &Shape, chain: &[&Shape], prefix: &str) -> Vec<Stop> {
    let name = format!("{prefix}Gradient");
    fn first<'a>(sheet: &'a Sheet, name: &str) -> Option<&'a vsdx::Section> {
        sheet.sections_named(name).into_iter().next()
    }
    let src = doc
        .instance_source(s, chain)
        .and_then(|m| first(&m.sheet, &name));
    // The style chain (the shape's — or its master's — Fill/LineStyle,
    // then each style's parent attribute): the first style whose stop
    // rows carry real values. The Theme style's `V='Themed'` rows are
    // no values without an evaluated theme.
    let parent_attr = format!("{prefix}Style");
    let mut style = None;
    let mut id = doc.shape_style_for(s, chain, &format!("{prefix}Gradient"));
    for _ in 0..8 {
        let Some(st) = doc.stylesheet(id) else { break };
        if let Some(sec) = first(&st.sheet, &name) {
            let real = sec.rows.iter().any(|r| {
                r.cell("GradientStopColor")
                    .is_some_and(|c| !c.is_themed() && !c.is_inherited())
            });
            if real {
                style = Some(sec);
                break;
            }
        }
        match st
            .attrs
            .iter()
            .find(|(n, _)| *n == parent_attr)
            .and_then(|(_, v)| v.parse::<i64>().ok())
        {
            Some(next) if next != id => id = next,
            _ => break,
        }
    }
    // Stop rows merge per IX across the layers — instance, master,
    // style — and per CELL within a row: a master row that overrides
    // only `GradientStopColor` keeps the style row's position (Visio
    // 2021 witness: the Network-Locations cloud overrides stop 0's
    // color and inherits stop 1 entirely from "3D Net Face"). A row
    // deleted (`Del='1'`) in the topmost layer that mentions it is gone.
    let layers: Vec<&vsdx::Section> = [first(&s.sheet, &name), src, style]
        .into_iter()
        .flatten()
        .collect();
    let mut ixs: Vec<i64> = layers
        .iter()
        .flat_map(|sec| sec.rows.iter().map(|r| r.ix))
        .collect();
    ixs.sort_unstable();
    ixs.dedup();
    let mut stops: Vec<Stop> = ixs
        .into_iter()
        .filter_map(|ix| {
            let rows: Vec<&vsdx::Row> = layers
                .iter()
                .filter_map(|sec| sec.rows.iter().find(|r| r.ix == ix))
                .collect();
            if rows.first().is_none_or(|r| r.is_deleted()) {
                return None;
            }
            let cell = |n: &str| {
                rows.iter()
                    .take_while(|r| !r.is_deleted())
                    .filter_map(|r| r.cell(n))
                    .find(|c| !c.is_inherited() && !c.is_themed())
            };
            let col = cell("GradientStopColor")?;
            let color = color_value(doc, col.v.as_deref().unwrap_or(""), col.f.as_deref());
            let trans = cell("GradientStopColorTrans")
                .and_then(|c| c.numeric())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let pos = cell("GradientStopPosition")
                .and_then(|c| c.numeric())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            Some((color, 1.0 - trans, pos))
        })
        .collect();
    stops.sort_by(|a, b| a.2.total_cmp(&b.2));
    stops
}

/// `<stop>` elements for `stops`.
/// Sub-stops per gradient segment: SVG and PDF ramp linearly between
/// stops, so Visio's eased ramp is sampled into this many pieces (the
/// largest gap to the curve stays under 0.012 of the color span).
pub(crate) const EASE_STEPS: usize = 8;

/// `#rrggbb` → linear-light channels; `None` for anything else, which
/// then ramps as given.
fn linear_rgb(c: &str) -> Option<[f64; 3]> {
    let h = c.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let ch = |i: usize| -> Option<f64> {
        let v = f64::from(u8::from_str_radix(&h[i..i + 2], 16).ok()?) / 255.0;
        Some(if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        })
    };
    Some([ch(0)?, ch(2)?, ch(4)?])
}

/// Linear-light channels → `#rrggbb`.
fn srgb_hex(l: [f64; 3]) -> String {
    let enc = |v: f64| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    format!("#{:02x}{:02x}{:02x}", enc(l[0]), enc(l[1]), enc(l[2]))
}

/// Whether Visio draws `stops` as an eased two-colour ramp. Visio 2021
/// (witness: per-variant ramps on its own PNG export) eases and mixes in
/// linear light only a true two-colour gradient spanning the whole box:
/// two stops at exactly 0 and 1, or a mirrored A-B-A with its ends at 0
/// and 1 (the middle stop anywhere). Every other gradient, with an end
/// stop inset (0.01, 0.99, 0.2/0.8), three distinct colours or four
/// stops, ramps linearly in sRGB (fits to 0.4–0.9 of 255).
fn eased_two_colour(stops: &[Stop]) -> bool {
    let at = |p: f64, v: f64| (p - v).abs() < 1e-9;
    match stops {
        [(_, _, p0), (_, _, p1)] => at(*p0, 0.0) && at(*p1, 1.0),
        [(c0, a0, p0), _, (c2, a2, p2)] => {
            at(*p0, 0.0) && at(*p2, 1.0) && c0.eq_ignore_ascii_case(c2) && (a0 - a2).abs() < 1e-9
        }
        _ => false,
    }
}

/// The SVG stops for a Visio gradient. A true two-colour ramp (see
/// [`eased_two_colour`]) is eased with smoothstep (3u² − 2u³) and mixed
/// in linear light, as Visio 2021 draws it (the network LLD's cloud
/// puffs came out far too dark as a plain sRGB ramp); each segment is
/// sampled into [`EASE_STEPS`] pieces. Every other gradient keeps its
/// stops as given, which SVG ramps linearly in sRGB, like Visio.
pub(crate) fn stop_elems(stops: &[Stop]) -> String {
    let eased = eased_two_colour(stops);
    let mut out = String::new();
    let mut emit = |c: &str, a: f64, p: f64| {
        let _ = write!(
            out,
            r#"<stop offset="{p:.4}" stop-color="{}"{}/>"#,
            escape_attr(c),
            opacity_attr("stop-opacity", a)
        );
    };
    for (i, (c, a, p)) in stops.iter().enumerate() {
        if let Some((pc, pa, pp)) = i.checked_sub(1).map(|j| &stops[j]) {
            if let (true, Some(x), Some(y), true) = (eased, linear_rgb(pc), linear_rgb(c), p > pp) {
                for k in 1..EASE_STEPS {
                    let u = k as f64 / EASE_STEPS as f64;
                    let e = u * u * (3.0 - 2.0 * u);
                    let mix = [0, 1, 2].map(|n| x[n] + (y[n] - x[n]) * e);
                    emit(&srgb_hex(mix), pa + (a - pa) * e, pp + (p - pp) * u);
                }
            }
        }
        emit(c, *a, *p);
    }
    out
}

/// Resolve a `Fill`/`Line` gradient for one shape (Visio 2021
/// witnesses t08–t10). `None` unless `<prefix>GradientEnabled` is on.
///
/// - Stops come from the `<prefix>Gradient` rows; a shape with the
///   flag but no rows takes `fallback` (FillForegnd → FillBkgnd).
/// - Dir 0 is linear along `<prefix>GradientAngle` (default 90°):
///   0° runs left → right, 90° top → bottom, 45° from the top-left
///   to the bottom-right corner — the axis spans the box's projection
///   onto the direction, so stop 0 and 1 land exactly on the corners.
/// - Dir 1–7 are radial from a box anchor (1 bottom-right, 2
///   bottom-left, 3 center, 4 top-center, 5 bottom-center, 6
///   top-right, 7 top-left), reaching the farthest corner.
/// - Dir 8–12 are rectangular from an anchor (8 bottom-right, 9
///   bottom-left, 10 center, 11 top-right, 12 top-left); 13 (path)
///   renders as the centered rectangle.
///
/// Coordinates are the shape's local SVG frame (x 0…W, y −H…0), so
/// every gradient is `userSpaceOnUse` and the definitions dedupe by
/// content hash into `out`. The box is the frame's ABSOLUTE extent: a
/// connector drawn right-to-left has a negative Width (`EndX−BeginX`),
/// and Visio still runs a 0° gradient left → right across it.
#[allow(clippy::too_many_arguments)]
fn gradient_paint(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    env: &vsdx::formula::Env,
    prefix: &str,
    (w, h): (f64, f64),
    fallback: (&str, &str),
    out: &mut String,
) -> Option<GradientPaint> {
    let num = |n: &str| {
        doc.resolve_cell(s, chain, &format!("{prefix}{n}"))
            .and_then(|c| cell_num(&c, env))
            .filter(|v| v.is_finite())
    };
    if num("GradientEnabled").is_none_or(|v| v == 0.0) {
        return None;
    }
    let mut stops = gradient_stops(doc, s, chain, prefix);
    if stops.is_empty() {
        stops = vec![
            (fallback.0.to_string(), 1.0, 0.0),
            (fallback.1.to_string(), 1.0, 1.0),
        ];
    }
    let stops_svg = stop_elems(&stops);
    // The absolute box in the local SVG frame (y down).
    let (bl, br) = (w.min(0.0), w.max(0.0));
    let (bt, bb) = ((-h).min(0.0), (-h).max(0.0));
    let (w, h) = (br - bl, bb - bt);
    let dir = num("GradientDir").unwrap_or(0.0).round() as i64;
    let angle = num("GradientAngle").unwrap_or(std::f64::consts::FRAC_PI_2);
    let def = |out: &mut String, body: String| -> String {
        let id = format!("dc-grad{:x}", fnv(&body));
        if !out.contains(&format!(r#"id="{id}""#)) {
            let _ = writeln!(out, "<defs>{}</defs>", body.replace("{ID}", &id));
        }
        id
    };
    let linear = |out: &mut String, (x1, y1): (f64, f64), (x2, y2): (f64, f64)| {
        def(
            out,
            format!(
                r#"<linearGradient id="{{ID}}" gradientUnits="userSpaceOnUse" x1="{x1:.5}" y1="{y1:.5}" x2="{x2:.5}" y2="{y2:.5}">{stops_svg}</linearGradient>"#
            ),
        )
    };
    // Box anchors in the local SVG frame (y down, −H…0).
    let (mx, my) = ((bl + br) / 2.0, (bt + bb) / 2.0);
    let anchor = |k: i64| -> (f64, f64) {
        match k {
            1 | 8 => (br, bb),
            2 | 9 => (bl, bb),
            4 => (mx, bt),
            5 => (mx, bb),
            6 | 11 => (br, bt),
            7 | 12 => (bl, bt),
            _ => (mx, my),
        }
    };
    let rect = (8..=13).contains(&dir) && prefix == "Fill";
    if (1..=7).contains(&dir) {
        let (cx, cy) = anchor(dir);
        let r = [(bl, bt), (br, bt), (bl, bb), (br, bb)]
            .iter()
            .map(|(x, y)| (x - cx).hypot(y - cy))
            .fold(0.0, f64::max)
            .max(1e-6);
        let id = def(
            out,
            format!(
                r#"<radialGradient id="{{ID}}" gradientUnits="userSpaceOnUse" cx="{cx:.5}" cy="{cy:.5}" r="{r:.5}">{stops_svg}</radialGradient>"#
            ),
        );
        return Some(GradientPaint::Url(format!("url(#{id})")));
    }
    if rect {
        // Chebyshev distance from the anchor: four triangles (one per
        // side), each a linear ramp from the anchor to that side.
        let (ox, oy) = anchor(dir);
        let rx = (ox - bl).max(br - ox).max(1e-6);
        let ry = (oy - bt).max(bb - oy).max(1e-6);
        let mut tiles = String::new();
        for (sx, sy, horiz) in [
            (1.0, 0.0, true),
            (-1.0, 0.0, true),
            (0.0, 1.0, false),
            (0.0, -1.0, false),
        ] {
            let (c1, c2) = if horiz {
                ((ox + sx * rx, oy - ry), (ox + sx * rx, oy + ry))
            } else {
                ((ox - rx, oy + sy * ry), (ox + rx, oy + sy * ry))
            };
            let end = (ox + sx * rx, oy + sy * ry);
            let id = linear(out, (ox, oy), end);
            let _ = write!(
                tiles,
                r#"<path d="M {ox:.5} {oy:.5} L {:.5} {:.5} L {:.5} {:.5} Z" fill="url(#{id})"/>"#,
                c1.0, c1.1, c2.0, c2.1
            );
        }
        return Some(GradientPaint::Tiles(tiles));
    }
    let (dx, dy) = (angle.cos(), angle.sin());
    let half = (dx.abs() * w + dy.abs() * h) / 2.0;
    let (cx, cy) = (mx, my);
    let id = linear(
        out,
        (cx - dx * half, cy - dy * half),
        (cx + dx * half, cy + dy * half),
    );
    Some(GradientPaint::Url(format!("url(#{id})")))
}

/// Round the corners of a path (the renderer's own number format) with
/// radius `r`, the way Visio's Rounding cell does: each corner is
/// replaced by a circular arc tangent to both legs, the tangent length
/// clamped to half of the shorter leg so neighboring arcs never
/// overlap. A walk that returns to its start rounds that corner too.
///
/// Curves keep their authored shape and their corners stay sharp, with
/// one exception: an elliptical arc (`A`) whose bulge is no deeper than
/// `r` counts as its chord, because Visio's rounding swallows it — a
/// pill drawn as a flat end plus a shallow arc end with `Rounding`
/// above half its height comes out fully round at both ends (the
/// network LLD's BVI and label pills). Such an arc turns into its chord
/// only where a corner at one of its ends actually rounds; elsewhere it
/// keeps its authored shape. `None` (path unchanged) when no corner
/// rounds, or for more than one subpath.
pub(crate) fn round_corners(d: &str, r: f64) -> Option<String> {
    // One segment of the walk: its end point, and the original command
    // text when it is a curve (`None` = straight, or an arc flat enough
    // to round through). `arc` keeps such a flat arc's command for when
    // no corner at its ends rounds.
    struct Seg {
        to: (f64, f64),
        curve: Option<String>,
        arc: Option<String>,
    }
    let mut start: Option<(f64, f64)> = None;
    let mut segs: Vec<Seg> = Vec::new();
    // Curve rows end without a space, so the next command letter can
    // arrive glued to the last number (`… -0.09843L 0 …`): split every
    // letter off as its own token. Our numbers carry no exponent.
    let tokens: Vec<&str> = d
        .split_whitespace()
        .flat_map(|w| {
            let mut parts = Vec::new();
            let mut from = 0;
            for (i, c) in w.char_indices() {
                if c.is_ascii_alphabetic() {
                    if i > from {
                        parts.push(&w[from..i]);
                    }
                    parts.push(&w[i..i + 1]);
                    from = i + 1;
                }
            }
            if from < w.len() {
                parts.push(&w[from..]);
            }
            parts
        })
        .collect();
    let mut toks = tokens.into_iter().peekable();
    let mut moves = 0;
    let num = |t: Option<&str>| -> Option<f64> { t?.parse().ok() };
    let mut cur = (0.0, 0.0);
    while let Some(t) = toks.next() {
        match t {
            "M" => {
                moves += 1;
                let p = (num(toks.next())?, num(toks.next())?);
                start = Some(p);
                cur = p;
            }
            "L" => {
                let p = (num(toks.next())?, num(toks.next())?);
                if (p.0 - cur.0).hypot(p.1 - cur.1) > 1e-9 {
                    segs.push(Seg {
                        to: p,
                        curve: None,
                        arc: None,
                    });
                }
                cur = p;
            }
            "A" => {
                let mut v = [0.0f64; 7];
                for x in v.iter_mut() {
                    *x = num(toks.next())?;
                }
                let p = (v[5], v[6]);
                let flat = v[3] == 0.0 && arc_sagitta(cur, p, v[0], v[1], v[2], v[4] != 0.0) <= r;
                let text = format!(
                    "A {:.5} {:.5} {} {} {} {:.5} {:.5} ",
                    v[0], v[1], v[2], v[3], v[4], p.0, p.1
                );
                let (curve, arc) = if flat {
                    (None, Some(text))
                } else {
                    (Some(text), None)
                };
                segs.push(Seg { to: p, curve, arc });
                cur = p;
            }
            "C" | "Q" => {
                let n = if t == "C" { 6 } else { 4 };
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(num(toks.next())?);
                }
                let p = (v[n - 2], v[n - 1]);
                let mut text = format!("{t} ");
                for x in &v {
                    let _ = write!(text, "{x:.5} ");
                }
                segs.push(Seg {
                    to: p,
                    curve: Some(text),
                    arc: None,
                });
                cur = p;
            }
            "Z" => {}
            _ => return None,
        }
    }
    let start = start?;
    if moves != 1 || segs.len() < 2 {
        return None;
    }
    let closed = {
        let b = segs[segs.len() - 1].to;
        (start.0 - b.0).hypot(start.1 - b.1) < 1e-6
    };
    let n = segs.len();
    // Vertex i is where segment i ends (segment i+1 starts); its point.
    let at = |i: usize| segs[i % n].to;
    // The point a segment starts from.
    let from = |i: usize| if i == 0 { start } else { segs[i - 1].to };
    // (tangent point on the incoming leg, on the outgoing leg, arc
    // radius, SVG sweep flag)
    type Corner = ((f64, f64), (f64, f64), f64, u8);
    // The corner at the end of segment `i`, between it and segment
    // `i + 1` (wrapping for a closed walk) — only where both are
    // straight.
    let corner = |i: usize| -> Option<Corner> {
        let j = if i + 1 < n {
            i + 1
        } else if closed {
            0
        } else {
            return None;
        };
        if segs[i].curve.is_some() || segs[j].curve.is_some() {
            return None;
        }
        let p = at(i);
        let a = from(i);
        let b = segs[j].to;
        let (la, lb) = ((a.0 - p.0).hypot(a.1 - p.1), (b.0 - p.0).hypot(b.1 - p.1));
        if la < 1e-9 || lb < 1e-9 {
            return None;
        }
        let u = ((a.0 - p.0) / la, (a.1 - p.1) / la);
        let v = ((b.0 - p.0) / lb, (b.1 - p.1) / lb);
        let theta = (u.0 * v.0 + u.1 * v.1).clamp(-1.0, 1.0).acos();
        if theta < 1e-6 || (std::f64::consts::PI - theta) < 1e-6 {
            return None;
        }
        let half_tan = (theta / 2.0).tan();
        let t = (r / half_tan).min(la / 2.0).min(lb / 2.0);
        let rr = t * half_tan;
        let t1 = (p.0 + u.0 * t, p.1 + u.1 * t);
        let t2 = (p.0 + v.0 * t, p.1 + v.1 * t);
        // Incoming direction (a→p) × outgoing (p→b): the turn's side
        // in the y-down frame picks the arc's sweep.
        let cross = (p.0 - a.0) * (b.1 - p.1) - (p.1 - a.1) * (b.0 - p.0);
        Some((t1, t2, rr, u8::from(cross > 0.0)))
    };
    let corners: Vec<Option<Corner>> = (0..n).map(corner).collect();
    if corners.iter().all(Option::is_none) {
        return None;
    }
    let mut out = String::new();
    // A closed walk starts where the corner at the start leaves off.
    let first = if closed {
        corners[n - 1].map_or(start, |c| c.1)
    } else {
        start
    };
    let _ = write!(out, "M {:.5} {:.5} ", first.0, first.1);
    for (i, seg) in segs.iter().enumerate() {
        match (&seg.curve, corners[i]) {
            (Some(text), _) => out.push_str(text),
            (None, Some(((x1, y1), (x2, y2), rr, sw))) => {
                let _ = write!(
                    out,
                    "L {x1:.5} {y1:.5} A {rr:.5} {rr:.5} 0 0 {sw} {x2:.5} {y2:.5} "
                );
            }
            (None, None) => {
                // The corner this segment leaves from.
                let before = if i > 0 {
                    corners[i - 1]
                } else if closed {
                    corners[n - 1]
                } else {
                    None
                };
                match &seg.arc {
                    // A flat arc no rounded corner touches keeps its shape.
                    Some(text) if before.is_none() => out.push_str(text),
                    _ => {
                        let _ = write!(out, "L {:.5} {:.5} ", seg.to.0, seg.to.1);
                    }
                }
            }
        }
    }
    if closed {
        out.push_str("Z ");
    }
    Some(out)
}

/// The bulge depth of an SVG arc from `a` to `b` (radii `rx`/`ry`,
/// x-axis rotation `rot_deg`, small arc): the largest distance of the
/// arc from its chord. When the radii cannot span the chord they scale
/// up uniformly, as SVG does, and the arc is half of that ellipse.
fn arc_sagitta(a: (f64, f64), b: (f64, f64), rx: f64, ry: f64, rot_deg: f64, sweep: bool) -> f64 {
    let (rx, ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        return 0.0;
    }
    let (sn, cs) = rot_deg.to_radians().sin_cos();
    // Into the frame where the ellipse is the unit circle.
    let to_unit = |p: (f64, f64)| {
        let (x, y) = (p.0 * cs + p.1 * sn, -p.0 * sn + p.1 * cs);
        (x / rx, y / ry)
    };
    let from_unit = |p: (f64, f64)| {
        let (x, y) = (p.0 * rx, p.1 * ry);
        (x * cs - y * sn, x * sn + y * cs)
    };
    let (ua, ub) = (to_unit(a), to_unit(b));
    let c = (ub.0 - ua.0).hypot(ub.1 - ua.1);
    if c < 1e-12 {
        return 0.0;
    }
    // Radii too small for the chord scale up by `k`: in the scaled unit
    // frame the chord is a diameter (SVG 2 implementation notes, F.6.6).
    let k = (c / 2.0).max(1.0);
    let (ua, ub) = ((ua.0 / k, ua.1 / k), (ub.0 / k, ub.1 / k));
    let from_unit = |p: (f64, f64)| from_unit((p.0 * k, p.1 * k));
    let (dx, dy) = (ub.0 - ua.0, ub.1 - ua.1);
    let c = c / k;
    let s = 1.0 - (1.0 - (c / 2.0).powi(2)).max(0.0).sqrt();
    // The unit-frame apex: the chord's midpoint pushed out by the
    // sagitta, on the side the sweep bulges to.
    let (nx, ny) = (-dy / c, dx / c);
    let side = if sweep { -1.0 } else { 1.0 };
    let mid = ((ua.0 + ub.0) / 2.0, (ua.1 + ub.1) / 2.0);
    let apex = from_unit((mid.0 + side * nx * s, mid.1 + side * ny * s));
    let (cx, cy) = (b.0 - a.0, b.1 - a.1);
    let len = cx.hypot(cy);
    ((apex.0 - a.0) * cy - (apex.1 - a.1) * cx).abs() / len
}

/// Does a ShapeSheet formula reference the shape's own `Width` or
/// `Height` cell? A whole-identifier match: `User.BarWidth`,
/// `Sheet.5!Width` or `TxtHeight` are other cells, and treating them as
/// the shape's size scaled a stretched U-Turn arrow's 0.2 in bar to
/// 1.9 in (the network LLD witness).
pub(crate) fn refs_own_size(f: &str) -> bool {
    let b = f.as_bytes();
    ["Width", "Height"].iter().any(|w| {
        f.match_indices(w).any(|(i, _)| {
            let before = i.checked_sub(1).map(|j| b[j]);
            let after = b.get(i + w.len()).copied();
            let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'!';
            !before.is_some_and(ident)
                && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
    })
}

/// The stroke's end caps and corner joins for a LineCap value. Dashed
/// strokes keep the round caps their dash table assumes (#235), but
/// the join still follows LineCap: flat and extended caps miter their
/// corners also when dashed (CodeRabbit on PR #342).
pub(crate) fn cap_attr(cap: i64, dashed: bool) -> String {
    let join = if matches!(cap, 1 | 2) {
        "miter"
    } else {
        "round"
    };
    let cap = match (cap, dashed) {
        (_, true) => String::new(),
        (1, false) => r#" stroke-linecap="butt""#.into(),
        (2, false) => r#" stroke-linecap="square""#.into(),
        _ => r#" stroke-linecap="round""#.into(),
    };
    format!(r#"{cap} stroke-linejoin="{join}""#)
}

/// Close a single-subpath stroke that returns to its start (`Z`), so
/// the start corner joins like every other corner instead of showing
/// two butt ends (Visio 2021 witness, b04's rectangle).
pub(crate) fn close_returning_path(d: &str) -> String {
    let t = d.trim_end();
    if t.ends_with('Z') || t.matches('M').count() != 1 {
        return d.to_string();
    }
    let nums: Vec<f64> = t
        .split(|c: char| c.is_whitespace() || c.is_ascii_alphabetic())
        .filter_map(|x| x.parse().ok())
        .collect();
    let first = t
        .trim_start()
        .strip_prefix('M')
        .map(|r| {
            r.split_whitespace()
                .take(2)
                .filter_map(|x| x.parse::<f64>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match (first.as_slice(), nums.len()) {
        ([x0, y0], n) if n >= 6 => {
            let (x1, y1) = (nums[n - 2], nums[n - 1]);
            if (x1 - x0).hypot(y1 - y0) < 1e-6 {
                format!("{t} Z")
            } else {
                d.to_string()
            }
        }
        _ => d.to_string(),
    }
}

/// The closed subpaths of a path in the renderer's own format
/// (space-separated tokens, each subpath opened by `M`): a subpath is
/// closed when it ends in `Z` or its last point returns to its `M`
/// point. Any command closes, so a single curve that returns to its
/// start counts. Open subpaths are dropped.
pub(crate) fn closed_subpaths(d: &str) -> String {
    let mut out = String::new();
    let mut flush = |sub: &[&str]| {
        if sub.len() < 4 {
            return; // `M x y` alone draws nothing
        }
        let num = |t: &str| t.parse::<f64>().ok();
        let closed = sub.last() == Some(&"Z")
            || match (
                num(sub[1]),
                num(sub[2]),
                num(sub[sub.len() - 2]),
                num(sub[sub.len() - 1]),
            ) {
                (Some(x0), Some(y0), Some(x1), Some(y1)) => {
                    (x0 - x1).abs() < 1e-4 && (y0 - y1).abs() < 1e-4
                }
                _ => false,
            };
        if closed {
            out.push_str(&sub.join(" "));
            out.push(' ');
        }
    };
    // Command letters need not be space-separated: curve and arc rows
    // trim their trailing space, so a following `M` lands glued to the
    // last number (`… 0.00000M 1 2`). Split every command letter off
    // as its own token (Greptile on PR #328); our numbers carry no
    // exponent, so no letter is part of a number.
    let toks: Vec<&str> = d
        .split_whitespace()
        .flat_map(|w| {
            let mut parts = Vec::new();
            let mut from = 0;
            for (i, c) in w.char_indices() {
                if c.is_ascii_alphabetic() {
                    if i > from {
                        parts.push(&w[from..i]);
                    }
                    parts.push(&w[i..i + 1]);
                    from = i + 1;
                }
            }
            if from < w.len() {
                parts.push(&w[from..]);
            }
            parts
        })
        .collect();
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        if *t == "M" && i > start {
            flush(&toks[start..i]);
            start = i;
        }
    }
    flush(&toks[start..]);
    out
}

/// The geometry frame's own size on one axis: `dim / scale` recovers
/// the master's dimension from the instance's (`scale` = instance /
/// master, so a negative instance size keeps its sign). A zero scale
/// has no master dimension to recover and keeps `dim` (Greptile on
/// PR #329: the old `scale.max(1e-12)` threw negative scales to 1e12).
pub(crate) fn frame_dim(dim: f64, scale: f64) -> f64 {
    if scale.abs() > 1e-12 {
        dim / scale
    } else {
        dim
    }
}

/// ` name="a"` for a partial alpha, nothing when opaque.
fn opacity_attr(name: &str, alpha: f64) -> String {
    if alpha < 1.0 {
        format!(r#" {name}="{:.3}""#, alpha.max(0.0))
    } else {
        String::new()
    }
}

/// One shape's accumulated paint, #228: every FILL-ELIGIBLE Geometry
/// section's path concatenates into ONE fill path the caller emits
/// with `fill-rule="evenodd"` — Visio's alternate rule, where nesting
/// and overlap toggle coverage regardless of winding direction
/// (MisterDiagram M5.109: inner nested region clear for both
/// directions, overlap clear, third nesting filled). Per-section
/// flags gate participation: NoFill opts a section out of the fill
/// only, NoShow already skipped it entirely, NoLine leaves the fill
/// intact; open contours close implicitly for the fill (SVG fills
/// open subpaths as closed) without gaining a stroked closing edge —
/// the strokes stay per-section, in section order, OVER the fill.
struct ShapePaint {
    /// The concatenated fill subpaths (each section's `d`, which all
    /// start with `M`).
    fill_d: String,
    /// The fill style (shape-level cells — identical for every
    /// participating section; the last one computed wins).
    fill: String,
    /// The fill's alpha (1 − FillForegndTrans / FillBkgndTrans).
    fill_opacity: f64,
    /// A rectangular gradient's tiles (`gradient_paint`): painted
    /// clipped to the fill path instead of the plain fill.
    fill_tiles: Option<String>,
    /// Stroke-only paths, arrow shafts/heads, and standalone ellipse
    /// rows, in emission order.
    strokes: String,
}

impl Default for ShapePaint {
    fn default() -> Self {
        Self {
            fill_d: String::new(),
            fill: "none".to_string(),
            fill_opacity: 1.0,
            fill_tiles: None,
            strokes: String::new(),
        }
    }
}

/// Emit the accumulated paint under the caller's ink (#228): the ONE
/// even-odd fill path first, then the per-section strokes.
fn flush_shape_paint(paint: &mut ShapePaint, out: &mut String) {
    if let (false, Some(tiles)) = (paint.fill_d.is_empty(), &paint.fill_tiles) {
        let d = paint.fill_d.trim();
        let id = format!("dc-gclip{:x}", fnv(&format!("{d}|{tiles}")));
        let _ = writeln!(
            out,
            r#"<clipPath id="{id}"><path clip-rule="evenodd" d="{}"/></clipPath><g clip-path="url(#{id})"{}{}>{tiles}</g>"#,
            escape_attr(d),
            opacity_attr("fill-opacity", paint.fill_opacity),
            opacity_attr("stroke-opacity", paint.fill_opacity)
        );
    } else if !paint.fill_d.is_empty() {
        let _ = writeln!(
            out,
            r#"<path fill-rule="evenodd" d="{}" fill="{}"{}/>"#,
            escape_attr(paint.fill_d.trim()),
            escape_attr(&paint.fill),
            opacity_attr("fill-opacity", paint.fill_opacity)
        );
    }
    out.push_str(&paint.strokes);
}

/// Visio's built-in dash/dot line patterns (LinePattern 2–23) as SVG
/// `stroke-dasharray` values in LineWeight multiples (patterns scale with
/// the line weight — the ShapeSheet gallery's defining property). The
/// table is MEASURED INK — run/gap lengths as painted at 600 dpi — and
/// the caller strokes it with ROUND caps, so the values convert to
/// Visio's canonical GEOMETRY before emission (#235 cap-aware split):
/// a round cap extends each mark by one weight (half at each end) and
/// eats one weight from the gap, i.e. geometry on = ink − 1w, gap =
/// ink + 1w — exactly MisterDiagram's canonical grammar (pattern 2 =
/// 7/5 where our ink reads 8/4; 14 = 9/3/3; 21 = 39/9/15/9). Dots
/// (ink ≤ 1w marks) are weight-diameter circles: geometry length 0,
/// plus a 0.0001w lead so rasterizers keep a tangent for the
/// zero-length subpath (their dot fix; the following gap gives the
/// lead back, period preserved).
pub(crate) fn line_dasharray(pattern: f64, weight: f64) -> Option<String> {
    // #171: the table is measured from Visio 2016 itself — the
    // owner-authored fixture `Sample/test files/lines.vsdx` (patterns
    // 0–23, 0.02 in weight) printed at 600 dpi, run/gap sequences in
    // stroke-width units (Analysis/line-patterns-visio2016.md). 0/1
    // stay None (0 hides the line entirely at the call site).
    let spec: &[f64] = match pattern as i64 {
        2 => &[8.0, 4.0],                         // dash
        3 => &[1.0, 4.0],                         // dot
        4 => &[8.0, 4.0, 1.0, 4.0],               // dash-dot
        5 => &[8.0, 4.0, 1.0, 4.0, 1.0, 4.0],     // dash-dot-dot
        6 => &[8.0, 4.0, 8.0, 4.0, 1.0, 4.0],     // dash-dash-dot
        7 => &[20.0, 4.0, 8.0, 4.0],              // long + short dash
        8 => &[20.0, 4.0, 8.0, 4.0, 8.0, 4.0],    // long + 2 short dashes
        9 => &[4.0, 2.0], // short dash (the corpus legend "long dash" label was wrong)
        10 => &[1.0, 2.0], // fine dot
        11 => &[4.0, 2.0, 1.0, 2.0], // short dash-dot
        12 => &[4.0, 2.0, 1.0, 2.0, 1.0, 2.0], // short dash-dot-dot
        13 => &[4.0, 2.0, 4.0, 2.0, 1.0, 2.0], // short dash-dash-dot
        14 => &[10.0, 2.0, 4.0, 2.0], // medium-short dash pair
        15 => &[10.0, 2.0, 4.0, 2.0, 4.0, 2.0], // medium + 2 short dashes
        16 => &[16.0, 8.0], // long dash
        17 => &[1.0, 8.0], // sparse dot
        18 => &[16.0, 8.0, 1.0, 8.0], // long dash-dot
        19 => &[16.0, 8.0, 1.0, 8.0, 1.0, 8.0], // long dash-dot-dot
        20 => &[16.0, 8.0, 16.0, 8.0, 1.0, 8.0], // long dash-dash-dot
        21 => &[40.0, 8.0, 16.0, 8.0], // extra-long + long dash
        22 => &[40.0, 8.0, 16.0, 8.0, 16.0, 8.0], // extra-long + 2 long
        23 => &[2.0, 1.0], // fine dash
        _ => return None,
    };
    // Ink → geometry under round caps (#235): marks give up one
    // weight to their caps, gaps take it back; a dot's mark becomes
    // the 0.0001w tangent lead and its gap returns the difference so
    // the period is unchanged.
    let dot_lead = 0.0001;
    let geom: Vec<f64> = spec
        .iter()
        .enumerate()
        .map(|(i, ink)| {
            if i % 2 == 0 {
                // An on-mark: a dot (ink ≤ 1w) draws as the lead
                // alone; a dash shortens by its caps.
                if *ink <= 1.0 {
                    dot_lead
                } else {
                    ink - 1.0
                }
            } else {
                // A gap: regains the mark's cap width, minus the
                // lead when the preceding mark was a dot.
                let prev_was_dot = spec[i - 1] <= 1.0;
                ink + 1.0 - if prev_was_dot { dot_lead } else { 0.0 }
            }
        })
        .collect();
    Some(
        geom.iter()
            .map(|m| {
                let v = m * weight;
                // The dot lead (0.0001w) sits far below the 5-decimal
                // rounding floor — print sub-floor values at 10
                // decimals so the lead and its gap compensation
                // survive (CodeRabbit, #278: the old `.max(1e-4)`
                // clamp inflated the lead 50× at a 0.02 weight and
                // stretched the period). Every value is positive by
                // construction, so no zero-length dash entries occur.
                if v < 1e-4 {
                    format!("{v:.10}")
                } else {
                    format!("{v:.5}")
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub(crate) fn themed_color_string(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    cell: &str,
) -> String {
    match doc.resolve_cell(s, chain, cell) {
        Some(c) => color_value(doc, c.v.as_deref().unwrap_or(""), c.f.as_deref()),
        None => palette(-1),
    }
}
