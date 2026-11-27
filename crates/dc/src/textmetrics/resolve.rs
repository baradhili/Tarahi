use std::sync::OnceLock;

use super::db::{is_generic, metrics, style_rank, MetricsDb};
use super::grid::{quantize18, round18, UNCOVERED_EM};
use super::split_runs;
use super::shaping::{shape_glyph_advances_em, shape_width_em, shaped_face};

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
        let advances =
            m.db.with_face_data(self.id, |data, index| {
                shaped_face(db_key, self.id, data, index)
                    .and_then(|shaped| shape_glyph_advances_em(&shaped, text, &fallback))
            })
            .flatten()
            .unwrap_or_else(|| text.chars().map(fallback).collect::<Vec<_>>());
        advances.into_iter().map(|em| round18(q * em)).sum()
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
        std::iter::once(self)
            .chain(others.iter().copied())
            .collect()
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
    pub fn width_in_runs(&self, others: &[&ResolvedFace], text: &str, size_in: f64) -> f64 {
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
    use super::*;

    /// Local CodeRabbit round 3: the composed width run-splits like
    /// the renderer's `width_in_runs` — each run composes on the face
    /// that covers it, the terminal space comes from the final run's
    /// face. Carlito lacks U+0476 (Ѷ) while Liberation Sans carries
    /// it at a real advance, a hermetic coverage split between two
    /// vendored faces (the soft hyphen also splits them but its
    /// notdef-estimated advance collides on the grid). One fixture
    /// subtlety pinned by the 2026-11-20 Windows-CI failure: Ѷ
    /// canonically decomposes (Ѵ + U+030F), so the SHAPER normalizes
    /// and finds Ѵ in Carlito at a real advance while `covers` (the
    /// direct cmap) sees nothing — and a machine's real Calibri may
    /// carry Ѷ outright, so the unknown-family probe below must
    /// expect whichever face its font set actually covers.
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
        // composed width — but the #232 rule resolves an unknown
        // family through the machine's Calibri FIRST, and a newer
        // Calibri covers Ѷ outright (the Windows runners do; the
        // Windows-CI failure of 2026-11-20 was exactly that — `none`
        // was the runner's Calibri, and chain order rightly gave it
        // the Ѷ run ahead of Liberation Sans). Assert the split's own
        // contract instead of the machine's font set: every run
        // composes on the FIRST chain face that covers it, whichever
        // face that is here.
        let none = resolve(Some("NoSuchFamily At All"), false, false);
        let none_covers = none.covers('\u{476}');
        let single = if none_covers {
            carlito.composed_width_in("a", size)
                + none.composed_width_in("\u{476}", size)
                + carlito.composed_width_in("b", size)
        } else {
            whole
        };
        assert_eq!(
            carlito.composed_width_in_runs(&[&none], text, size),
            single,
            "the single-fallback chain composes per covering face"
        );
        // The N-face form: the first fallback in the chain wins the
        // run even when a later one would also cover it — `none` on
        // machines whose Calibri carries Ѷ, Liberation Sans
        // elsewhere. (Watch the pairing stay exact: each expected
        // term is the same composed_run_in call the split makes.)
        let winner = if none_covers { &none } else { &libsans };
        let n_face = carlito.composed_width_in("a", size)
            + winner.composed_width_in("\u{476}", size)
            + carlito.composed_width_in("b", size);
        let got2 = carlito.composed_width_in_runs(&[&none, &libsans], text, size);
        assert!(
            (got2 - n_face).abs() < 1e-12,
            "chain order, not coverage breadth, picks the run's face: {got2} vs {n_face}"
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
