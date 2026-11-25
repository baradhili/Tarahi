//! A master (stencil shape definition).

use super::raw::{Attrs, RawEl};
use super::shape::Shape;
use super::sheet::{IndexChild, Sheet};

/// A master (stencil shape definition): its index entry and shape tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Master {
    pub id: i64,
    pub name_u: Option<String>,
    /// Masters-index `<Master>` attributes, in order.
    pub attrs: Attrs,
    /// Index children (`MasterSheet`, `Rel`, `Icon`, …) in order.
    pub children: Vec<IndexChild>,
    /// The master's shape tree from its part (usually one root shape).
    pub shapes: Vec<Shape>,
    /// Sheet from the master part itself, if any (rare).
    pub contents_sheet: Sheet,
    /// Opening-tag attrs of the `<MasterContents>` root.
    pub contents_attrs: Attrs,
    /// Unknown children of the contents root, verbatim.
    pub contents_raw: Vec<RawEl>,
    /// Part name the master contents were parsed from.
    pub part: String,
}

impl Master {
    /// The master's sheet: the index `PageSheet`/`MasterSheet` when present,
    /// else the part's own sheet.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.children.iter().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }
}
