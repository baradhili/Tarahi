//! The document walk — depth-first z-order traversal with clip
//! groups, and text emission as real BT…ET runs.

use pdf_writer::{Chunk, Content, Name, Str};
use resvg::usvg;
use resvg::usvg::tiny_skia_path;

use super::context::{GlyphRun, PdfContext};
use super::font::{face_upem, glyph_width_1000, shape_text, split_fallback_runs};
use super::paint::{color_rgb, set_alpha, transform_matrix};
use super::shape::{emit_image, emit_path, write_path_data};

/// Depth-first emission in document order — the z-order the SVG
/// raster paints, preserved exactly.
pub(super) fn walk_group(chunk: &mut Chunk, ctx: &mut PdfContext, content: &mut Content, group: &usvg::Group) {
    // Group opacity multiplies into every leaf's constant alpha — exact
    // for Tarahi's non-overlapping per-shape groups, an approximation
    // of a true transparency group otherwise.
    let saved = ctx.alpha;
    ctx.alpha *= group.opacity().get();
    // clip-path (rectangular gradient tiles clip to the shape outline):
    // the clip's paths, mapped through the group's absolute transform
    // and the clip's own, become one clipping path for the children.
    let clipped = group
        .clip_path()
        .is_some_and(|cp| push_clip(content, cp, group.abs_transform()));
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk_group(chunk, ctx, content, g),
            usvg::Node::Path(p) => emit_path(chunk, ctx, content, p),
            usvg::Node::Image(i) => emit_image(chunk, ctx, content, i),
            usvg::Node::Text(t) => emit_text(ctx, content, t),
        }
    }
    if clipped {
        content.restore_state();
    }
    ctx.alpha = saved;
}

/// `q` + the clip path's geometry + `W n` (even-odd when the clip rule
/// says so). Returns false — nothing pushed — for an empty clip.
fn push_clip(content: &mut Content, cp: &usvg::ClipPath, abs: tiny_skia_path::Transform) -> bool {
    fn collect(
        g: &usvg::Group,
        t: tiny_skia_path::Transform,
        out: &mut Vec<(tiny_skia_path::Path, bool)>,
    ) {
        for node in g.children() {
            match node {
                usvg::Node::Path(p) => {
                    let even_odd = p
                        .fill()
                        .is_some_and(|f| matches!(f.rule(), usvg::FillRule::EvenOdd));
                    if let Some(path) = p.data().clone().transform(t) {
                        out.push((path, even_odd));
                    }
                }
                usvg::Node::Group(sub) => collect(sub, t.pre_concat(sub.transform()), out),
                _ => {}
            }
        }
    }
    let mut paths = Vec::new();
    collect(cp.root(), abs.pre_concat(cp.transform()), &mut paths);
    if paths.is_empty() {
        return false;
    }
    content.save_state();
    let even_odd = paths.iter().any(|(_, eo)| *eo);
    for (path, _) in &paths {
        write_path_data(content, path);
    }
    if even_odd {
        content.clip_even_odd();
    } else {
        content.clip_nonzero();
    }
    content.end_path();
    true
}

