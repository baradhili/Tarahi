//! The VSDX document model: sheets of cells/sections, with master and
//! stylesheet inheritance resolution.
//!
//! v0.2: every container is ordered and every element keeps its original
//! attribute list, so a parsed document can be written back without data
//! loss (unknown cells, attributes, and whole elements survive verbatim).
//!
//! Layout: [`raw`] the verbatim-kept elements and ordered attribute
//! lists; [`cell`] the cells, rows, and sections; [`sheet`] the
//! ShapeSheet and its child containers; [`shape`]/[`master`]/[`page`]/
//! [`style`] the document entities; [`document`] the parsed document and
//! master/style inheritance resolution; [`env`] the instance evaluation
//! environments; and [`snap_glue`] the DocumentSettings snap & glue
//! settings.

mod cell;
mod document;
mod env;
mod master;
mod page;
mod raw;
mod shape;
mod sheet;
mod snap_glue;
mod style;

pub use cell::{Cell, Row, Section};
pub use document::{DocItem, Document, MediaRef};
pub use env::{instance_env, qualified_env};
pub use master::Master;
pub use page::Page;
pub use raw::{Attrs, RawEl};
pub use shape::{find_shape_by_id, find_shape_by_id_mut, find_shape_by_name_u, Shape, ShapeFlow};
pub use sheet::{IndexChild, Sheet, SheetChild};
pub use snap_glue::{glue_bits, snap_bits, DocSnapGlue};
pub use style::{builtin_defaults, StyleSheet};

#[cfg(test)]
mod tests;
