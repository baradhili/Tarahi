//! Real-font text metrics for the SVG renderer (GitHub #21).
//!
//! The wrap decision in [`crate::svg::emit_text`] used to be a
//! ~0.5 em average-advance guess, re-wrapping lines the drawing
//! author had fitted to the block. This module measures per-glyph
//! advances from the SAME faces the rasterizer draws:
//!
//! - the database is **fontdb** — the same crate resvg's rasterizer
//!   uses — loaded with the vendored metric faces (Carlito,
//!   style-complete; Liberation Sans/Serif/Mono for the Arial/Times/
//!   Courier substitution chains — #145) plus **every installed
//!   system font** (the app renders with real Arial/Calibri/Noto where
//!   installed; owner note 2026-10-27);
//! - family resolution walks the CSS chain the renderer emits
//!   (`Calibri, Carlito, sans-serif`, …): the first INSTALLED named
//!   family wins (case-insensitive, like `svgimg`'s raster font db),
//!   generic keywords and chains nothing satisfies pin to the
//!   embedded Carlito;
//! - a glyph the resolved face lacks falls back PER CHARACTER to
//!   Carlito and finally to a conservative advance estimate — the
//!   rasterizer substitutes per glyph too (system fonts stay
//!   available for scripts Carlito lacks).
//!
//! The layer architecture (system catalog + vendored fallback +
//! per-character coverage fallback) follows cadcraft/wordcraft's
//! font crates (owner pointer, 2026-10-27); this is the
//! wrap-metrics-sized cut of it.
//!
//! Determinism: [`pin_embedded_for_tests`] swaps the process-wide
//! database for the Carlito-only one, mirroring
//! `svgimg::pin_embedded_fonts_for_tests` — kittest goldens and
//! unit tests measure the vendored face on every machine. Real
//! documents measure with the user's installed fonts, exactly what
//! rasterizes.
//!
//! Shaped measurement (#146): usvg 0.50 rasterizes text through
//! harfrust 0.15 (BIDI visual runs at base LTR, default features —
//! pair kerning on, ligatures and contextual forms applied, script and
//! language guessed). `width_in` runs the SAME pipeline over the SAME
//! face, so the wrap decision and the raster shape identically by
//! construction. Glyphs the resolved face lacks keep the
//! per-character Carlito/conservative fallback advance (usvg swaps a
//! fallback font in there — the one documented divergence, coverage
//! gaps only). The shaper pin moves in lockstep with the in-tree usvg
//! (#270): harfrust 0.15 is usvg 0.50's shaper, and it bumps only
//! when usvg's own dependency does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Conservative advance for a glyph no loaded face covers (wide-ish
/// Latin; CJK full-width runs wider, but coverage gaps are rare in the
/// corpus and never wrap-decisive at this precision).
const UNCOVERED_EM: f64 = 0.55;

/// One step of the 18-fractional-bit inch composition grid (#234):
/// 1/2^18 in ≈ 0.0000038 in. Visio quantizes COMPOSED metrics — font
/// sizes, then per-glyph advances — onto this grid before summing them
/// (the measured 120 %-spacing height corrections at 6/8/10/12 pt are
/// exactly the round-to-nearest residues of the quantized size). Only
/// measurement quantizes; SVG painting keeps the raw size (#234).
const GRID: f64 = 262_144.0; // 2^18

/// Round `v` (inches) to the nearest 18-fractional-bit grid step —
/// the quantization Visio applies to composed font sizes (#234).
pub fn quantize18(v: f64) -> f64 {
    (v * GRID).round() / GRID
}

/// Truncate `v` (inches) toward zero on the grid — the reduction
/// Visio applies to a script run's drawn size before its glyph
/// advances round on the grid (#230: "the reduced em is truncated on
/// the 18-fractional-bit inch composition grid").
pub fn trunc18(v: f64) -> f64 {
    (v * GRID).trunc() / GRID
}

/// Round `v` (inches) onto the grid — the per-glyph advance rounding
/// (#234 terminal-run advance, #230 script advances).
fn round18(v: f64) -> f64 {
    (v * GRID).round() / GRID
}

/// Every vendored face (GitHub #145): the Carlito set is style-complete
/// (Regular/Bold/Italic/BoldItalic — italic used to measure as the
/// upright face), and the Liberation metric faces (Sans/Serif/Mono × 4
/// styles) make the Arial/Times New Roman/Courier New substitution
/// chains resolve the face they NAME on every machine, not only hosts
/// with Liberation installed. Licenses live beside the files.
const EMBEDDED_FONTS: [&[u8]; 16] = [
    include_bytes!("../assets/fonts/Carlito-Regular.ttf"),
    include_bytes!("../assets/fonts/Carlito-Bold.ttf"),
    include_bytes!("../assets/fonts/Carlito-Italic.ttf"),
    include_bytes!("../assets/fonts/Carlito-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-Italic.ttf"),
    include_bytes!("../assets/fonts/LiberationSans-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/LiberationSerif-Regular.ttf"),
    include_bytes!("../assets/fonts/LiberationSerif-Bold.ttf"),
    include_bytes!("../assets/fonts/LiberationSerif-Italic.ttf"),
    include_bytes!("../assets/fonts/LiberationSerif-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/LiberationMono-Regular.ttf"),
    include_bytes!("../assets/fonts/LiberationMono-Bold.ttf"),
    include_bytes!("../assets/fonts/LiberationMono-Italic.ttf"),
    include_bytes!("../assets/fonts/LiberationMono-BoldItalic.ttf"),
];

/// CSS generic keywords the chain builder can emit — they resolve to
/// the embedded default, matching the raster db's generic mappings.
fn is_generic(name: &str) -> bool {
    matches!(
        name,
        "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui"
    )
}

/// The font database: the vendored faces plus system fonts, or
/// vendored-only for hermetic measurement (system fonts stay the
/// measurement source wherever installed — the vendored set is the
/// deterministic floor, owner note 2026-10-30).
struct MetricsDb {
    db: fontdb::Database,
    /// The vendored Carlito faces (per-character coverage + the
    /// chain-end default), one per (weight, style) corner.
    carlito: fontdb::ID,
    carlito_bold: fontdb::ID,
    carlito_italic: fontdb::ID,
    carlito_bold_italic: fontdb::ID,
}

