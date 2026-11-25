//! PDF export backend (#35): the parsed page SVG walks out as a PDF
//! 1.7 file — shapes and strokes as vector path operators, gradients
//! as axial/radial shading patterns, media as image XObjects, and
//! TEXT AS TEXT: every text chunk re-shapes through harfrust on the
//! same face the raster resolved (`raster::parse_svg` collapsed each
//! font-family chain to one canonical family before usvg parsed), and
//! a Type0/CID font with a ToUnicode CMap keeps the words selectable
//! and copyable — the issue's acceptance bar.
//!
//! Coordinate story: the SVG viewBox is inches, y-down; the PDF
//! MediaBox is points, y-up. The base content transform
//! `[72 0 0 −72 0 H·72]` maps one onto the other, and every node's
//! usvg ABSOLUTE transform (already resolved through all ancestors)
//! rides on top of it — no per-node composition is kept.

mod context;
mod emit;
mod font;
mod paint;
mod resources;
mod shape;
mod walk;

pub use emit::pdf_bytes;
