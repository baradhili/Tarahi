//! Page rendering: the public page render entry points and the full
//! page assembly (chrome, background pages, the shape walk).
use super::*;
use std::fmt::Write as _;
pub fn render_page(doc: &Document, page: &vsdx::Page) -> anyhow::Result<String> {
    render_page_with(doc, page, &[])
}

/// The export render of one page (#35): page chrome dropped (no gray
/// sheet border — Visio's own exports carry none), white background
/// per `transparent`. The ink is identical to [`render_page`] — only
/// the chrome differs.
pub fn render_page_export(
    doc: &Document,
    page: &vsdx::Page,
    transparent: bool,
) -> anyhow::Result<String> {
    render_page_full(doc, page, &[], None, None, false, !transparent)
}

/// [`render_page`] with hidden layer members: shapes whose
/// `LayerMember` names one of `hidden` (Visio layer row indexes, the
/// LayerMember cell convention) are skipped entirely — the annotation
/// toggle (#29) and friends.
pub fn render_page_with(
    doc: &Document,
    page: &vsdx::Page,
    hidden: &[i64],
) -> anyhow::Result<String> {
    render_page_opts(doc, page, hidden, None, None)
}

/// [`render_page`] with one shape's TEXT suppressed (GitHub #22): the
/// in-place canvas editor covers the text block, so the page raster
/// underneath must not also show the shape's old text (ghost text
/// under the caret). The shape itself — geometry, fill, borders —
/// keeps rendering; only its own `emit_text` pass is skipped.
pub fn render_page_no_text(
    doc: &Document,
    page: &vsdx::Page,
    hidden: &[i64],
    skip_text_shape: i64,
) -> anyhow::Result<String> {
    render_page_opts(doc, page, hidden, None, Some(skip_text_shape))
}

/// [`render_page_with`] restricted to one top-level shape's subtree —
/// the GUI's group-edit mode (GitHub #78): every other top-level shape
/// is skipped; the group renders with its members at their page
/// positions through the normal group transforms.
pub fn render_page_scoped(
    doc: &Document,
    page: &vsdx::Page,
    hidden: &[i64],
    only: i64,
) -> anyhow::Result<String> {
    render_page_opts(doc, page, hidden, Some(only), None)
}

fn render_page_opts(
    doc: &Document,
    page: &vsdx::Page,
    hidden: &[i64],
    only: Option<i64>,
    skip_text: Option<i64>,
) -> anyhow::Result<String> {
    render_page_full(doc, page, hidden, only, skip_text, true, true)
}