/// CSS-style distance from the wanted posture (CodeRabbit, #145):
/// an Oblique face serves an italic request ahead of Normal — the
/// same order resvg's fontdb query uses, so measurement and raster
/// pick the same face when a family lacks a true Italic.
fn style_penalty(have: fontdb::Style, want_italic: bool) -> usize {
    use fontdb::Style::{Italic, Normal, Oblique};
    match (want_italic, have) {
        (true, Italic) | (true, Oblique) => 0,
        (true, Normal) => 1,
        (false, Normal) => 0,
        (false, Italic) | (false, Oblique) => 1,
    }
}

/// Rank one face against the wanted (weight, style): upright faces
/// stay preferred for upright text (v1 measured italic as upright),
/// and a family that lacks the wanted style still resolves its
/// nearest face instead of nothing.
fn style_rank(f: &fontdb::FaceInfo, want_style: fontdb::Style, target: i32) -> (usize, i32) {
    (
        style_penalty(f.style, want_style == fontdb::Style::Italic),
        (f.weight.0 as i32 - target).abs(),
    )
}

fn build_db(embedded_only: bool) -> MetricsDb {
    let mut db = fontdb::Database::new();
    for bytes in EMBEDDED_FONTS {
        db.load_font_data(bytes.to_vec());
    }
    // Runtime-registered faces (the dc-ui complex-script drop — see
    // `register_face_bytes`); kept so BOTH databases see them no
    // matter which builds first.
    for bytes in pending_faces().lock().unwrap().iter() {
        db.load_font_data(bytes.clone());
    }
    if !embedded_only {
        db.load_system_fonts();
    }
    let pick = |bold: bool, italic: bool| -> Option<fontdb::ID> {
        let want_style = if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        };
        let target = if bold { 700 } else { 400 };
        db.faces()
            .filter(|f| {
                f.families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("Carlito"))
            })
            .filter(|f| matches!(f.style, fontdb::Style::Normal | fontdb::Style::Italic))
            .min_by_key(|f| style_rank(f, want_style, target))
            .map(|f| f.id)
    };
    MetricsDb {
        carlito: pick(false, false).expect("vendored Carlito Regular is embedded"),
        carlito_bold: pick(true, false).expect("vendored Carlito Bold is embedded"),
        carlito_italic: pick(false, true).expect("vendored Carlito Italic is embedded"),
        carlito_bold_italic: pick(true, true).expect("vendored Carlito BoldItalic is embedded"),
        db,
    }
}

/// Kittest/unit determinism: pin the process to the Carlito-only
/// database (see the module docs). Set once by the test harnesses;
/// there is deliberately no way to unset it.
static PIN_EMBEDDED: AtomicBool = AtomicBool::new(false);

/// Faces registered at runtime after this module's vendored set
/// (#257 text-direction coordination: ONE font drop for the complex
/// script face — dc's vendored Vazirmatn — registered into BOTH the
/// metrics and the raster databases, never copied). The list is
/// consulted when each database builds; callers must register before
/// the first measurement — `dc::register_complex_script_faces` (the
/// dc CLI's main and dc-ui's `theme::install_fonts` both call it).
fn pending_faces() -> &'static std::sync::Mutex<Vec<Vec<u8>>> {
    static PENDING: OnceLock<std::sync::Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    PENDING.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Register a face (its file bytes) into the metrics database. Must
/// precede the first `resolve`/`width_in` call — the databases build
/// once, lazily, and consult the pending list at build time.
#[doc(hidden)]
pub fn register_face_bytes(bytes: Vec<u8>) {
    pending_faces().lock().unwrap().push(bytes);
}

/// Test hook for hermetic measurement (see the module docs).
#[doc(hidden)]
pub fn pin_embedded_for_tests() {
    PIN_EMBEDDED.store(true, Ordering::Relaxed);
}

fn metrics() -> &'static MetricsDb {
    static SYSTEM: OnceLock<MetricsDb> = OnceLock::new();
    static EMBEDDED: OnceLock<MetricsDb> = OnceLock::new();
    if PIN_EMBEDDED.load(Ordering::Relaxed) {
        EMBEDDED.get_or_init(|| build_db(true))
    } else {
        SYSTEM.get_or_init(|| build_db(false))
    }
}

/// One resolved measurement face. Cheap to construct (an id into the
/// process-wide database); measure through [`Self::width_in`].
pub struct ResolvedFace {
    id: fontdb::ID,
    carlito: fontdb::ID,
    is_carlito: bool,
}

impl ResolvedFace {
    /// Whether THIS face (not the Carlito fallback chain — the raster
    /// resolves the element's family to this one) carries a glyph for
    /// `c`. The wrap's complex-script probe uses it to pick the face
    /// usvg's per-cluster fallback will actually draw (PR #269
    /// review).
    pub fn covers(&self, c: char) -> bool {
        self.advance_of(self.id, c).is_some()
    }
}

/// Split `text` into contiguous runs by which face of the measurement
/// chain covers each character: `(face_ix, text)` pairs where
/// `face_ix` indexes `[self, others…]` — a character an earlier face
/// lacks and a later one covers joins that later face's run, and a
/// character NOTHING covers stays on face 0 (the measuring face's
/// per-character fallback estimates it, exactly like the single-face
/// path). Shaping then happens per run with the face that paints it,
/// mirroring the raster's per-cluster fallback (usvg walks its
/// fontdb in load order — the registration order of
/// `dc::register_complex_script_faces`, which this chain must copy);
/// only cross-script run boundaries forgo cross-run kerning, which
/// does not exist across scripts anyway. The two-face form (PR #269
/// review) grew to the N-face chain with the CJK wave (#282):
/// Vazirmatn covers the Arabic script, Noto Sans SC the Han glyphs
/// the Latin faces lack.
pub(crate) fn split_runs(
    text: &str,
    covers: &[&dyn Fn(char) -> bool],
) -> Vec<(usize, String)> {
    let mut runs: Vec<(usize, String)> = Vec::new();
    for c in text.chars() {
        let face_ix = covers.iter().position(|covers| covers(c)).unwrap_or(0);
        match runs.last_mut() {
            Some((last_ix, buf)) if *last_ix == face_ix => buf.push(c),
            _ => runs.push((face_ix, c.to_string())),
        }
    }
    runs
}

