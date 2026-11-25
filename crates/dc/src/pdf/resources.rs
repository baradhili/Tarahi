//! Shared PDF resources — images as Flate-compressed XObjects and
//! gradients as axial/radial shading patterns, deduped through the
//! context registries.

use std::sync::Arc;

use pdf_writer::types::FunctionShadingType;
use pdf_writer::{Chunk, Filter, Finish, Name, Ref};
use resvg::usvg;
use resvg::usvg::tiny_skia_path;

use super::context::{ImageEntry, PdfContext, ShadeEntry};
use super::paint::{color_rgb, compose, transform_matrix};

/// Flate-compress raw bytes — PDF's FlateDecode is zlib-wrapped
/// deflate, so the encoder is `ZlibEncoder` (not the raw one).
pub(super) fn flate(raw: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = enc.write_all(raw);
    enc.finish().unwrap_or_else(|_| raw.to_vec())
}

/// Encode and register one image's bytes as an XObject (deduped by
/// the encoded-bytes identity): raw RGB samples + an alpha soft mask
/// when any pixel is translucent, both Flate-compressed.
pub(super) fn register_image(
    chunk: &mut Chunk,
    ctx: &mut PdfContext,
    kind: &usvg::ImageKind,
) -> Option<&'static str> {
    let bytes: &Arc<Vec<u8>> = match kind {
        usvg::ImageKind::PNG(b)
        | usvg::ImageKind::JPEG(b)
        | usvg::ImageKind::GIF(b)
        | usvg::ImageKind::WEBP(b) => b,
        // Nested SVG images never occur in Tarahi output.
        usvg::ImageKind::SVG(_) => return None,
    };
    let key = Arc::as_ptr(bytes) as *const u8 as usize;
    if let Some(hit) = ctx.images.get(&key) {
        return Some(hit.name);
    }
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    let opaque = img.pixels().all(|p| p.0[3] == 255);
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    let mut alpha = Vec::with_capacity(w as usize * h as usize);
    for p in img.pixels() {
        rgb.extend_from_slice(&[p.0[0], p.0[1], p.0[2]]);
        alpha.push(p.0[3]);
    }
    let smask = if opaque {
        None
    } else {
        let r = ctx.alloc_ref();
        let data = flate(&alpha);
        let mut sm = chunk.image_xobject(r, &data);
        sm.width(w as i32)
            .height(h as i32)
            .color_space_name(Name(b"DeviceGray"))
            .bits_per_component(8);
        sm.filter(Filter::FlateDecode).finish();
        Some(r)
    };
    let object = ctx.alloc_ref();
    let name = PdfContext::leak_name("Im", ctx.image_counter);
    let data = flate(&rgb);
    let mut xobj = chunk.image_xobject(object, &data);
    xobj.width(w as i32)
        .height(h as i32)
        .color_space_name(Name(b"DeviceRGB"))
        .bits_per_component(8)
        .interpolate(true);
    if let Some(sm) = smask {
        xobj.s_mask(sm);
    }
    xobj.filter(Filter::FlateDecode).finish();
    ctx.image_counter += 1;
    ctx.images.insert(key, ImageEntry { object, name });
    ctx.image_order.push(key);
    Some(name)
}

/// Build the multi-stop stitching function of a gradient (an
/// exponential segment per adjacent stop pair, bounds at the stops).
fn write_stitch_function(
    chunk: &mut Chunk,
    ctx: &mut PdfContext,
    stops: &[usvg::Stop],
) -> Option<Ref> {
    if stops.len() < 2 {
        return None;
    }
    let mut segments = Vec::new();
    for pair in stops.windows(2) {
        let r = ctx.alloc_ref();
        let (a, b) = (color_rgb(pair[0].color()), color_rgb(pair[1].color()));
        let mut f = chunk.exponential_function(r);
        // The codomain is RGB: one 0..1 dimension per channel.
        f.domain([0.0, 1.0])
            .range([0.0, 1.0, 0.0, 1.0, 0.0, 1.0])
            .n(1.0);
        f.c0([a[0], a[1], a[2]]);
        f.c1([b[0], b[1], b[2]]);
        segments.push(r);
    }
    let top = ctx.alloc_ref();
    let bounds: Vec<f32> = stops[1..stops.len() - 1]
        .iter()
        .map(|s| s.offset().get())
        .collect();
    let encode: Vec<f32> = (0..segments.len()).flat_map(|_| [0.0, 1.0]).collect();
    let mut f = chunk.stitching_function(top);
    // Type 3 requires /Domain like the segments it stitches.
    f.domain([0.0, 1.0]);
    f.functions(segments).bounds(bounds).encode(encode);
    Some(top)
}

/// Register one gradient's shading pattern. usvg has already resolved
/// objectBoundingBox units into element user space at parse time (its
/// `units` field is parse-only), so the pattern matrix is the
/// gradient's own transform and the coordinates are element-local.
pub(super) fn register_gradient(
    chunk: &mut Chunk,
    ctx: &mut PdfContext,
    paint: &usvg::Paint,
    extra: tiny_skia_path::Transform,
) -> Option<&'static str> {
    let ptr = match paint {
        usvg::Paint::LinearGradient(g) => Arc::as_ptr(g) as *const u8 as usize,
        usvg::Paint::RadialGradient(g) => Arc::as_ptr(g) as *const u8 as usize,
        // Pattern fills never occur in Tarahi output.
        _ => return None,
    };
    // The gradient's own transform composes after base ∘ abs: the
    // pattern's /Matrix maps pattern space to DEFAULT user space and
    // ignores the CTM at fill time (#218 review). Paths sharing one
    // gradient under different transforms need distinct patterns.
    let matrix = match paint {
        usvg::Paint::LinearGradient(g) => compose(extra, g.transform()),
        usvg::Paint::RadialGradient(g) => compose(extra, g.transform()),
        _ => unreachable!(),
    };
    let key = (ptr, transform_matrix(matrix).map(f32::to_bits));
    if let Some(hit) = ctx.shadings.get(&key) {
        return Some(hit.name);
    }
    let function = match paint {
        usvg::Paint::LinearGradient(g) => write_stitch_function(chunk, ctx, g.stops()),
        usvg::Paint::RadialGradient(g) => write_stitch_function(chunk, ctx, g.stops()),
        _ => None,
    }?;
    let radial = matches!(paint, usvg::Paint::RadialGradient(_));
    let shading = ctx.alloc_ref();
    let pattern = ctx.alloc_ref();
    let coords = match paint {
        usvg::Paint::LinearGradient(g) => vec![g.x1(), g.y1(), g.x2(), g.y2()],
        usvg::Paint::RadialGradient(g) => vec![g.fx(), g.fy(), 0.0, g.cx(), g.cy(), g.r().get()],
        _ => unreachable!(),
    };
    {
        let mut s = chunk.function_shading(shading);
        s.shading_type(if radial {
            FunctionShadingType::Radial
        } else {
            FunctionShadingType::Axial
        });
        s.pair(Name(b"ColorSpace"), Name(b"DeviceRGB"));
        s.function(function);
        s.coords(coords);
        s.extend([true, true]);
    }
    {
        let mut p = chunk.shading_pattern(pattern);
        p.shading_ref(shading);
        p.matrix(transform_matrix(matrix));
    }
    let name = PdfContext::leak_name("P", ctx.shade_counter);
    ctx.shade_counter += 1;
    ctx.shadings.insert(key, ShadeEntry { pattern, name });
    ctx.shading_order.push(key);
    Some(name)
}
