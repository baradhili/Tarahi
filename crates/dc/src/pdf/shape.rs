//! Shape nodes — paths as vector path operators and images as `Do`
//! XObject draws.

use pdf_writer::{Chunk, Content, Name};
use resvg::usvg;
use resvg::usvg::tiny_skia_path;

use super::context::PdfContext;
use super::paint::{set_alpha, set_fill_paint, set_stroke_paint, transform_matrix};
use super::resources::register_image;

/// One path node: q / cm(abs) / state / path / op / Q.
pub(super) fn emit_path(chunk: &mut Chunk, ctx: &mut PdfContext, content: &mut Content, p: &usvg::Path) {
    if !p.is_visible() {
        return;
    }
    let fill = p.fill();
    let stroke = p.stroke();
    if fill.is_none() && stroke.is_none() {
        return;
    }
    content
        .save_state()
        .transform(transform_matrix(p.abs_transform()));
    set_alpha(
        ctx,
        content,
        fill.map_or(1.0, |f| f.opacity().get()),
        stroke.map_or(1.0, |s| s.opacity().get()),
    );
    if let Some(f) = fill {
        set_fill_paint(chunk, ctx, content, f, p.abs_transform());
    }
    if let Some(s) = stroke {
        set_stroke_paint(chunk, ctx, content, s, p.abs_transform());
    }
    write_path_data(content, p.data());
    // Our renderer never combines fill+stroke on one element (a path
    // carries either fill= or stroke=), but the SVG model allows it —
    // fill first, Visio's paint order.
    match (fill, stroke) {
        (Some(f), Some(_)) => {
            if matches!(f.rule(), usvg::FillRule::EvenOdd) {
                content.fill_even_odd_and_stroke();
            } else {
                content.fill_nonzero_and_stroke();
            }
        }
        (Some(f), None) => {
            if matches!(f.rule(), usvg::FillRule::EvenOdd) {
                content.fill_even_odd();
            } else {
                content.fill_nonzero();
            }
        }
        (None, Some(_)) => {
            content.stroke();
        }
        (None, None) => {
            content.end_path();
        }
    }
    content.restore_state();
}

pub(super) fn write_path_data(content: &mut Content, path: &tiny_skia_path::Path) {
    let mut cur = (0.0f32, 0.0f32);
    let mut started = false;
    for seg in path.segments() {
        match seg {
            tiny_skia_path::PathSegment::MoveTo(pt) => {
                content.move_to(pt.x, pt.y);
                cur = (pt.x, pt.y);
                started = true;
            }
            tiny_skia_path::PathSegment::LineTo(pt) => {
                content.line_to(pt.x, pt.y);
                cur = (pt.x, pt.y);
            }
            tiny_skia_path::PathSegment::QuadTo(ctrl, to) => {
                // pdf-writer has no quad op: elevate to a cubic — the
                // standard 2/3 de Casteljau control points.
                let (p0x, p0y) = cur;
                let c1x = p0x + 2.0 / 3.0 * (ctrl.x - p0x);
                let c1y = p0y + 2.0 / 3.0 * (ctrl.y - p0y);
                let c2x = to.x + 2.0 / 3.0 * (ctrl.x - to.x);
                let c2y = to.y + 2.0 / 3.0 * (ctrl.y - to.y);
                content.cubic_to(c1x, c1y, c2x, c2y, to.x, to.y);
                cur = (to.x, to.y);
            }
            tiny_skia_path::PathSegment::CubicTo(c1, c2, to) => {
                content.cubic_to(c1.x, c1.y, c2.x, c2.y, to.x, to.y);
                cur = (to.x, to.y);
            }
            tiny_skia_path::PathSegment::Close => {
                content.close_path();
            }
        }
    }
    if !started {
        content.end_path();
    }
}

/// One image node: q / cm(abs ∘ bitmap scale) / Do / Q. The parser
/// bakes the SVG x/y/width/height into the absolute transform; the
/// drawing space is the bitmap's own pixel size.
pub(super) fn emit_image(chunk: &mut Chunk, ctx: &mut PdfContext, content: &mut Content, img: &usvg::Image) {
    if !img.is_visible() {
        return;
    }
    let Some(name) = register_image(chunk, ctx, img.kind()) else {
        return;
    };
    let size = img.size();
    let t = img.abs_transform();
    // A ∘ [w 0 0 −h 0 h] (#218 review): PDF image space renders the
    // unit square with the FIRST sample row at y = 1, and the page
    // content's base transform is y-down — without the vertical flip
    // every picture exports mirrored top to bottom. The composition:
    // linear = A ∘ diag(w, −h); the S' translation [0, h] lands as
    // A.linear · [0, h] + A.trans.
    let (w, h) = (size.width(), size.height());
    let m = [
        t.sx * w,
        t.ky * w,
        -t.kx * h,
        -t.sy * h,
        t.kx * h + t.tx,
        t.sy * h + t.ty,
    ];
    content.save_state().transform(m);
    set_alpha(ctx, content, 1.0, 1.0);
    content.x_object(Name(name.as_bytes()));
    content.restore_state();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster;
    use pdf_writer::Pdf;

    /// A 2×2 RGBA PNG: row 0 (top) red, row 1 (bottom) blue.
    fn two_row_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        img.put_pixel(1, 1, image::Rgba([0, 0, 255, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// #218 review: the image matrix composes the vertical flip —
    /// the first sample row lands at the rect's TOP edge in the
    /// y-down content frame, not the bottom (every picture exported
    /// upside down).
    #[test]
    fn image_matrix_flips_the_first_row_to_the_top() {
        let png = two_row_png();
        let b64 = crate::svg::base64(&png);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="10"><image x="0" y="0" width="20" height="10" xlink:href="data:image/png;base64,{b64}"/></svg>"#
        );
        let db = raster::font_db();
        let tree = raster::parse_svg(&svg, &db).expect("svg parses");
        // The single Image node under the root group.
        fn find_image(g: &usvg::Group) -> Option<&usvg::Image> {
            g.children().iter().find_map(|n| match n {
                usvg::Node::Image(img) => Some(img.as_ref()),
                usvg::Node::Group(sub) => find_image(sub),
                _ => None,
            })
        }
        let img = find_image(tree.root()).expect("the svg carries the image");

        let mut pdf = Pdf::new();
        let mut ctx = PdfContext::new(&db);
        let mut content = Content::new();
        emit_image(&mut pdf, &mut ctx, &mut content, img);
        let buf = String::from_utf8(content.finish().to_vec()).unwrap();
        // A ∘ [w 0 0 −h 0 h]: with an axis-aligned image the cm's
        // matrix is [w' 0 0 −h' e' h'] — d NEGATIVE with f = −d (the
        // first sample row lands at the rect's top edge). The pre-fix
        // matrix read `[w' 0 0 h' 0 0]`: row 0 at the bottom.
        let cm = buf
            .lines()
            .find(|l| l.ends_with(" cm"))
            .expect("the image emitted a cm");
        let m: Vec<f32> = cm
            .split(' ')
            .filter_map(|t| t.parse().ok())
            .collect();
        assert_eq!(m.len(), 6, "a cm matrix: {cm:?}");
        assert!(
            m[0] > 0.0 && m[1] == 0.0 && m[2] == 0.0,
            "axis-aligned image: {cm:?}"
        );
        assert!(
            m[3] < 0.0 && (m[5] - (-m[3])).abs() < 1e-3,
            "the image matrix carries the vertical flip (d<0, f=−d): {cm:?}"
        );
    }
}