// ------------- CJK line-break classes (#282 zh-Hans) -------------
//
// The greedy wrap is space-based (0.2.9), and Chinese paragraphs
// carry no spaces: without these classes a whole paragraph measures
// as ONE unbreakable word that only the over-long hard-split can cut
// — at any character, ignoring the kinsoku rules. The classes below
// give the wrap break opportunities BETWEEN ideographs (UAX #14
// lite), with the two prohibition families Chinese typography
// requires.

/// The "break between any two" class: CJK ideographs and kana.
/// Hangul is deliberately absent — Korean text is space-separated
/// and needs no intra-word breaks (a future `ko` rides this wave's
/// fonts but not these classes).
fn is_cjk_ideographic(c: char) -> bool {
    matches!(
        c,
        '\u{3400}'..='\u{4DBF}'     // CJK Extension A
            | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs (the zh workhorse)
            | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
            | '\u{3040}'..='\u{30FF}' // Hiragana + Katakana (ja rides the same classes)
            | '\u{31F0}'..='\u{31FF}' // Katakana Phonetic Extensions
    )
}

/// Full-width CJK closing punctuation (kinsoku 禁則): grants the
/// break AFTER it (sentence enders) and never starts a line.
fn is_cjk_closing(c: char) -> bool {
    const CLOSING: &str = "。，、；：！？）］｝〉》」』】〕”’…—～·％￥";
    CLOSING.contains(c)
}

/// Chars a line may never START with: the CJK closers plus the ASCII
/// closers riding along, so a Han char before an ASCII `.`, `)`, …
/// stays glued the same way. ASCII closers SUPPRESS only — they never
/// GRANT a boundary (CodeRabbit on #282: letting them grant split
/// `x.y` into `x.`|`y`), so Latin-internal tokenization stays
/// space-driven. Opening punct needs no set of its own — it grants no
/// break (see [`cjk_break_between`]), so a line never ends right
/// after （ 「 『. Public to the wrap's emergency splitter, which must
/// never leave a forbidden char to OPEN a line either.
pub(crate) fn line_head_forbidden(c: char) -> bool {
    is_cjk_closing(c)
        || matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%'
        )
}

/// Explicit non-breaking controls (UAX #14 class GL and friends): no
/// boundary sits before OR after one — `图⍽表` with a no-break space
/// is the author saying "keep these together".
fn is_glue(c: char) -> bool {
    matches!(c, '\u{00A0}' | '\u{202F}' | '\u{2060}' | '\u{FEFF}')
}

/// Combining marks and joiners: a boundary adjacent to one is not a
/// grapheme-cluster boundary (UAX #29) — breaking there splits a
/// decomposed character (カ + dakuten, CodeRabbit on #282).
fn is_combining(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'       // combining diacritical marks
            | '\u{3099}'..='\u{309A}' // combining kana voicing
            | '\u{FE00}'..='\u{FE0F}' // variation selectors
            | '\u{200D}'              // zero-width joiner
            | '\u{20D0}'..='\u{20FF}' // combining marks for symbols
    )
}

/// A wrap opportunity between `a` and `b`: the LEFT char must grant
/// one — an ideograph, kana, or a full-width CJK closing punct (Latin
/// and ASCII punctuation never grant; Latin-internal breaks stay
/// space-driven, so pure-Latin wrap output is untouched) — and the
/// RIGHT char must accept it (not a line-head-forbidden closer). A
/// boundary adjacent to glue or a combining mark never opens
/// (explicit no-break controls and grapheme interiors). Sentence
/// enders like 。 therefore both grant the following break and refuse
/// to open a line.
pub(crate) fn cjk_break_between(a: char, b: char) -> bool {
    if is_glue(a) || is_glue(b) || is_combining(a) || is_combining(b) {
        return false;
    }
    (is_cjk_ideographic(a) || is_cjk_closing(a)) && !line_head_forbidden(b)
}

/// One greedy-wrap token of a paragraph: `sep` is the separator
/// content that precedes the token when it JOINS the current line
/// (the one space of a word boundary, the ideographic space U+3000,
/// or nothing at a CJK break); a line break consumes its separator,
/// exactly like the space-based wrap consumed the joining space.
pub(crate) struct WrapToken<'a> {
    pub sep: &'a str,
    pub tok: &'a str,
}

/// Tokenize one paragraph for the greedy wrap: words separated by
/// ASCII spaces (each space reproducing the `split(' ')` /
/// rejoin-with-one-space semantics verbatim — including the empty
/// words consecutive spaces yield, so space runs and leading/trailing
/// spaces survive byte-for-byte), the ideographic space U+3000
/// breaking like a space, and every [`cjk_break_between`] opportunity
/// inside a word splitting it — so Chinese paragraphs wrap between
/// ideographs under the kinsoku rules instead of measuring as one
/// unbreakable word. A paragraph without CJK grant-chars tokenizes
/// to its unsplit words: wrap output is byte-identical to the
/// space-only engine.
pub(crate) fn wrap_tokens(para: &str) -> Vec<WrapToken<'_>> {
    // Parts between separators — `split(' ')` semantics, generalized
    // to U+3000: every separator char ends a part (possibly empty),
    // so a trailing separator yields a final empty part and
    // consecutive separators yield empty parts between, exactly like
    // `para.split(' ')`.
    let mut parts: Vec<(char, &str)> = Vec::new();
    {
        let bytes = para.as_bytes();
        let mut start = 0usize;
        let mut sep = ' ';
        let mut i = 0usize;
        while i < bytes.len() {
            let c = para[i..].chars().next().unwrap();
            if c == ' ' || c == '\u{3000}' {
                parts.push((sep, &para[start..i]));
                sep = c;
                start = i + c.len_utf8();
            }
            i += c.len_utf8();
        }
        parts.push((sep, &para[start..]));
    }
    // Each part splits at the CJK opportunities; the FIRST sub-token
    // carries the part's separator (empty for the paragraph's opening
    // part — the wrap loop starts unstarted), the rest join with
    // nothing.
    let mut out: Vec<WrapToken> = Vec::new();
    for (ix, (sep, part)) in parts.into_iter().enumerate() {
        let mut pending_sep: &str = if ix == 0 {
            ""
        } else {
            match sep {
                '\u{3000}' => "\u{3000}",
                _ => " ",
            }
        };
        let mut sub_start = 0usize;
        let mut prev: Option<char> = None;
        for (i, c) in part.char_indices() {
            if prev.is_some_and(|p| cjk_break_between(p, c)) {
                out.push(WrapToken {
                    sep: pending_sep,
                    tok: &part[sub_start..i],
                });
                pending_sep = "";
                sub_start = i;
            }
            prev = Some(c);
        }
        out.push(WrapToken {
            sep: pending_sep,
            tok: &part[sub_start..],
        });
    }
    out
}