/// [`render_page_opts`] with the page chrome gated: `border` drops the
/// gray sheet frame, `background` drops the white page fill — the
/// file exports (#35) produce the ink only (Visio's own SVG/PDF/PNG
/// exports carry no sheet chrome), while the canvas and `dc render`
/// keep the full picture.
fn render_page_full(
    doc: &Document,
    page: &vsdx::Page,
    hidden: &[i64],
    only: Option<i64>,
    skip_text: Option<i64>,
    border: bool,
    background: bool,
) -> anyhow::Result<String> {
    let w = num(page.sheet().and_then(|s| s.cell("PageWidth"))).unwrap_or(8.5);
    let h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{pw:.0}" height="{ph:.0}">"#,
        pw = w * PX_PER_IN,
        ph = h * PX_PER_IN,
    );
    let _ = writeln!(
        svg,
        r#"<!-- Tarahi (cleanroom VSDX renderer); page '{}' -->"#,
        escape_attr(&page.name)
    );
    if border {
        let _ = writeln!(
            svg,
            r##"<rect x="0" y="0" width="{w}" height="{h}" fill="none" stroke="#c8c8c8" stroke-width="0.01"/>"##
        );
    }
    // White background under the sheet (Visio page is white); an
    // export with `--transparent` omits it so the alpha channel shows.
    if background {
        let _ = writeln!(
            svg,
            r##"<rect x="0" y="0" width="{w}" height="{h}" fill="#ffffff"/>"##,
        );
    }
    let hidden_member = |s: &Shape| -> bool {
        s.sheet
            .cell("LayerMember")
            .and_then(|c| c.numeric())
            .is_some_and(|m| hidden.contains(&(m as i64)))
    };
    // ONE field context for the whole walk (CodeRabbit agent
    // review): the shared `Sheet.N!` layer caches per CONTEXT, so a
    // per-shape context rebuilt it for every root — the quadratic
    // cost the shared layer exists to remove.
    let fields = crate::fields::FieldsCtx::for_page(doc, page);
    // #187: line jumps — Visio prints hops. They are computed BEFORE
    // the ink so each jumping connector can be cut where it hops: the
    // line is clipped open around every hop and the arc alone bridges
    // the gap, exactly like Visio's own jump geometry (I3: the hop is
    // part of the line — the background shows through beneath it, no
    // paper-white gap painted, no under-line redraw; the old overlay's
    // white dash sat on every non-white background, #322). Scoped
    // renders (`only`) draw one shape's subtree — page-wide jumps
    // would drag in crossings from connectors outside the scope. The
    // page's Show-Line-Jumps switch gates them (#251), and a resolved
    // radius of 0 (an explicit LineJumpFactorX=0, #294 review)
    // computes no hops at all.
    let radius = crate::geom::jump_radius_in(doc, page);
    let radius_y = crate::geom::jump_radius_y_in(doc, page);
    let jumps = if only.is_none() && page.line_jumps_shown() && (radius > 0.0 || radius_y > 0.0) {
        crate::geom::page_jumps(doc, page, hidden)
    } else {
        Vec::new()
    };
    for shape in &page.shapes {
        if hidden_member(shape) {
            continue;
        }
        // Guides are page furniture, never ink (#113): Visio excludes
        // them from print and export, and the canvas draws them as its
        // own overlay.
        if shape.type_.as_deref() == Some("Guide") {
            continue;
        }
        if let Some(only) = only {
            if shape.id != only {
                continue;
            }
        }
        // A jumping connector renders inside page-space clips that cut
        // it open around its hops (#324 phase 3). One clip PER HOP,
        // nested: clip groups INTERSECT, so two hops closer than their
        // radii still open the shaft along both — a single even-odd
        // path with overlapping slots would RE-KEEP the overlap
        // (odd parity) and the shaft would reappear between the arcs
        // (CodeRabbit round on #324 phase 3). The slot coordinates are
        // page-space, so the groups wrap the shape OUTSIDE its
        // transform.
        let cuts: Vec<&crate::geom::Jump> = jumps.iter().filter(|j| j.jumper == shape.id).collect();
        for (k, j) in cuts.iter().enumerate() {
            let id = if k == 0 {
                format!("dc-jumpcut{}", shape.id)
            } else {
                format!("dc-jumpcut{}-{}", shape.id, k)
            };
            // Per-crossing hop radius: the jumping SEGMENT's own axis
            // (the more-horizontal line may hop on a vertical leg —
            // Visio sizes that with the Y cells, PR #299 review). A
            // zero on that axis never reaches here: the engine drops
            // zero-axis hops before they can cluster (#324 phase 2).
            let hop_r = if j.dir.0.abs() >= j.dir.1.abs() {
                radius
            } else {
                radius_y
            };
            let _ = writeln!(
                svg,
                r#"<clipPath id="{id}"><path clip-rule="evenodd" d="{}"/></clipPath><g clip-path="url(#{id})">"#,
                jump_cut_clip_d(j, h, w, hop_r)
            );
        }
        render_shape(
            doc,
            shape,
            h,
            &[],
            &page.part,
            // A jumper renders WITHOUT its text inside the clips — the
            // label re-renders below in a twin group outside every
            // clip: the hop cuts the SHAFT, never the label's
            // background or glyphs (CodeRabbit round on #324 phase 3).
            skip_text.or_else(|| (!cuts.is_empty()).then_some(shape.id)),
            None,
            None,
            Some(&fields),
            // The overlay covers a full-page render only; scoped
            // (`only`) renders draw no jump group (#251, #294 review) —
            // but the ink strips with the page's radius either way: a
            // stored arc is stale decoration even in a scoped view.
            if only.is_none() {
                crate::geom::jump_radii_in(doc, page)
            } else {
                (0.0, 0.0)
            },
            &mut svg,
        );
        for _ in &cuts {
            let _ = writeln!(svg, "</g>");
        }
        // The jumper's label, in an IDENTICAL transform group outside
        // every clip — `emit_text` is coordinate-compatible with the
        // shape's own transform (the same reads render_shape's head
        // makes, factored into `shape_xform`).
        if !cuts.is_empty() && skip_text != Some(shape.id) {
            let xf = shape_xform(doc, shape, &[], h);
            let _ = writeln!(
                svg,
                r#"<g transform="{t}">"#,
                t = escape_attr(&xf.transform)
            );
            emit_text(
                doc,
                shape,
                &[shape],
                xf.width,
                xf.height,
                xf.sx,
                xf.sy,
                false,
                None,
                Some(&fields),
                &mut svg,
            );
            let _ = writeln!(svg, "</g>");
        }
        // The hop arcs bridge the cuts, painted WITH the connector —
        // BEFORE later shapes, so an opaque shape above covers shaft
        // and hop alike, exactly as it covers the connector's own ink
        // (Visio's paint order; the old post-ink group floated arcs on
        // top of everything — CodeRabbit round on #324 phase 3). The
        // radius is the page's own LineToLineX/Y × LineJumpFactorX/Y /
        // 2, resolved through the style chain — never the old 0.12
        // constant (#294).
        if !cuts.is_empty() {
            let _ = writeln!(svg, r#"<g data-dc="line-jumps">"#);
            for j in &cuts {
                let hop_r = if j.dir.0.abs() >= j.dir.1.abs() {
                    radius
                } else {
                    radius_y
                };
                append_jump_scaled(&mut svg, h, j, hop_r);
            }
            let _ = writeln!(svg, "</g>");
        }
    }
    let _ = writeln!(svg, "</svg>");
    Ok(svg)
}
