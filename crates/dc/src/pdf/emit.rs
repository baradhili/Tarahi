//! File assembly — the per-page walk loop, and the object-graph
//! assembly that turns the walk's allocated refs into the finished
//! PDF bytes.

use pdf_writer::types::{CidFontType, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Name, Pdf, Ref, Str};
use resvg::usvg;
use resvg::usvg::tiny_skia_path;

use crate::raster;
use crate::svg;

use super::context::PdfContext;
use super::font::{face_bytes, face_content_name, flags_of, global_bbox};
use super::paint::transform_matrix;
use super::resources::flate;
use super::walk::walk_group;

/// Export the selected pages (0-based) of `doc` to PDF bytes.
///
/// `transparent` drops the white page fill (viewers show paper white
/// by convention; alpha-savvy pipelines honor the empty page).
pub fn pdf_bytes(
    doc: &vsdx::Document,
    pages: &[usize],
    transparent: bool,
) -> anyhow::Result<Vec<u8>> {
    if pages.is_empty() {
        anyhow::bail!("no pages selected");
    }
    let db = raster::font_db();
    // Pdf derefs to its Chunk, so every emitter writes through the
    // same object graph the trailer finishes.
    let mut pdf = Pdf::new();
    let mut ctx = PdfContext::new(&db);
    let mut page_data = Vec::new();
    let mut trees: Vec<usvg::Tree> = Vec::new();

    for &idx in pages {
        let page = doc
            .pages
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("page {} out of range", idx + 1))?;
        let w_in = num_of(page, "PageWidth").unwrap_or(8.5);
        let h_in = num_of(page, "PageHeight").unwrap_or(11.0);
        let page_svg = svg::render_page_export(doc, page, transparent)?;
        let tree = raster::parse_svg(&page_svg, &db)?;
        let mut content = Content::new();
        // Base: the SVG's user units land as PIXELS in the tree (usvg
        // bakes the viewBox→px scale — PX_PER_IN — into every node's
        // absolute transform), y-down; PDF points, y-up. 72/96 maps
        // the tree's px to points. The gradient pattern matrix needs
        // the same mapping (it ignores the CTM at paint time).
        let base = tiny_skia_path::Transform::from_row(
            72.0 / 96.0,
            0.0,
            0.0,
            -72.0 / 96.0,
            0.0,
            h_in as f32 * 72.0,
        );
        content.transform(transform_matrix(base));
        ctx.base = base;
        walk_group(&mut pdf, &mut ctx, &mut content, tree.root());
        // The image/gradient registries key by tree-object addresses:
        // every tree must OUTLIVE them, or the allocator can hand a
        // freed address to the next page's image and alias its
        // XObject (#218 review).
        trees.push(tree);
        let buf = content.finish();
        page_data.push(PageData {
            content: buf.as_slice().to_vec(),
            width_pt: w_in as f32 * 72.0,
            height_pt: h_in as f32 * 72.0,
        });
    }

    assemble(pdf, ctx, page_data)
}

fn num_of(page: &vsdx::Page, name: &str) -> Option<f64> {
    page.sheet()
        .and_then(|s| s.cell(name))
        .and_then(|c| c.numeric())
}

/// Per-page assembled state.
struct PageData {
    content: Vec<u8>,
    width_pt: f32,
    height_pt: f32,
}

