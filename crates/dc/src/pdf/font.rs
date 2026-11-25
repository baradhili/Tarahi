//! Font shaping and embedding — harfrust shaping with per-glyph
//! fallback splitting, and the sfnt/descriptor helpers assembly
//! consumes.

use pdf_writer::types::FontFlags;
use pdf_writer::Name;

use super::context::{FaceKey, GlyphRun, PdfContext, ShapedGlyph};

/// Does the face at `data[index]` have a real glyph for `c`?
pub(super) fn face_covers(data: &[u8], index: u32, c: char) -> bool {
    ttf_parser::Face::parse(data, index)
        .ok()
        .and_then(|f| f.glyph_index(c))
        .is_some_and(|g| g.0 != 0)
}

/// Units-per-em of a face (parsed per call; faces are few).
pub(super) fn face_upem(data: &[u8], index: u32) -> f32 {
    ttf_parser::Face::parse(data, index)
        .map(|f| f32::from(f.units_per_em()))
        .unwrap_or(1000.0)
}

/// Nominal advance of one glyph in 1/1000-em units (the /W form).
pub(super) fn glyph_width_1000(face: &ttf_parser::Face, gid: u16) -> f32 {
    face.glyph_hor_advance(ttf_parser::GlyphId(gid))
        .map(|a| a as f32 * 1000.0 / f32::from(face.units_per_em()))
        .unwrap_or(0.0)
}

/// Shape `text` with harfrust on `font` — the same pipeline the
/// measurement side uses (`textmetrics`): BIDI visual runs at base
/// LTR, default shaping features, advances in font units (the
/// identity `Scale` default). The font comes from
/// [`PdfContext::shaper_font`] (per-face cache — no per-span copy).
pub(super) fn shape_text(font: &harfrust::Font, text: &str) -> Vec<ShapedGlyph> {
    use harfrust::{shape, Buffer, Direction, ShapeOptions, ShaperFont};
    let shaper = ShaperFont::new(font);
    let bidi = unicode_bidi::BidiInfo::new(text, Some(unicode_bidi::Level::ltr()));
    let Some(paragraph) = bidi.paragraphs.first() else {
        return Vec::new();
    };
    let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
    let mut glyphs = Vec::new();
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
        // In place since 0.14 (#270); a run that fails to shape is
        // skipped — the old API had no failure path, so this only
        // ever drops glyphs the font cannot produce at all.
        if shape(&shaper, &mut buffer, ShapeOptions::new()).is_err() {
            continue;
        }
        let infos = buffer.glyph_infos();
        let positions = buffer.glyph_positions();
        for i in 0..buffer.len() {
            glyphs.push(ShapedGlyph {
                id: u16::try_from(infos[i].glyph_id).unwrap_or(0),
                // Text-absolute: harfrust clusters are offsets into
                // the run's own slice; the fallback splitter slices
                // the SPAN text by them (#218 review).
                cluster: run.start + infos[i].cluster as usize,
                x_advance: positions[i].x_advance as f32,
                x_offset: positions[i].x_offset as f32,
            });
        }
    }
    glyphs
}

/// Split a shaped run where the primary face lacks coverage: every
/// glyph with id 0 (the .notdef harfrust returns for missing glyphs)
/// whose cluster text the face cannot draw moves to a fallback face,
/// so a Latin label with one CJK name still renders both halves —
/// the same per-glyph fallback resvg does, at run granularity.
pub(super) fn split_fallback_runs(
    ctx: &mut PdfContext,
    primary: FaceKey,
    text: &str,
    glyphs: Vec<ShapedGlyph>,
) -> Vec<GlyphRun> {
    split_fallback_runs_impl(ctx, primary, text, glyphs)
        .into_iter()
        .map(|run| {
            // #218 review: a run the splitter handed a FALLBACK face
            // still carried the PRIMARY face's glyph ids — /W lookups
            // and Identity-H codes named the wrong font's glyphs.
            // Reshape the run's own text on its assigned face; the
            // primary's runs (and a reshape that fails) keep the
            // shaped output as it stands.
            if run.face == primary {
                return run;
            }
            let Some(font) = ctx.shaper_font(run.face) else {
                return run;
            };
            let glyphs = shape_text(&font, &run.text);
            if glyphs.is_empty() {
                return run;
            }
            GlyphRun { glyphs, ..run }
        })
        .collect()
}

