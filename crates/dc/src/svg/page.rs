//! Page rendering: the public page render entry points and the full
//! page assembly (chrome, background pages, the shape walk).
use std::fmt::Write as _;
use super::*;
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
        render_shape(
            doc,
            shape,
            h,
            &[],
            &page.part,
            skip_text,
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
    }
    // #187: line jumps — Visio prints hops. The export appends the
    // same treatment the canvas paints (white gap, under redraw, hop
    // arc) as a page-space group above the ink. The gap/extension
    // paddings pin the canvas's screen-px extras at 96 dpi. Scoped
    // renders (`only`) draw one shape's subtree — page-wide jumps
    // would drag in crossings from connectors outside the scope.
    // The page's Show-Line-Jumps switch gates the overlay (#251), and
    // a resolved radius of 0 (an explicit LineJumpFactorX=0, #294
    // review) draws nothing rather than degenerate zero-size hops.
    let radius = crate::geom::jump_radius_in(doc, page);
    let radius_y = crate::geom::jump_radius_y_in(doc, page);
    let jumps = if only.is_none()
        && page.line_jumps_shown()
        && (radius > 0.0 || radius_y > 0.0)
    {
        crate::geom::page_jumps(doc, page, hidden)
    } else {
        Vec::new()
    };
    // Per-crossing hop radius: the jumping SEGMENT's own axis (the
    // more-horizontal line may hop on a vertical leg — Visio sizes
    // that with the Y cells, PR #299 review). An explicit Factor=0 on
    // that axis is a zero-size hop — the crossing draws no arc at all,
    // never a degenerate one (#294 review); a page whose every hop is
    // zero-sized draws no group.
    let jumps: Vec<_> = jumps
        .into_iter()
        .filter(|j| {
            if j.dir.0.abs() >= j.dir.1.abs() {
                radius > 0.0
            } else {
                radius_y > 0.0
            }
        })
        .collect();
    if !jumps.is_empty() {
        // #294: the hop radius is the page's own LineToLineX/Y ×
        // LineJumpFactorX/Y / 2, resolved through the style chain —
        // never the old 0.12 constant.
        let _ = writeln!(svg, r#"<g data-dc="line-jumps">"#);
        for j in &jumps {
            let r = if j.dir.0.abs() >= j.dir.1.abs() {
                radius
            } else {
                radius_y
            };
            append_jump_scaled(&mut svg, h, j, r);
        }
        let _ = writeln!(svg, "</g>");
    }
    let _ = writeln!(svg, "</svg>");
    Ok(svg)
}

