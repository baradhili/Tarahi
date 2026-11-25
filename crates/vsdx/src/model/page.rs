//! A page: its pages-index entry plus the shape tree from its part.

use super::raw::{Attrs, RawEl};
use super::shape::Shape;
use super::sheet::{IndexChild, Sheet};

/// A page: its pages-index entry plus the shape tree from its part.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub id: i64,
    pub name: String,
    /// Pages-index `<Page>` attributes (`ID`, `NameU`, `ViewScale`, …).
    pub attrs: Attrs,
    /// Index children (`PageSheet`, `Rel`, …) in order.
    pub children: Vec<IndexChild>,
    pub shapes: Vec<Shape>,
    /// Opening-tag attrs of the `<PageContents>` root.
    pub contents_attrs: Attrs,
    /// Unknown children of the contents root (`<Connects>`, …), verbatim.
    pub contents_raw: Vec<RawEl>,
    /// Part name the page contents were parsed from ("" for stencil pages
    /// that have no page part).
    pub part: String,
}

impl Page {
    /// The page's line-jump display switch (#251): the PageSheet's
    /// `LineJumpCode` (the PageLayout-section cell — the oracle
    /// enumeration of a real Visio-authored page carried
    /// `LineJumpCode = 1`, Visio's "horizontal lines jump" default).
    /// Absent shows (the shipped corpus carries no jump cells at
    /// all). **0 = visPLOJumpNone** — the documented off switch
    /// ([MS-Learn] LineJumpCode cell, Page Layout section; values
    /// 0–5). 6 stays treated as off too (#218 review: files written
    /// by tools that use it as "none" keep their intent).
    pub fn line_jumps_shown(&self) -> bool {
        self.sheet()
            .and_then(|s| s.cell("LineJumpCode"))
            .and_then(|c| c.numeric())
            .map(|v| v != 0.0 && v != 6.0)
            .unwrap_or(true)
    }

    /// The page's sheet: the inline `<PageSheet>` from the pages index.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.children.iter().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }

    /// [`Page::sheet`] for mutation (page-default cells a writer or
    /// test sets in place).
    pub fn sheet_mut(&mut self) -> Option<&mut Sheet> {
        self.children.iter_mut().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }
}
