//! Paint state — transform/color helpers, cap and join mapping, the
//! constant-alpha graphics states, and the fill/stroke paint
//! selectors.

use pdf_writer::types::{LineCapStyle, LineJoinStyle};
use pdf_writer::{Chunk, Content, Name};
use resvg::usvg;
use resvg::usvg::tiny_skia_path;

use super::context::PdfContext;
use super::resources::register_gradient;

/// usvg's absolute transform as a PDF matrix (same six coefficients).
pub(super) fn transform_matrix(t: tiny_skia_path::Transform) -> [f32; 6] {
    [t.sx, t.ky, t.kx, t.sy, t.tx, t.ty]
}

/// A ∘ B: B maps a point first, then A (PDF cm / usvg convention).
pub(super) fn compose(a: tiny_skia_path::Transform, b: tiny_skia_path::Transform) -> tiny_skia_path::Transform {
    tiny_skia_path::Transform::from_row(
        a.sx * b.sx + a.kx * b.ky,
        a.ky * b.sx + a.sy * b.ky,
        a.sx * b.kx + a.kx * b.sy,
        a.ky * b.kx + a.sy * b.sy,
        a.sx * b.tx + a.kx * b.ty + a.tx,
        a.ky * b.tx + a.sy * b.ty + a.ty,
    )
}

/// usvg y-down color → PDF rgb 0..1.
pub(super) fn color_rgb(c: usvg::Color) -> [f32; 3] {
    [
        f32::from(c.red) / 255.0,
        f32::from(c.green) / 255.0,
        f32::from(c.blue) / 255.0,
    ]
}

pub(super) fn cap_pdf(cap: usvg::LineCap) -> LineCapStyle {
    match cap {
        usvg::LineCap::Butt => LineCapStyle::ButtCap,
        usvg::LineCap::Round => LineCapStyle::RoundCap,
        usvg::LineCap::Square => LineCapStyle::ProjectingSquareCap,
    }
}

pub(super) fn join_pdf(join: usvg::LineJoin) -> LineJoinStyle {
    match join {
        usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoinStyle::MiterJoin,
        usvg::LineJoin::Round => LineJoinStyle::RoundJoin,
        usvg::LineJoin::Bevel => LineJoinStyle::BevelJoin,
    }
}

/// Select the constant fill/stroke alpha (`/ca`, `/CA`) for the
/// following paint operators — SVG `fill-opacity`/`stroke-opacity`
/// (Visio's *Trans cells) and inherited group opacity. Opaque paint
/// emits nothing, so fully opaque pages stay byte-identical.
pub(super) fn set_alpha(ctx: &mut PdfContext, content: &mut Content, fill: f32, stroke: f32) {
    let q = |a: f32| (a.clamp(0.0, 1.0) * 1000.0).round() as u16;
    let key = (q(fill * ctx.alpha), q(stroke * ctx.alpha));
    if key == (1000, 1000) {
        return;
    }
    let name = match ctx.gstates.get(&key) {
        Some(n) => *n,
        None => {
            let r = ctx.alloc_ref();
            let n = PdfContext::leak_name("Gs", ctx.gstate_order.len() as u32 + 1);
            ctx.gstates.insert(key, n);
            ctx.gstate_order.push((key, r, n));
            n
        }
    };
    content.set_parameters(Name(name.as_bytes()));
}

pub(super) fn set_fill_paint(
    chunk: &mut Chunk,
    ctx: &mut PdfContext,
    content: &mut Content,
    f: &usvg::Fill,
    abs: tiny_skia_path::Transform,
) {
    match f.paint() {
        usvg::Paint::Color(c) => {
            let [r, g, b] = color_rgb(*c);
            content.set_fill_rgb(r, g, b);
        }
        usvg::Paint::LinearGradient(_) | usvg::Paint::RadialGradient(_) => {
            let extra = compose(ctx.base, abs);
            match register_gradient(chunk, ctx, f.paint(), extra) {
                Some(name) => {
                    content.set_fill_color_space(pdf_writer::types::ColorSpaceOperand::Pattern);
                    content.set_fill_pattern(std::iter::empty(), Name(name.as_bytes()));
                }
                // No stops to shade with: neutral gray, never
                // white-on-white.
                None => {
                    content.set_fill_rgb(0.5, 0.5, 0.5);
                }
            }
        }
        // Pattern fills never occur in Tarahi output.
        usvg::Paint::Pattern(_) => {
            content.set_fill_rgb(0.5, 0.5, 0.5);
        }
    }
}

pub(super) fn set_stroke_paint(
    chunk: &mut Chunk,
    ctx: &mut PdfContext,
    content: &mut Content,
    s: &usvg::Stroke,
    abs: tiny_skia_path::Transform,
) {
    match s.paint() {
        usvg::Paint::Color(c) => {
            let [r, g, b] = color_rgb(*c);
            content.set_stroke_rgb(r, g, b);
        }
        // LineGradientEnabled strokes: the same shading patterns as
        // gradient fills.
        usvg::Paint::LinearGradient(_) | usvg::Paint::RadialGradient(_) => {
            let extra = compose(ctx.base, abs);
            match register_gradient(chunk, ctx, s.paint(), extra) {
                Some(name) => {
                    content.set_stroke_color_space(pdf_writer::types::ColorSpaceOperand::Pattern);
                    content.set_stroke_pattern(std::iter::empty(), Name(name.as_bytes()));
                }
                None => {
                    content.set_stroke_rgb(0.5, 0.5, 0.5);
                }
            }
        }
        usvg::Paint::Pattern(_) => {
            content.set_stroke_rgb(0.5, 0.5, 0.5);
        }
    }
    content.set_line_width(s.width().get());
    content.set_line_cap(cap_pdf(s.linecap()));
    content.set_line_join(join_pdf(s.linejoin()));
    content.set_miter_limit(s.miterlimit().get());
    if let Some(dash) = s.dasharray() {
        content.set_dash_pattern(dash.to_vec(), 0.0);
    }
}
