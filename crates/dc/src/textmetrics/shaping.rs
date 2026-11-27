use std::sync::OnceLock;

/// A face prepared for harfrust 0.15 shaping: an OWNED
/// `harfrust::Font` — the read-fonts model font carries its bytes in
/// an Arc-backed blob and clones cheaply, so the leak-the-`FontRef`
/// pattern the ≤0.13 API forced is gone (#270) — plus its
/// units-per-em for the em-unit advance math. One per (database,
/// face), shared as a plain `Arc` cache entry.
pub(super) struct ShapedFace {
    upem: f64,
    font: harfrust::Font,
}

/// The per-(database, face) shaped-face cache (see [`ShapedFace`]).
/// Faces that fail to load are remembered in a negative cache so a
/// broken font costs one parse attempt, not one per measurement —
/// the `Font` owns its byte copy (nothing leaks; a rejected face's
/// one `to_vec` is the whole cost).
pub(super) fn shaped_face(
    db_key: usize,
    id: fontdb::ID,
    data: &[u8],
    index: u32,
) -> Option<std::sync::Arc<ShapedFace>> {
    type Faces = std::sync::Mutex<
        std::collections::HashMap<(usize, fontdb::ID), std::sync::Arc<ShapedFace>>,
    >;
    static FACES: OnceLock<Faces> = OnceLock::new();
    type Rejected = std::sync::Mutex<std::collections::HashSet<(usize, fontdb::ID)>>;
    static REJECTED: OnceLock<Rejected> = OnceLock::new();
    let key = (db_key, id);
    let cache = FACES.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return Some(hit.clone());
    }
    if REJECTED
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .contains(&key)
    {
        return None;
    }
    // The owned Font parses and validates the copy; a face that
    // cannot load lands in the negative cache above.
    let Some(font) = harfrust::Font::new(data.to_vec(), index) else {
        REJECTED
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(key);
        return None;
    };
    let upem = ttf_parser::Face::parse(data, index)
        .ok()
        .map(|f| f.units_per_em() as f64)
        .unwrap_or(1000.0);
    let shaped = std::sync::Arc::new(ShapedFace { upem, font });
    cache.lock().unwrap().insert(key, shaped.clone());
    Some(shaped)
}

/// The usvg-0.50 shaping pipeline, measurement-side (#146): BIDI
/// visual runs, each shaped by harfrust 0.15 with DEFAULT features —
/// pair kerning on, ligatures/contextual forms applied, script and
/// language guessed — advances summed in em units (the identity
/// `Scale` default keeps them in font units, exactly what usvg reads
/// off the same shaper). The base level is `None` (#257
/// text-direction): each PARAGRAPH takes the first-strong base
/// (explicit U+202A/U+202B overrides honored), the reconstruction the
/// analysis note pins for Visio-authored text. usvg's own pipeline
/// still hardcodes base LTR — the SVG exporter compensates with
/// isolates (see `svg::emit_text`) — but the advance SUM is
/// base-invariant either way, so a width measured here is the width
/// the rasterizer draws. Returns `None` when a run fails to shape —
/// the caller falls back to the isolated-advance sum (the 0.15 API
/// shapes in place and can fail; the old API could not).
pub(super) fn shape_glyph_advances_em(
    face: &ShapedFace,
    text: &str,
    fallback: &dyn Fn(char) -> f64,
) -> Option<Vec<f64>> {
    use harfrust::{shape, Buffer, Direction, ShapeOptions, ShaperFont};
    // Built per call, like usvg's own shaping path: harfrust's
    // interop slot caches the font's layout data behind the `Font`,
    // so repeated construction reuses it.
    let shaper = ShaperFont::new(&face.font);
    let bidi = unicode_bidi::BidiInfo::new(text, None);
    let Some(paragraph) = bidi.paragraphs.first() else {
        return Some(Vec::new());
    };
    let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
    let mut out = Vec::new();
    for run in runs.iter() {
        let sub = &text[run.clone()];
        if sub.is_empty() {
            continue;
        }
        let direction = if levels[run.start].is_ltr() {
            Direction::LeftToRight
        } else {
            Direction::RightToLeft
        };
        let mut buffer = Buffer::new();
        buffer.push_str(sub);
        buffer.set_direction(direction);
        buffer.guess_segment_properties();
        shape(&shaper, &mut buffer, ShapeOptions::new()).ok()?;
        // harfrust reports limit overruns (not API misuse) through the
        // buffer's allocation flag: Ok(()) with a failed buffer means
        // INCOMPLETE output (CodeRabbit, #278) — fall back to the
        // isolated sum instead of summing and caching a partial width.
        if !buffer.allocation_successful() {
            return None;
        }
        let infos = buffer.glyph_infos();
        let positions = buffer.glyph_positions();
        for i in 0..buffer.len() {
            let info = &infos[i];
            if info.glyph_id == 0 {
                // Missing glyph: charge the cluster's characters the
                // per-character fallback advance (usvg swaps a fallback
                // font in here; the divergence is coverage-gap only —
                // #146 keeps the documented per-character estimate).
                let start = info.cluster as usize;
                let next = infos.get(i + 1).map_or(sub.len(), |n| n.cluster as usize);
                let (lo, hi) = if start <= next {
                    (start, next)
                } else {
                    (next, start)
                };
                let hi = hi.min(sub.len());
                out.extend(sub.get(lo..hi).unwrap_or("").chars().map(fallback));
            } else {
                out.push(positions[i].x_advance as f64 / face.upem);
            }
        }
    }
    Some(out)
}

pub(super) fn shape_width_em(face: &ShapedFace, text: &str, fallback: &dyn Fn(char) -> f64) -> Option<f64> {
    shape_glyph_advances_em(face, text, fallback).map(|a| a.into_iter().sum())
}