fn split_fallback_runs_impl(
    ctx: &mut PdfContext,
    primary: FaceKey,
    text: &str,
    glyphs: Vec<ShapedGlyph>,
) -> Vec<GlyphRun> {
    let Some((data, index)) = ctx.face_data(primary) else {
        return Vec::new();
    };
    // Runs group (face, byte-span, glyphs); the span closes when the
    // face changes, and `text` is sliced to it with the cluster
    // offsets rebased — cluster values index the whole shaped string.
    type PendingRun = (FaceKey, usize, usize, Vec<ShapedGlyph>);
    let mut runs: Vec<GlyphRun> = Vec::new();
    let mut current: Option<PendingRun> = None;
    for (i, g) in glyphs.iter().enumerate() {
        let end = glyphs
            .get(i + 1)
            .map_or(text.len(), |n| n.cluster.min(text.len()));
        let start = g.cluster.min(end);
        let sub = text.get(start..end).unwrap_or("");
        let missing = g.id == 0 && sub.chars().all(|c| !face_covers(&data, index, c));
        let face = if missing {
            // The cluster's chars are all uncovered: fallback face for
            // the first char (runs group by char coverage).
            sub.chars().next().and_then(|c| ctx.fallback_face(c))
        } else {
            Some(primary)
        };
        match (face, current.as_mut()) {
            (Some(f), Some((rf, _, re, rg))) if *rf == f => {
                rg.push(g.clone());
                *re = end;
            }
            (Some(f), _) => {
                if let Some(run) = current.take() {
                    runs.push(finalize_run(run, text));
                }
                current = Some((f, start, end, vec![g.clone()]));
            }
            // No face anywhere: drop the glyph (the raster drops it too).
            (None, _) => {}
        }
    }
    if let Some(run) = current.take() {
        runs.push(finalize_run(run, text));
    }
    runs
}

/// Slice a pending run's text to its byte span and rebase its glyph
/// cluster offsets onto the slice.
fn finalize_run(run: (FaceKey, usize, usize, Vec<ShapedGlyph>), text: &str) -> GlyphRun {
    let (face, start, end, mut glyphs) = run;
    // RTL visual order returns DECREASING clusters, so the first
    // glyph can sit past a later one — rebase from the run's MINIMUM
    // cluster and never underflow (#218 review).
    let base = glyphs.iter().map(|g| g.cluster).min().unwrap_or(start).min(start);
    for g in &mut glyphs {
        g.cluster = g.cluster.saturating_sub(base);
    }
    GlyphRun {
        face,
        text: text.get(start..end).unwrap_or("").to_string(),
        glyphs,
    }
}

pub(super) fn face_content_name(postscript: &str) -> Name<'_> {
    let bytes = postscript.as_bytes();
    Name(if bytes.is_empty() { b"F0" } else { bytes })
}

/// The single font program to embed. A collection's per-font offset
/// table names a face whose TABLE OFFSETS ARE COLLECTION-RELATIVE
/// (OpenType TTC layout) — slicing at the face offset would leave
/// every table pointing into the wrong bytes. Rebuild a standalone
/// sfnt with the tables rebased; a plain font file is already the
/// face.
pub(super) fn face_bytes(data: &[u8], index: u32) -> Vec<u8> {
    if data.len() >= 12 && &data[0..4] == b"ttcf" {
        let n = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let i = index as usize;
        if n > 0 && i < n {
            let at = 12 + i * 4;
            if at + 4 <= data.len() {
                let o = u32::from_be_bytes([
                    data[at],
                    data[at + 1],
                    data[at + 2],
                    data[at + 3],
                ]) as usize;
                if o + 12 <= data.len() {
                    if let Some(out) = rebuild_sfnt(data, o) {
                        return out;
                    }
                }
            }
        }
    }
    data.to_vec()
}

/// Rebuild one sfnt (its header at `header` into the COLLECTION
/// `data`) into a standalone font: copy the table directory and
/// re-point each entry at the table's new position, 4-byte aligned
/// as the spec requires. The stored offsets count from the
/// COLLECTION start and may point anywhere in it — collections
/// share tables across faces, and a shared table can physically
/// PRECEDE this face's header (Noto CJK .ttc/.otc do exactly that),
/// so tables are read from the whole collection at their absolute
/// offsets (#218 review round: rebasing through `header` made any
/// preceding table fail the rebuild and fall back to embedding the
/// whole collection as a font program).
fn rebuild_sfnt(data: &[u8], header: usize) -> Option<Vec<u8>> {
    let face = data.get(header..)?;
    if face.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([face[4], face[5]]) as usize;
    let dir_len = 12 + num_tables * 16;
    if face.len() < dir_len {
        return None;
    }
    let mut total = dir_len;
    let mut spans = Vec::with_capacity(num_tables);
    for e in 0..num_tables {
        let at = 12 + e * 16;
        let off = u32::from_be_bytes(face[at + 8..at + 12].try_into().ok()?) as usize;
        let len = u32::from_be_bytes(face[at + 12..at + 16].try_into().ok()?) as usize;
        if off.checked_add(len)? > data.len() {
            return None;
        }
        spans.push((off, len));
        total += (len + 3) & !3;
    }
    let mut out = vec![0u8; total];
    out[..dir_len].copy_from_slice(&face[..dir_len]);
    let mut cursor = dir_len;
    for (e, (off, len)) in spans.iter().enumerate() {
        let at = 12 + e * 16;
        out[at + 8..at + 12].copy_from_slice(&(cursor as u32).to_be_bytes());
        out[cursor..cursor + len].copy_from_slice(&data[*off..*off + len]);
        cursor += (len + 3) & !3;
    }
    Some(out)
}

