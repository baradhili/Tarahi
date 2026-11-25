//! The ShapeSheet and its ordered child containers.

use super::cell::{Cell, Section};
use super::raw::{Attrs, RawEl};

/// One child of a sheet body, in document order. Real sheets interleave
/// `Cell`s with `Trigger`s (and occasionally other elements), so order is
/// part of the round-trip contract.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetChild {
    Cell(String, Cell),
    Section(Section),
    Raw(RawEl),
}

/// A ShapeSheet: an ordered child list plus the derived text fields.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    /// Opening-tag attributes (`PageSheet LineStyle='0' …`).
    pub attrs: Attrs,
    /// Cells, sections, and raw elements (`Trigger`, …) in document order.
    pub children: Vec<SheetChild>,
    /// Plain text content of `<Text>` (markup stripped) — rendering input.
    pub text: Option<String>,
    /// Raw inner XML of `<Text>` (markup, entities, CDATA verbatim) —
    /// writing output. `Some("")` for an empty `<Text/>`.
    pub text_raw: Option<String>,
}

impl Sheet {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.children.iter().find_map(|c| match c {
            SheetChild::Cell(n, cell) if n == name => Some(cell),
            _ => None,
        })
    }

    /// Insert or replace a cell by name, preserving position on replace.
    pub fn set_cell(&mut self, name: &str, cell: Cell) {
        let cell = Cell {
            attrs: {
                let mut a: Attrs = vec![("N".into(), name.to_string())];
                if let Some(v) = &cell.v {
                    a.push(("V".into(), v.clone()));
                }
                if let Some(u) = &cell.u {
                    a.push(("U".into(), u.clone()));
                }
                if let Some(f) = &cell.f {
                    a.push(("F".into(), f.clone()));
                }
                a
            },
            ..cell
        };
        for child in self.children.iter_mut() {
            if let SheetChild::Cell(n, slot) = child {
                if n == name {
                    *slot = cell;
                    return;
                }
            }
        }
        self.children.push(SheetChild::Cell(name.to_string(), cell));
    }

    /// Whether the sheet carries any section.
    pub fn has_sections(&self) -> bool {
        self.children
            .iter()
            .any(|c| matches!(c, SheetChild::Section(_)))
    }

    /// All sections with the given name, ordered by IX.
    pub fn sections_named(&self, name: &str) -> Vec<&Section> {
        let mut v: Vec<&Section> = self
            .children
            .iter()
            .filter_map(|c| match c {
                SheetChild::Section(s) if s.name == name => Some(s),
                _ => None,
            })
            .collect();
        v.sort_by_key(|s| s.ix);
        v
    }

    /// The section with exactly this (name, ix), if present.
    pub fn section(&self, name: &str, ix: i64) -> Option<&Section> {
        self.children.iter().find_map(|c| match c {
            SheetChild::Section(s) if s.name == name && s.ix == ix => Some(s),
            _ => None,
        })
    }
}

/// A child of a `<Page>`/`<Master>` index entry, in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexChild {
    /// `<PageSheet>`/`<MasterSheet>` — `tag` preserves which, `sheet.attrs`
    /// hold the opening-tag attributes.
    Sheet { tag: String, sheet: Sheet },
    /// `<Rel r:id='…'/>`.
    Rel(Attrs),
    /// Anything else (notably `<Icon>`), kept verbatim.
    Raw(RawEl),
}
