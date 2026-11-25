//! Shapes and shape-tree lookups.

use super::raw::{Attrs, RawEl};
use super::sheet::Sheet;

/// A shape (page-level or nested group member).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    pub id: i64,
    pub name_u: Option<String>,
    /// `Type` attribute: `Shape`, `Group`, `Foreign`, `Page`, …
    pub type_: Option<String>,
    /// `Master` attribute — master ID this shape instantiates.
    pub master: Option<i64>,
    /// `MasterShape` attribute — master sub-shape ID this shape maps to.
    pub master_shape: Option<i64>,
    /// `LineStyle` / `FillStyle` / `TextStyle` attribute defaults (IDs).
    pub line_style: Option<i64>,
    pub fill_style: Option<i64>,
    pub text_style: Option<i64>,
    /// Full original attribute list (`ID`, `NameU`, `UniqueID`, …).
    pub attrs: Attrs,
    pub sheet: Sheet,
    /// Order of the post-sheet children (`<Text>`, raw children like
    /// `<ForeignData>`, `<Shapes>`) as they appeared in the source.
    pub flow: Vec<ShapeFlow>,
    /// Unknown shape children (`ForeignData`, …), kept verbatim in order.
    pub raw_children: Vec<RawEl>,
    pub sub_shapes: Vec<Shape>,
}

/// Where a shape's `<Text>`, raw children, and `<Shapes>` appeared.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeFlow {
    Text,
    Raw(usize),
    Shapes,
}

pub fn find_shape_by_id(shapes: &[Shape], id: i64) -> Option<&Shape> {
    for s in shapes {
        if s.id == id {
            return Some(s);
        }
        if let Some(found) = find_shape_by_id(&s.sub_shapes, id) {
            return Some(found);
        }
    }
    None
}

/// Mutable counterpart of [`find_shape_by_id`].
pub fn find_shape_by_id_mut(shapes: &mut [Shape], id: i64) -> Option<&mut Shape> {
    for s in shapes {
        if s.id == id {
            return Some(s);
        }
        if let Some(found) = find_shape_by_id_mut(&mut s.sub_shapes, id) {
            return Some(found);
        }
    }
    None
}

/// Depth-first `NameU` lookup within a shape tree (used for matching inline
/// copies to their master originals).
pub fn find_shape_by_name_u<'a>(shapes: &'a [Shape], name: &str) -> Option<&'a Shape> {
    for s in shapes {
        if s.name_u.as_deref() == Some(name) {
            return Some(s);
        }
        if let Some(found) = find_shape_by_name_u(&s.sub_shapes, name) {
            return Some(found);
        }
    }
    None
}