pub(super) fn flags_of(face: &ttf_parser::Face) -> FontFlags {
    let mut flags = FontFlags::SYMBOLIC;
    if face.is_italic() {
        flags |= FontFlags::ITALIC;
    }
    if face.is_monospaced() {
        flags |= FontFlags::FIXED_PITCH;
    }
    flags
}

pub(super) fn global_bbox(face: &ttf_parser::Face) -> pdf_writer::Rect {
    let b = face.global_bounding_box();
    pdf_writer::Rect::new(
        f32::from(b.x_min),
        f32::from(b.y_min),
        f32::from(b.x_max),
        f32::from(b.y_max),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #218 review: the image matrix composes the vertical flip —
    /// the first sample row lands at the rect's TOP edge in the
    /// y-down content frame, not the bottom (every picture exported
    /// upside down).
    /// #218 review round 2: a collection face extracts as a
    /// STANDALONE sfnt — the per-font table offsets are
    /// collection-relative, so the rebuild re-points them; both faces
    /// of a synthetic two-font .ttc parse at index 0 afterward.
    #[test]
    fn ttc_face_extracts_as_a_standalone_sfnt() {
        let font = include_bytes!("../../../dc/assets/fonts/Carlito-Regular.ttf");
        // Wrap the font twice into a TTC, rebasing each copy's table
        // offsets to its position in the collection — what real
        // builders do (collection-relative offsets).
        let embed_at = |font: &[u8], at: u32| -> Vec<u8> {
            let mut copy = font.to_vec();
            let num_tables = u16::from_be_bytes([copy[4], copy[5]]) as usize;
            for e in 0..num_tables {
                let t = 12 + e * 16;
                let off = u32::from_be_bytes(copy[t + 8..t + 12].try_into().unwrap());
                copy[t + 8..t + 12].copy_from_slice(&(off + at).to_be_bytes());
            }
            copy
        };
        // The header is 12 bytes (magic, version, numFonts) followed
        // by the offset array — 2 offsets = 8, so font 0 starts at 20.
        let head = font.len() as u32;
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        ttc.extend_from_slice(&2u32.to_be_bytes());
        ttc.extend_from_slice(&20u32.to_be_bytes());
        ttc.extend_from_slice(&(20 + head).to_be_bytes());
        ttc.extend_from_slice(&embed_at(font, 20));
        ttc.extend_from_slice(&embed_at(font, 20 + head));

        // A shared-table face: face 1's directory still points at
        // FACE 0's tables (the `embed_at(font, 20)` offsets) —
        // physically before face 1's header, the Noto-CJK-collection
        // shape. The rebuild must read them at their absolute
        // collection offsets.
        let shared = embed_at(font, 20);
        let mut ttc_shared = ttc.clone();
        let face1_at = (20 + head) as usize;
        ttc_shared[face1_at..face1_at + shared.len()].copy_from_slice(&shared);

        let f0 = face_bytes(&ttc, 0);
        let f1 = face_bytes(&ttc, 1);
        let fs = face_bytes(&ttc_shared, 1);
        let face = ttf_parser::Face::parse(&fs, 0)
            .unwrap_or_else(|e| panic!("shared-table face parses standalone: {e}"));
        assert_eq!(
            face.number_of_glyphs(),
            ttf_parser::Face::parse(font, 0).unwrap().number_of_glyphs(),
            "the shared face carries face 0's tables"
        );
        let src_face = ttf_parser::Face::parse(font, 0).unwrap();
        for (i, bytes) in [(0usize, &f0), (1, &f1)] {
            let face = ttf_parser::Face::parse(bytes, 0)
                .unwrap_or_else(|e| panic!("face {i} of the collection parses standalone: {e}"));
            assert_eq!(
                face.number_of_glyphs(),
                src_face.number_of_glyphs(),
                "face {i} carries the source's tables"
            );
        }
        // Out-of-range index falls back to the raw bytes (parse as a
        // collection fails the caller's own guard).
        assert_eq!(face_bytes(&ttc, 5).len(), ttc.len());
    }
}