/// One text node: every chunk becomes a BT…ET run of real text —
/// re-shaped with harfrust on the resolved face, glyph codes via
/// Identity-H (CID = glyph id), positioning corrections in TJ, and
/// the cluster → Unicode mapping feeding the ToUnicode CMap.
fn emit_text(ctx: &mut PdfContext, content: &mut Content, text: &usvg::Text) {
    for text_chunk in text.chunks() {
        let full = text_chunk.text();
        if full.trim().is_empty() {
            continue;
        }
        // One style per chunk in Tarahi output; multiple spans (mixed
        // styling) shape sequentially on the same baseline.
        let mut runs: Vec<GlyphRun> = Vec::new();
        for span in text_chunk.spans() {
            if span.fill().is_none() {
                continue; // fill="none": invisible in the raster too
            }
            let start = span.start().min(full.len());
            let end = span.end().min(full.len());
            let Some(sub) = (start <= end).then(|| full.get(start..end)).flatten() else {
                continue;
            };
            if sub.is_empty() {
                continue;
            }
            let Some(primary) = ctx.query_face(span.font()) else {
                continue;
            };
            let Some(font) = ctx.shaper_font(primary) else {
                continue;
            };
            let shaped = shape_text(&font, sub);
            runs.extend(split_fallback_runs(ctx, primary, sub, shaped));
        }
        if runs.is_empty() {
            continue;
        }
        // The anchor offset: chunk x is the DECLARED anchor point;
        // usvg applies middle/end shifts when flattening, so the PDF
        // does the same here — total advance of the whole line in ems,
        // scaled to the chunk's font size (viewBox units) before it
        // moves the inches-space anchor.
        let total_em: f32 = runs
            .iter()
            .map(|run| {
                ctx.face_data(run.face)
                    .map(|(d, i)| {
                        let upem = face_upem(&d, i);
                        run.glyphs.iter().map(|g| g.x_advance).sum::<f32>() / upem
                    })
                    .unwrap_or(0.0)
            })
            .sum();
        // Font size in user units (inches — the viewBox unit).
        let font_size = text_chunk
            .spans()
            .first()
            .map(|s| s.font_size().get())
            .unwrap_or(0.1);
        let anchor_shift = match text_chunk.anchor() {
            usvg::TextAnchor::Middle => -total_em * font_size / 2.0,
            usvg::TextAnchor::End => -total_em * font_size,
            usvg::TextAnchor::Start => 0.0,
        };
        // The node's absolute transform rides as a cm (like paths) so
        // the font size stays in viewBox units; glyph outlines are
        // y-up while the SVG frame is y-down, hence the Tm flip.
        let node = transform_matrix(text.abs_transform());
        let local_x = text_chunk.x().unwrap_or(0.0) + anchor_shift;
        let local_y = text_chunk.y().unwrap_or(0.0);
        // The paint: first span's fill (uniform in Tarahi output).
        let first_fill = text_chunk.spans().first().and_then(|s| s.fill());
        let [pr, pg, pb] = first_fill
            .map(|f| match f.paint() {
                usvg::Paint::Color(c) => color_rgb(*c),
                _ => [0.0, 0.0, 0.0],
            })
            .unwrap_or([0.0, 0.0, 0.0]);
        content.save_state().transform(node);
        set_alpha(
            ctx,
            content,
            first_fill.map_or(1.0, |f| f.opacity().get()),
            1.0,
        );
        content
            .begin_text()
            .set_fill_rgb(pr, pg, pb)
            .set_text_matrix([1.0, 0.0, 0.0, -1.0, local_x, local_y]);
        for run in &runs {
            let Some(refs) = ctx.face_refs(run.face) else {
                continue;
            };
            let Some((data, index)) = ctx.face_data(run.face) else {
                continue;
            };
            let Ok(tt) = ttf_parser::Face::parse(&data, index) else {
                continue;
            };
            let upem = face_upem(&data, index);
            content.set_font(Name(refs.res_name.as_bytes()), font_size);
            let mut positioned = content.show_positioned();
            let mut items = positioned.items();
            // RTL runs emit glyphs in VISUAL order: cluster values
            // DECREASE along the run, so "next glyph's cluster" is
            // BELOW this one and the old range came out empty —
            // Hebrew/Arabic glyphs got no ToUnicode entry and their
            // text could not be selected or copied (#218 review).
            // The cluster's chars end at the SMALLEST cluster above
            // this glyph's (text end when there is none).
            if let Some(first) = run.glyphs.first() {
                let off = first.x_offset * 1000.0 / upem;
                if off.abs() > 1e-3 {
                    // The first glyph's own offset applies before it
                    // is shown (the loop only carries offsets forward).
                    items.adjust(-off);
                }
            }
            for (i, g) in run.glyphs.iter().enumerate() {
                // Cluster chars → ToUnicode (multi-char for ligatures).
                let end = run
                    .glyphs
                    .iter()
                    .map(|n| n.cluster)
                    .filter(|&c| c > g.cluster)
                    .min()
                    .unwrap_or(run.text.len())
                    .min(run.text.len());
                let chars: Vec<char> = run
                    .text
                    .get(g.cluster.min(end)..end)
                    .map(|s| s.chars().collect())
                    .unwrap_or_default();
                if g.id != 0 {
                    ctx.widths
                        .entry(run.face)
                        .or_default()
                        .insert(g.id, glyph_width_1000(&tt, g.id));
                    if !chars.is_empty() {
                        ctx.to_unicode
                            .entry(run.face)
                            .or_default()
                            .entry(g.id)
                            .or_insert(chars.clone());
                    }
                }
                // Show: 2-byte big-endian glyph id (Identity-H).
                let bytes = [(g.id >> 8) as u8, g.id as u8];
                items.show(Str(bytes.as_slice()));
                // Kerning correction: shaped advance vs the /W nominal,
                // plus the OFFSET DIFFERENCE to the next glyph — a
                // per-glyph x_offset must affect only its own glyph,
                // so this one's is cancelled and the next's applied
                // (#218 review; marks/diacritics otherwise shift every
                // later glyph in the run). Thousandths of the font
                // size, subtracted from the pen.
                let nominal = glyph_width_1000(&tt, g.id);
                let shaped1000 = g.x_advance * 1000.0 / upem;
                let this_offset = g.x_offset * 1000.0 / upem;
                let next_offset =
                    run.glyphs.get(i + 1).map(|n| n.x_offset).unwrap_or(0.0) * 1000.0 / upem;
                let adjust = nominal - shaped1000 - (next_offset - this_offset);
                if adjust.abs() > 1e-3 {
                    items.adjust(adjust);
                }
            }
        }
        content.end_text().restore_state();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster;
    use pdf_writer::Pdf;

    /// Fill/stroke opacity (Visio *Trans cells, TextBkgndTrans) and
    /// group opacity reach the PDF as constant-alpha graphics states;
    /// opaque paint selects none.
    #[test]
    fn opacity_selects_a_constant_alpha_gstate() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="5" height="5" fill="#ff0000"/><rect x="6" width="5" height="5" fill="#ff0000" fill-opacity="0.5" stroke="#000" stroke-opacity="0.25"/><g opacity="0.5"><rect x="12" width="5" height="5" fill="#00f" fill-opacity="0.5"/></g></svg>"##;
        let db = raster::font_db();
        let tree = raster::parse_svg(svg, &db).expect("svg parses");
        let mut pdf = Pdf::new();
        let mut ctx = PdfContext::new(&db);
        let mut content = Content::new();
        walk_group(&mut pdf, &mut ctx, &mut content, tree.root());
        let buf = String::from_utf8(content.finish().to_vec()).unwrap();
        let keys: Vec<(u16, u16)> = ctx.gstate_order.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, vec![(500, 250), (250, 500)], "{buf}");
        assert_eq!(
            buf.matches(" gs").count(),
            2,
            "opaque rect selects no gstate: {buf}"
        );
    }

    /// A clip-path group (rectangular gradient tiles) pushes one
    /// clipping path around its children; stroke gradients select a
    /// shading pattern instead of the old gray.
    #[test]
    fn clip_paths_and_stroke_gradients_reach_the_pdf() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="10" y2="0"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient></defs><clipPath id="c"><path clip-rule="evenodd" d="M 1 1 L 9 1 L 9 9 Z"/></clipPath><g clip-path="url(#c)"><rect width="10" height="10" fill="#0f0"/></g><path d="M 0 5 L 20 5" stroke="url(#g)" stroke-width="2" fill="none"/></svg>"##;
        let db = raster::font_db();
        let tree = raster::parse_svg(svg, &db).expect("svg parses");
        let mut pdf = Pdf::new();
        let mut ctx = PdfContext::new(&db);
        let mut content = Content::new();
        walk_group(&mut pdf, &mut ctx, &mut content, tree.root());
        let buf = String::from_utf8(content.finish().to_vec()).unwrap();
        assert!(buf.contains("W* n") || buf.contains("W*\nn"), "{buf}");
        assert!(buf.contains("/Pattern CS"), "stroke shading: {buf}");
    }
}