/// Object assembly: fonts, images, shadings, page tree, catalog —
/// everything the walk phase allocated refs for gets its dict here.
fn assemble(
    mut pdf: Pdf,
    mut ctx: PdfContext<'_>,
    pages: Vec<PageData>,
) -> anyhow::Result<Vec<u8>> {
    let catalog = ctx.alloc_ref();
    let pages_root = ctx.alloc_ref();
    let page_refs: Vec<Ref> = (0..pages.len()).map(|_| ctx.alloc_ref()).collect();
    let content_refs: Vec<Ref> = (0..pages.len()).map(|_| ctx.alloc_ref()).collect();
    let resources = ctx.alloc_ref();

    pdf.catalog(catalog).pages(pages_root);
    {
        let mut pages_writer = pdf.pages(pages_root);
        pages_writer.kids(page_refs.iter().copied());
        pages_writer.count(pages.len() as i32);
    }

    for ((page, page_ref), content_ref) in pages.iter().zip(&page_refs).zip(&content_refs) {
        let mut p = pdf.page(*page_ref);
        p.parent(pages_root);
        p.media_box(pdf_writer::Rect::new(
            0.0,
            0.0,
            page.width_pt,
            page.height_pt,
        ));
        p.contents(*content_ref);
        // The shared resources dict, by reference (fonts/images/
        // patterns are global — one font embedding per face).
        p.pair(Name(b"Resources"), resources);
    }

    // The shared resources dict: /Font, /XObject, /Pattern, /ProcSet.
    {
        let mut res = pdf.indirect(resources).dict();
        if !ctx.face_order.is_empty() {
            let mut fonts = res.insert(Name(b"Font")).dict();
            for key in &ctx.face_order {
                if let Some(refs) = ctx.faces.get(key) {
                    fonts.pair(Name(refs.res_name.as_bytes()), refs.type0);
                }
            }
        }
        if !ctx.image_order.is_empty() {
            let mut xobjects = res.insert(Name(b"XObject")).dict();
            for key in &ctx.image_order {
                if let Some(img) = ctx.images.get(key) {
                    xobjects.pair(Name(img.name.as_bytes()), img.object);
                }
            }
        }
        if !ctx.gstate_order.is_empty() {
            let mut gs = res.insert(Name(b"ExtGState")).dict();
            for (_, r, n) in &ctx.gstate_order {
                gs.pair(Name(n.as_bytes()), *r);
            }
        }
        if !ctx.shading_order.is_empty() {
            let mut patterns = res.insert(Name(b"Pattern")).dict();
            for key in &ctx.shading_order {
                if let Some(s) = ctx.shadings.get(key) {
                    patterns.pair(Name(s.name.as_bytes()), s.pattern);
                }
            }
        }
        let mut procset = res.insert(Name(b"ProcSet")).array();
        procset.items([
            Name(b"PDF"),
            Name(b"Text"),
            Name(b"ImageB"),
            Name(b"ImageC"),
            Name(b"ImageI"),
        ]);
    }

    // The constant-alpha graphics states.
    for ((fill, stroke), r, _) in ctx.gstate_order.clone() {
        pdf.ext_graphics(r)
            .non_stroking_alpha(f32::from(fill) / 1000.0)
            .stroking_alpha(f32::from(stroke) / 1000.0);
    }

    // Content streams (Flate-compressed).
    for (page, content_ref) in pages.iter().zip(&content_refs) {
        let compressed = flate(&page.content);
        let mut s = pdf.stream(*content_ref, &compressed);
        s.filter(Filter::FlateDecode);
    }

    // Fonts: Type0 + CID + descriptor + file + ToUnicode.
    for key in ctx.face_order.clone() {
        let Some(refs) = ctx.faces.get(&key).cloned() else {
            continue;
        };
        let Some((data, index)) = ctx.face_data(key) else {
            continue;
        };
        let tt = ttf_parser::Face::parse(&data, index)
            .map_err(|e| anyhow::anyhow!("parsing font for embedding: {e}"))?;
        let upem = f32::from(tt.units_per_em());
        let true_type = tt.tables().cff.is_none();
        let base = face_content_name(&refs.postscript);
        {
            let mut type0 = pdf.type0_font(refs.type0);
            type0.base_font(base);
            type0.encoding_predefined(Name(b"Identity-H"));
            type0.descendant_font(refs.cid);
            type0.to_unicode(refs.to_unicode);
        }
        {
            let mut cid = pdf.cid_font(refs.cid);
            cid.subtype(if true_type {
                CidFontType::Type2
            } else {
                CidFontType::Type0
            });
            cid.base_font(base);
            cid.system_info(SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            });
            cid.font_descriptor(refs.descriptor);
            cid.default_width(0.0);
            cid.cid_to_gid_map_predefined(Name(b"Identity"));
            // /W: consecutive runs over the glyphs the document used.
            let mut w = cid.widths();
            if let Some(widths) = ctx.widths.get(&key) {
                let mut iter = widths.iter().peekable();
                while let Some((gid, width)) = iter.next() {
                    let mut run = vec![*width];
                    let start = *gid;
                    while let Some((next, nw)) = iter.peek() {
                        if **next == start + run.len() as u16 {
                            run.push(**nw);
                            iter.next();
                        } else {
                            break;
                        }
                    }
                    w.consecutive(start, run);
                }
            }
        }
        {
            let mut d = pdf.font_descriptor(refs.descriptor);
            d.name(base);
            d.flags(flags_of(&tt));
            d.bbox(global_bbox(&tt));
            d.italic_angle(tt.italic_angle());
            d.ascent(f32::from(tt.ascender()) * 1000.0 / upem);
            d.descent(f32::from(tt.descender()) * 1000.0 / upem);
            d.cap_height(
                tt.capital_height()
                    .map(|c| f32::from(c) * 1000.0 / upem)
                    .unwrap_or(700.0),
            );
            d.stem_v(10.0);
            if true_type {
                d.font_file2(refs.file);
            } else {
                d.font_file3(refs.file);
            }
        }
        // The font file stream: /Length1 for TrueType (required by
        // readers), /Subtype OpenType for CFF containers. A font
        // COLLECTION (.ttc/.otc — the CJK fallback shape on most
        // hosts) is not a font program: slice out the face the
        // offset table names (#218 review).
        let embed = face_bytes(&data, index);
        // Flate the font program (#218 review): fonts are the fat tail
        // of an export's size, and like every other stream they
        // decompress transparently. Length1 stays the UNCOMPRESSED
        // length — that is its defined meaning for FontFile2.
        let compressed = flate(&embed);
        {
            let mut s = pdf.stream(refs.file, &compressed);
            s.filter(Filter::FlateDecode);
            if true_type {
                s.pair(Name(b"Length1"), embed.len() as i32);
            } else {
                s.pair(Name(b"Subtype"), Name(b"OpenType"));
            }
        }
        // ToUnicode CMap from the collected glyph → text pairs.
        let mut cmap = UnicodeCmap::new(
            Name(b"TarahiToUnicode"),
            SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            },
        );
        if let Some(map) = ctx.to_unicode.get(&key) {
            for (gid, chars) in map {
                cmap.pair_with_multiple(*gid, chars.iter().copied());
            }
        }
        let cmap_bytes = cmap.finish();
        // pdf-writer's `.filter()` only DECLares the filter — the
        // bytes must arrive already compressed.
        let cmap_compressed = flate(cmap_bytes.as_slice());
        {
            let mut cmap_stream = pdf.cmap(refs.to_unicode, &cmap_compressed);
            cmap_stream.filter(Filter::FlateDecode);
        }
    }

    Ok(pdf.finish())
}