/// Resolve the face a shape's text measures (and rasterizes) in: the
/// family chain the renderer emits, first installed family wins,
/// Carlito fallback. `bold`/`italic` prefer the family's matching
/// weight/style corner (#145: the italic faces are vendored, so
/// italic text now measures italic advances instead of the upright
/// face's).
pub fn resolve(font: Option<&str>, bold: bool, italic: bool) -> ResolvedFace {
    let m = metrics();
    let target = if bold { 700 } else { 400 };
    let want_style = if italic {
        fontdb::Style::Italic
    } else {
        fontdb::Style::Normal
    };
    let installed = |family: &str| -> Option<fontdb::ID> {
        m.db.faces()
            .filter(|f| {
                f.families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(family))
            })
            .min_by_key(|f| style_rank(f, want_style, target))
            .map(|f| f.id)
    };
    for name in crate::svg::family_chain(font) {
        if is_generic(&name) {
            break; // the raster db maps generics to Carlito
        }
        if let Some(id) = installed(&name) {
            return ResolvedFace {
                id,
                carlito: m.carlito,
                is_carlito: false,
            };
        }
    }
    let id = match (bold, italic) {
        (false, false) => m.carlito,
        (true, false) => m.carlito_bold,
        (false, true) => m.carlito_italic,
        (true, true) => m.carlito_bold_italic,
    };
    ResolvedFace {
        id,
        carlito: m.carlito,
        is_carlito: true,
    }
}

impl ResolvedFace {
    /// Advance of one char in em units, from the face tables —
    /// falling back per character to the vendored Carlito, then to a
    /// conservative estimate (the rasterizer substitutes per glyph
    /// too).
    fn char_em(&self, c: char) -> f64 {
        if let Some(a) = self.advance_of(self.id, c) {
            return a;
        }
        if !self.is_carlito {
            if let Some(a) = self.advance_of(self.carlito, c) {
                return a;
            }
        }
        UNCOVERED_EM
    }

    /// Per-(database, face, char) advance cache (#21 review):
    /// `with_face_data` re-opens and re-parses the face file on every
    /// call, and the wrap loop measures text char by char — repeated
    /// glyphs would reopen the file each time. Missing advances are
    /// cached too (the fallback chain re-asks the same face). The
    /// database is keyed by its static's address (as `usize` — a
    /// static Mutex must be `Send`): fontdb IDs are slotmap keys,
    /// unique per `Database`, and the system and embedded statics can
    /// coexist in one test process.
    fn advance_of(&self, id: fontdb::ID, c: char) -> Option<f64> {
        // (database, face, char) → advance, missing ones included.
        type Advances =
            std::sync::Mutex<std::collections::HashMap<(usize, fontdb::ID, char), Option<f64>>>;
        static ADVANCES: OnceLock<Advances> = OnceLock::new();
        let m = metrics();
        let key = (m as *const MetricsDb as usize, id, c);
        let cache = ADVANCES.get_or_init(Default::default);
        if let Some(hit) = cache.lock().unwrap().get(&key) {
            return *hit;
        }
        let out = m.db.with_face_data(id, |data, index| {
            let face = ttf_parser::Face::parse(data, index).ok()?;
            let gid = face.glyph_index(c)?;
            let adv = face.glyph_hor_advance(gid)?;
            Some(adv as f64 / face.units_per_em() as f64)
        })?;
        cache.lock().unwrap().insert(key, out);
        out
    }

    /// Width of `text` at `size_in` inches (Char.Size units): the
    /// SHAPED advance sum (#146) — the same harfrust 0.15 shaping
    /// usvg 0.50 rasterizes with (pair kerning on, ligatures and
    /// contextual forms applied, BIDI visual runs at base LTR,
    /// script/language guessed), missing glyphs falling back per
    /// character. The isolated-advance sum survives as the fallback
    /// when the face bytes cannot be shaped.
    pub fn width_in(&self, text: &str, size_in: f64) -> f64 {
        self.shaped_width_em(text) * size_in
    }

