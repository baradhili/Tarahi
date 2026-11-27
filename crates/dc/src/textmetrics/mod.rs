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

mod db;
mod grid;
mod resolve;
mod runs;
mod shaping;
mod wrap;

// Stable re-exports: every path under `textmetrics::` keeps resolving
// exactly as before the split — the named `pub use`s are the public
// API, and the crate-level globs carry the crate-internal helpers to
// the sibling modules (a `pub` glob cannot re-export less-visible
// items).
pub(crate) use runs::*;
pub(crate) use wrap::*;

pub use db::{pin_embedded_for_tests, register_face_bytes};
pub use grid::{quantize18, trunc18};
pub use resolve::{face_data, resolve, FaceData, ResolvedFace};