    /// Visio's COMPOSED width of `text` at `size_in` inches (#234):
    /// the size quantized onto the 18-fractional-bit grid first, then
    /// every shaped glyph advance rounded onto that same grid before
    /// summing. This is the TEXTWIDTH building block — the caller adds
    /// the terminal-run space and the 0.00005 allowance (#234's three
    /// rules stay separable for tests). The plain `width_in` stays the
    /// WRAP engine's measure: quantization deltas (≤ 1e-6 in) must not
    /// flip a break the raster would not also flip.
    pub fn composed_width_in(&self, text: &str, size_in: f64) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        self.composed_run_in(text, quantize18(size_in))
    }

    /// The #234 composed advance sum for one run at the (already
    /// quantized) `q`: every shaped glyph advance rounded onto the
    /// 18-fractional-bit grid before summing — [`Self::composed_width_in`]'s
    /// core, split out for the runs variant below.
    fn composed_run_in(&self, text: &str, q: f64) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let fallback = |c: char| self.char_em(c);
        let m = metrics();
        let db_key = m as *const MetricsDb as usize;
        let advances = m
            .db
            .with_face_data(self.id, |data, index| {
                shaped_face(db_key, self.id, data, index)
                    .and_then(|shaped| shape_glyph_advances_em(&shaped, text, &fallback))
            })
            .flatten()
            .unwrap_or_else(|| text.chars().map(fallback).collect::<Vec<_>>());
        advances
            .into_iter()
            .map(|em| round18(q * em))
            .sum()
    }

    /// The run-split core over the measurement chain `[self,
    /// others…]` (see [`split_runs`]) — shared by the three runs
    /// variants below.
    fn chain_runs<'a>(
        &'a self,
        others: &'a [&'a ResolvedFace],
        text: &'a str,
    ) -> Vec<(usize, String)> {
        let self_cov = |c: char| self.covers(c);
        let other_covs: Vec<_> = others.iter().map(|f| |c: char| f.covers(c)).collect();
        let mut covers: Vec<&dyn Fn(char) -> bool> = Vec::with_capacity(1 + others.len());
        covers.push(&self_cov);
        covers.extend(other_covs.iter().map(|c| c as &dyn Fn(char) -> bool));
        split_runs(text, &covers)
    }

    /// The chain as a face slice — `[self, others…]`, indexed by the
    /// run's face index.
    fn chain_faces<'a>(&'a self, others: &'a [&'a ResolvedFace]) -> Vec<&'a ResolvedFace> {
        std::iter::once(self).chain(others.iter().copied()).collect()
    }

    /// True when `self` alone covers every character — the common
    /// pure-Latin case takes the allocation-free single-face path
    /// (byte-identical widths, the pre-CJK hot loop).
    fn self_covers_all(&self, text: &str) -> bool {
        text.chars().all(|c| self.covers(c))
    }

    /// The composed-width twin of [`Self::width_in_runs`] (local
    /// CodeRabbit round 3): #234's per-glyph grid rounding, with each
    /// run measured on the face that covers it — the renderer's
    /// fallback runs. A paragraph the primary face cannot cover must
    /// not compose its width from missing-glyph estimates the paint
    /// never draws. Single-coverage text (or fallbacks that cover
    /// nothing, as in the unregistered test world) is byte-identical
    /// to [`Self::composed_width_in`]: every run lands on `self`.
    pub fn composed_width_in_runs(
        &self,
        others: &[&ResolvedFace],
        text: &str,
        size_in: f64,
    ) -> f64 {
        if text.is_empty() || self.self_covers_all(text) {
            return self.composed_width_in(text, size_in);
        }
        let q = quantize18(size_in);
        let faces = self.chain_faces(others);
        self.chain_runs(others, text)
            .into_iter()
            .map(|(ix, run)| faces[ix].composed_run_in(&run, q))
            .sum()
    }

    /// The face covering `text`'s FINAL run — where the terminal-run
    /// space of a composed width comes from (the runs variant of
    /// taking the measuring face's space advance).
    pub fn final_run_face<'a>(
        &'a self,
        others: &'a [&'a ResolvedFace],
        text: &str,
    ) -> &'a ResolvedFace {
        if self.self_covers_all(text) {
            return self;
        }
        let faces = self.chain_faces(others);
        self.chain_runs(others, text)
            .last()
            .map_or(self, |(ix, _)| faces[*ix])
    }

    /// The grid-rounded advance of ONE space at `size_in` — the
    /// terminal-run space of #234's composed width ("one terminal
    /// space, from the final run, even if it paints no glyph").
    pub fn space_advance_in(&self, size_in: f64) -> f64 {
        round18(quantize18(size_in) * self.shaped_width_em(" "))
    }

    /// Shaped width of `text` in em units, through a per-(database,
    /// face, text) cache — the wrap loop measures growing candidate
    /// strings repeatedly, and a shaped width is far too costly to
    /// recompute per candidate. The cache is bounded by clearing it
    /// wholesale past 65 536 entries (text blocks are small; this is
    /// a recompute, not a correctness, bound).
    fn shaped_width_em(&self, text: &str) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        type Widths = std::sync::Mutex<std::collections::HashMap<(usize, fontdb::ID, String), f64>>;
        static WIDTHS: OnceLock<Widths> = OnceLock::new();
        let m = metrics();
        let key = (m as *const MetricsDb as usize, self.id, text.to_string());
        let cache = WIDTHS.get_or_init(Default::default);
        if let Some(hit) = cache.lock().unwrap().get(&key) {
            return *hit;
        }
        let fallback = |c: char| self.char_em(c);
        let db_key = m as *const MetricsDb as usize;
        let width =
            m.db.with_face_data(self.id, |data, index| {
                shaped_face(db_key, self.id, data, index)
                    .and_then(|shaped| shape_width_em(&shaped, text, &fallback))
            })
            .flatten()
            .unwrap_or_else(|| text.chars().map(fallback).sum());
        let mut c = cache.lock().unwrap();
        if c.len() > 65_536 {
            c.clear();
        }
        c.insert(key, width);
        width
    }

    /// Shaped width where the chain covers the runs this face lacks
    /// (see [`split_runs`]) — a mixed paragraph's script tail measures
    /// with the face the raster actually paints it with, instead of
    /// missing-glyph estimates. Text the measuring face covers alone
    /// takes the same single-face shaped path as [`Self::width_in`]:
    /// unmixed text keeps byte-identical widths.
    pub fn width_in_runs(
        &self,
        others: &[&ResolvedFace],
        text: &str,
        size_in: f64,
    ) -> f64 {
        if text.is_empty() || self.self_covers_all(text) {
            return self.width_in(text, size_in);
        }
        let faces = self.chain_faces(others);
        let total: f64 = self
            .chain_runs(others, text)
            .into_iter()
            .map(|(ix, run)| faces[ix].shaped_width_em(&run))
            .sum();
        total * size_in
    }

    /// The face's ascender in em units (hhea) — the GUI overlay places
    /// its editor from the renderer's baseline math, so it needs the
    /// same face's ascent (#22 review).
    pub fn ascent_em(&self) -> f64 {
        metrics()
            .db
            .with_face_data(self.id, |data, index| {
                ttf_parser::Face::parse(data, index)
                    .ok()
                    .map(|f| f.ascender() as f64 / f.units_per_em() as f64)
            })
            .flatten()
            .unwrap_or(0.8)
    }
}

/// A face prepared for harfrust 0.15 shaping: an OWNED
/// `harfrust::Font` — the read-fonts model font carries its bytes in
/// an Arc-backed blob and clones cheaply, so the leak-the-`FontRef`
/// pattern the ≤0.13 API forced is gone (#270) — plus its
/// units-per-em for the em-unit advance math. One per (database,
/// face), shared as a plain `Arc` cache entry.
struct ShapedFace {
    upem: f64,
    font: harfrust::Font,
}

/// The per-(database, face) shaped-face cache (see [`ShapedFace`]).
/// Faces that fail to load are remembered in a negative cache so a
/// broken font costs one parse attempt, not one per measurement —
/// the `Font` owns its byte copy (nothing leaks; a rejected face's
/// one `to_vec` is the whole cost).
fn shaped_face(
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
fn shape_glyph_advances_em(
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

fn shape_width_em(face: &ShapedFace, text: &str, fallback: &dyn Fn(char) -> f64) -> Option<f64> {
    shape_glyph_advances_em(face, text, fallback).map(|a| a.into_iter().sum())
}

/// The actual face a shape's text rasterizes in, as bytes — for
/// consumers that must RENDER with the same font (the GUI's in-place
/// editor draws its overlay in the shape's real face, not the
/// embedded default: #22 review — WYSIWYG means the actual font).
#[derive(Clone)]
pub struct FaceData {
    /// The face's canonical family name ("Arial" when the system's
    /// Arial won the chain, "Carlito" for the vendored fallback).
    pub family: String,
    /// The font file's bytes (a collection's face comes with its
    /// `index`).
    pub data: std::sync::Arc<Vec<u8>>,
    /// Face index inside a collection (0 for plain files).
    pub index: u32,
    /// The ascender in em units (see [`ResolvedFace::ascent_em`]).
    pub ascent_em: f64,
}

/// The bytes + identity of the face `resolve` picks for `font` —
/// reading a system font file on first use (vendored faces are
/// embedded; callers cache what they register from this).
pub fn face_data(font: Option<&str>, bold: bool, italic: bool) -> Option<FaceData> {
    let m = metrics();
    let face = resolve(font, bold, italic);
    let info = m.db.faces().find(|f| f.id == face.id)?;
    let family = info
        .families
        .first()
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "Carlito".to_string());
    let data = match &info.source {
        fontdb::Source::Binary(arc) => {
            // The vendored faces (and any memory-registered font).
            let bytes: &[u8] = (**arc).as_ref();
            std::sync::Arc::new(bytes.to_vec())
        }
        fontdb::Source::File(path) => std::sync::Arc::new(std::fs::read(path).ok()?),
        fontdb::Source::SharedFile(path, _) => std::sync::Arc::new(std::fs::read(path).ok()?),
        #[allow(unreachable_patterns)] // memmap-off builds see only Binary+File
        _ => return None,
    };
    Some(FaceData {
        family,
        data,
        index: info.index,
        ascent_em: face.ascent_em(),
    })
}

#[cfg(test)]
mod tests {

    /// PR #269 review: runs split by which face covers each char —
    /// only chars an EARLIER face lacks AND a later one covers switch
    /// runs; chars nothing covers stay on face 0 (their advance falls
    /// through the missing-glyph chain either way).
    #[test]
    fn split_runs_follows_per_char_coverage() {
        let lower = |c: char| c.is_ascii_lowercase();
        let upper = |c: char| c.is_ascii_uppercase();
        let digit = |c: char| c.is_ascii_digit();
        let runs = split_runs("ab CD EF gh", &[&lower, &upper]);
        let flat: Vec<(usize, String)> = runs
            .into_iter()
            .map(|(o, t): (usize, String)| {
                (
                    o,
                    t.chars().filter(|c| !c.is_whitespace()).collect::<String>(),
                )
            })
            .filter(|(_, t)| !t.is_empty())
            .collect();
        assert_eq!(
            flat,
            vec![
                (0, "ab".into()),
                (1, "CD".into()),
                (1, "EF".into()),
                (0, "gh".into()),
            ],
            "contiguous covered spans join; the split follows coverage, not script"
        );
        // Chars NEITHER face covers stay on the primary run.
        let runs = split_runs("a中b", &[&lower, &upper]);
        assert_eq!(
            runs,
            vec![(0, "a中b".to_string())],
            "uncovered chars never switch runs"
        );
        // The N-face chain picks the FIRST covering face in order.
        let runs = split_runs("a1Ab2", &[&lower, &digit, &upper]);
        assert_eq!(
            runs,
            vec![
                (0, "a".into()),
                (1, "1".into()),
                (2, "A".into()),
                (0, "b".into()),
                (1, "2".into()),
            ],
            "chain order decides which fallback owns a run"
        );
    }

    /// #282 zh-Hans: the CJK break classes — opportunities between
    /// ideographs, kinsoku suppression before closing punctuation,
    /// never inside Latin (the space-only engine keeps its output).
    #[test]
    fn cjk_break_classes_follow_kinsoku() {
        use super::cjk_break_between as brk;
        // Between two ideographs: the workhorse opportunity.
        assert!(brk('图', '表'));
        assert!(brk('フ', 'ォ'));
        // Closing punctuation never STARTS a line, and grants the
        // break that FOLLOWS it (sentence enders).
        assert!(!brk('图', '。'));
        assert!(brk('。', '下'));
        assert!(!brk('表', '，'));
        assert!(!brk('形', ')'));
        assert!(!brk('状', '%'));
        // Opening punctuation never ENDS a line (it grants nothing)…
        assert!(!brk('（', '中'));
        // …but a line may START with one (only closers are
        // line-head-forbidden — the strict rule set).
        assert!(brk('起', '（'));
        // Latin-internal: no new opportunities.
        assert!(!brk('a', 'b'));
        assert!(!brk('1', '2'));
        // ASCII closers suppress but never GRANT — `x.y` and `3.14`
        // stay whole (CodeRabbit on #282).
        assert!(!brk('.', 'y'));
        assert!(!brk('.', '1'));
        assert!(!brk(')', 'x'));
        // …but a Han-to-Latin boundary is a real break.
        assert!(brk('图', 'a'));
        // Explicit non-breaking controls bind both sides.
        assert!(!brk('图', '\u{00A0}'));
        assert!(!brk('\u{00A0}', '表'));
        assert!(!brk('图', '\u{2060}'));
        // Combining sequences never split (decomposed ガ = カ + U+3099).
        assert!(!brk('カ', '\u{3099}'));
        assert!(!brk('\u{3099}', 'ク'));
    }

    /// #282 zh-Hans: the wrap tokenizer reproduces the space-only
    /// engine's parts verbatim for Latin (goldens pin the wrap), and
    /// splits CJK words at the kinsoku opportunities — with the
    /// U+3000 ideographic space acting as a word separator.
    #[test]
    fn wrap_tokens_split_cjk_and_preserve_latin() {
        let toks = |s: &str| {
            wrap_tokens(s)
                .into_iter()
                .map(|t| (t.sep.to_string(), t.tok.to_string()))
                .collect::<Vec<_>>()
        };
        // Pure Latin == split(' ') parts with single-space seps.
        assert_eq!(
            toks("ab cd"),
            vec![("".into(), "ab".into()), (" ".into(), "cd".into())]
        );
        // Space runs, leading and trailing spaces survive verbatim.
        assert_eq!(
            toks("a  b"),
            vec![
                ("".into(), "a".into()),
                (" ".into(), "".into()),
                (" ".into(), "b".into()),
            ]
        );
        assert_eq!(
            toks(" x"),
            vec![("".into(), "".into()), (" ".into(), "x".into())]
        );
        assert_eq!(
            toks("x "),
            vec![("".into(), "x".into()), (" ".into(), "".into())]
        );
        assert_eq!(toks(""), vec![("".into(), "".into())]);
        // CJK: every ideograph is its own token, kinsoku-glued
        // punctuation rides its neighbor.
        assert_eq!(
            toks("图表形状"),
            vec![
                ("".into(), "图".into()),
                ("".into(), "表".into()),
                ("".into(), "形".into()),
                ("".into(), "状".into()),
            ]
        );
        assert_eq!(
            toks("画图，上色。"),
            vec![
                ("".into(), "画".into()),
                ("".into(), "图，".into()),
                ("".into(), "上".into()),
                ("".into(), "色。".into()),
            ]
        );
        // Mixed: the Latin word stays one token, the Han run splits,
        // and the word's separator lands on its FIRST sub-token.
        assert_eq!(
            toks("ab 图表 cd"),
            vec![
                ("".into(), "ab".into()),
                (" ".into(), "图".into()),
                ("".into(), "表".into()),
                (" ".into(), "cd".into()),
            ]
        );
        // The ideographic space separates words like an ASCII space.
        assert_eq!(
            toks("图\u{3000}表"),
            vec![("".into(), "图".into()), ("\u{3000}".into(), "表".into())]
        );
        // A no-break space binds ideographs (CodeRabbit on #282).
        assert_eq!(
            toks("图\u{00A0}表"),
            vec![("".into(), "图\u{00A0}表".into())]
        );
        // Dotted ASCII words stay one token (no grant from '.').
        assert_eq!(
            toks("a x.y"),
            vec![("".into(), "a".into()), (" ".into(), "x.y".into())]
        );
        assert_eq!(
            toks("3.14 15"),
            vec![("".into(), "3.14".into()), (" ".into(), "15".into())]
        );
        // Decomposed kana stays one grapheme (カ + U+3099 = ガ).
        assert_eq!(
            toks("カ\u{3099}ク"),
            vec![("".into(), "カ\u{3099}ク".into())]
        );
    }


    use super::*;

    /// Local CodeRabbit round 3: the composed width run-splits like
    /// the renderer's `width_in_runs` — each run composes on the face
    /// that covers it, the terminal space comes from the final run's
    /// face. Carlito lacks U+0476 (Ѷ) while Liberation Sans carries
    /// it at a real advance, a hermetic coverage split between two
    /// vendored faces (the soft hyphen also splits them but its
    /// notdef-estimated advance collides on the grid).
    #[test]
    fn composed_width_in_runs_splits_coverage() {
        let carlito = resolve(Some("Carlito"), false, false);
        let libsans = resolve(Some("Liberation Sans"), false, false);
        assert!(
            !carlito.covers('\u{476}') && libsans.covers('\u{476}'),
            "fixture: Ѷ splits the two vendored faces"
        );
        let text = "a\u{476}b";
        let size = 12.0 / 72.0;
        // Each run composes on its covering face at the same grid.
        let want = carlito.composed_width_in("a", size)
            + libsans.composed_width_in("\u{476}", size)
            + carlito.composed_width_in("b", size);
        let got = carlito.composed_width_in_runs(&[&libsans], text, size);
        assert!(
            (got - want).abs() < 1e-12,
            "the runs compose per covering face: {got} vs {want}"
        );
        // …and that differs from single-face composition, which
        // estimates Ѷ through Carlito's missing-glyph chain instead
        // of Liberation Sans's real advance.
        let whole = carlito.composed_width_in(text, size);
        assert_ne!(
            got, whole,
            "the split run composes on the covering face: {got} vs {whole}"
        );
        // A fallback that covers nothing degenerates to the plain
        // composed width (the unregistered test world — every run
        // stays on the primary face).
        let none = resolve(Some("NoSuchFamily At All"), false, false);
        assert_eq!(
            carlito.composed_width_in_runs(&[&none], text, size),
            whole,
            "an uncovered fallback keeps plain composition"
        );
        // The N-face form: the first fallback in the chain wins the
        // run even when a later one would also cover it.
        let got2 = carlito.composed_width_in_runs(&[&none, &libsans], text, size);
        assert!(
            (got2 - want).abs() < 1e-12,
            "chain order, not coverage breadth, picks the run's face"
        );
    }

    /// The final run's face — the terminal-run space's owner.
    #[test]
    fn final_run_face_picks_the_covering_face() {
        let carlito = resolve(Some("Carlito"), false, false);
        let libsans = resolve(Some("Liberation Sans"), false, false);
        // Trailing covered run → the fallback owns the line's end.
        let chain = [&libsans];
        let ptr = carlito.final_run_face(&chain, "a\u{476}");
        assert!(std::ptr::eq(ptr, &libsans), "the final run's face wins");
        // Trailing primary run → the primary face.
        let ptr = carlito.final_run_face(&chain, "\u{476}b");
        assert!(std::ptr::eq(ptr, &carlito), "a primary-run ending stays");
    }


    /// Measurement uses the embedded Carlito everywhere (hermetic on
    /// any machine); these tests only assert relationships that hold
    /// under metric-compatible substitution too.
    fn face() -> ResolvedFace {
        resolve(None, false, false)
    }

    #[test]
    fn width_scales_linearly_with_size() {
        let f = face();
        let w1 = f.width_in("measurement", 0.13889);
        let w2 = f.width_in("measurement", 0.27778);
        assert!((w2 - w1 * 2.0).abs() < 1e-9, "{w1} vs {w2}");
        assert_eq!(f.width_in("", 0.13889), 0.0);
    }

    #[test]
    fn narrow_glyphs_measure_narrower_than_wide_ones() {
        let f = face();
        let i = f.width_in("i", 1.0);
        let m = f.width_in("m", 1.0);
        assert!(i * 2.0 < m, "i={i} m={m} — real metrics, not a flat 0.5 em");
        // And the average is in the same ballpark as the old guess.
        let avg = f.width_in("measurement", 1.0) / "measurement".len() as f64;
        assert!(avg > 0.3 && avg < 0.7, "avg={avg}");
    }

    #[test]
    fn uncovered_characters_measure_with_a_fallback() {
        let f = face();
        // CJK is absent from Carlito: the per-character fallback
        // still yields a positive width.
        assert!(f.width_in("按", 0.13889) > 0.0);
    }

    /// #232 stock-font rule: a family OUTSIDE Visio's stock set
    /// resolves to the CALIBRI chain — byte-for-byte the same face a
    /// document naming "Calibri" gets (the real installed Calibri
    /// where the host carries one, the vendored Carlito metric twin
    /// otherwise), never a wider look-alike. Machine-independent by
    /// construction: the assertion is an identity between two chains,
    /// not a pinned face id. On an embedded-only database (the
    /// harness-pinned path) that face IS the vendored Carlito.
    #[test]
    fn resolve_pins_uninstalled_families_to_the_calibri_chain() {
        let f = resolve(Some("Certainly-Not-Installed-21"), false, false);
        let c = resolve(Some("Calibri"), false, false);
        assert_eq!(
            f.id, c.id,
            "an unknown family measures exactly what Calibri measures (#232)"
        );
        assert_eq!(
            resolve(Some("Certainly-Not-Installed-21"), true, false).id,
            resolve(Some("Calibri"), true, false).id,
            "bold corners follow the same chain"
        );
        assert_eq!(
            resolve(Some("Certainly-Not-Installed-21"), false, true).id,
            resolve(Some("Calibri"), false, true).id,
            "italic corners follow the same chain"
        );
        assert_eq!(
            resolve(Some("Certainly-Not-Installed-21"), true, true).id,
            resolve(Some("Calibri"), true, true).id,
            "bold+italic corners follow the same chain"
        );
    }

    /// #232: OUR OWN infrastructure faces (the vendored floor, the
    /// RTL fallback) resolve as themselves — the stock rule must not
    /// swallow them or every RTL line would regress to Carlito.
    #[test]
    fn app_infrastructure_faces_resolve_as_themselves() {
        // On the embedded-only database Vazirmatn is absent, so the
        // resolution lands on the vendored Carlito chain end — the
        // assertion is that the CHAIN keeps Vazirmatn first (visible
        // on a system database where the face exists): the emitted
        // family chain names it.
        let chain = crate::svg::family_chain(Some("Vazirmatn"));
        assert_eq!(chain[0], "Vazirmatn", "the app face stays named");
        // Same for the vendored floor faces named directly.
        assert_eq!(crate::svg::family_chain(Some("Carlito"))[0], "Carlito");
    }

    /// #146: measurement SHAPES the run the way the rasterizer draws
    /// it. A kerned pair (AV in Liberation Serif — the vendored face
    /// carries GPOS kerning) must measure narrower than the isolated
    /// sum; this is exactly the gap that used to wrap kerned lines
    /// one word early.
    #[test]
    fn kerned_pairs_measure_kerned() {
        let f = resolve(Some("Liberation Serif"), false, false);
        let pair = f.width_in("AV", 1.0);
        let isolated = f.width_in("A", 1.0) + f.width_in("V", 1.0);
        assert!(
            pair < isolated,
            "shaped {pair} must be kerned under the isolated sum {isolated}"
        );
        // A kern-free string still measures the plain advance sum
        // (shaping agrees with the isolated tables when no feature
        // applies): same string, spaced so no pair kerns.
        let plain = f.width_in("nn", 1.0);
        let isolated_n = 2.0 * f.width_in("n", 1.0);
        assert!((plain - isolated_n).abs() < 1e-9);
    }

    /// Coverage gaps keep the per-character fallback under shaping
    /// (#146): a CJK glyph Carlito lacks still contributes its
    /// fallback advance inside a shaped run instead of vanishing.
    #[test]
    fn shaped_runs_fall_back_per_character_on_gaps() {
        let f = face();
        let latin = f.width_in("a", 1.0);
        let mixed = f.width_in("按a", 1.0);
        assert!(
            mixed > latin,
            "the missing CJK glyph must add its fallback advance: {mixed} vs {latin}"
        );
    }

    /// The resolved (style, weight) of a face, for assertions about
    /// WHICH face a chain picks (#145 verification).
    fn resolved_style(font: Option<&str>, bold: bool, italic: bool) -> (fontdb::Style, u16) {
        let m = metrics();
        let f = resolve(font, bold, italic);
        let info =
            m.db.faces()
                .find(|x| x.id == f.id)
                .expect("resolved id is in the db");
        (info.style, info.weight.0)
    }

    /// #145: the Liberation faces are vendored, so the Arial chain
    /// resolves a Liberation Sans face (the system one where
    /// installed, the vendored copy otherwise — either way NOT the
    /// Carlito the chain used to fall through to), and the style
    /// corners resolve their own faces instead of measuring upright.
    #[test]
    fn arial_chain_measures_liberation_and_styles_resolve() {
        let m = metrics();
        let f = resolve(Some("Arial"), false, false);
        assert_ne!(
            f.id, m.carlito,
            "Arial resolves Liberation Sans, not Carlito"
        );
        // It measures different advances than the Carlito fallback
        // (the two designs share Arial metrics only approximately).
        let carlito = ResolvedFace {
            id: m.carlito,
            carlito: m.carlito,
            is_carlito: true,
        };
        assert_ne!(
            f.width_in("measurement", 1.0),
            carlito.width_in("measurement", 1.0),
            "Liberation advances differ from Carlito's"
        );
        // Style corners pick the matching (style, weight) face.
        assert_eq!(
            resolved_style(Some("Arial"), false, false),
            (fontdb::Style::Normal, 400)
        );
        assert_eq!(
            resolved_style(Some("Arial"), false, true),
            (fontdb::Style::Italic, 400)
        );
        assert_eq!(
            resolved_style(Some("Arial"), true, true),
            (fontdb::Style::Italic, 700)
        );
    }
}
