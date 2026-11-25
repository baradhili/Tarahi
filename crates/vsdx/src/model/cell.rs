//! ShapeSheet cells, rows, and sections.

use super::raw::Attrs;

/// One ShapeSheet cell: `N` name, `V` value, `U` unit, `F` formula.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    pub v: Option<String>,
    pub u: Option<String>,
    pub f: Option<String>,
    /// Full original attribute list (`N`, `V`, `U`, `F`, `E`, …) in
    /// document order; kept so writing is lossless.
    pub attrs: Attrs,
    /// Raw inner XML when the cell has children (`<RefBy/>`), verbatim.
    pub raw_inner: Option<String>,
}

impl Cell {
    /// A plain value cell (no unit, no formula) — for tests and editing.
    pub fn value(v: impl Into<String>) -> Cell {
        Cell {
            v: Some(v.into()),
            ..Cell::default()
        }
    }
    pub fn numeric(&self) -> Option<f64> {
        self.v.as_deref().and_then(|v| v.trim().parse::<f64>().ok())
    }
    pub fn is_inherited(&self) -> bool {
        self.f.as_deref() == Some("Inh")
    }
    /// `V='Themed'`: the value comes from the document theme through the
    /// style chain (`THEMEVAL()` on the Theme style, `F='Inh'` below
    /// it). Without an evaluated theme it carries no value of its own —
    /// resolution continues to the next style up, ending at the No Style
    /// defaults (Visio 2021 witness: Basic/Network stencil masters
    /// dropped into a theme-less drawing paint white fills and black
    /// lines, never solid black).
    pub fn is_themed(&self) -> bool {
        self.v.as_deref().map(str::trim) == Some("Themed")
    }
    /// A Themed cell that EVALUATES the theme (`THEMEVAL()`, not `Inh`):
    /// without an evaluated theme it takes the No Style value outright
    /// — Visio 2021 paints a Box master (`FillForegnd=THEMEVAL()` on the
    /// master shape, style "Block Normal" with a cached tint) plain
    /// white in a theme-less drawing, never the style's tint.
    pub fn is_theme_eval(&self) -> bool {
        self.is_themed() && !self.is_inherited()
    }
}

/// A row inside a section (optionally typed, e.g. `T='MoveTo'`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// Row type (`MoveTo`, `LineTo`, …) for geometry rows; `None` otherwise.
    pub t: Option<String>,
    /// Row index within the section (the `IX` attribute), 0 when absent.
    pub ix: i64,
    /// Named row (`N='Label'`) for named sections (User, Prop, …).
    pub n: Option<String>,
    /// Full original attribute list, in document order.
    pub attrs: Attrs,
    pub cells: Vec<(String, Cell)>,
}

impl Row {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }

    /// `Del="1"` — the row is deleted locally (GitHub #293). The
    /// attribute is schema-defined on `Row_Type` (xsd:boolean) and the
    /// shipped corpus carries it pervasively: the built-in catalog's
    /// 371 stencils — repackaged from the shipped Visio 2016 masters —
    /// hold ~44,000 deleted rows across 79 stencils, and Visio honors
    /// them on open (the #293 oracle: the deleted connector row renders
    /// gone in Visio 2021). `xsd:boolean` accepts both spellings.
    pub fn is_deleted(&self) -> bool {
        self.attrs
            .iter()
            .find(|(n, _)| n == "Del")
            .and_then(|(_, v)| match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" => Some(true),
                "0" | "false" => Some(false),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Drop the local deletion marker (an edit that needs the row
    /// again un-deletes it — GitHub #293's Expected).
    pub fn undelete(&mut self) {
        self.attrs.retain(|(n, _)| n != "Del");
    }
}

/// A sheet section, e.g. `Geometry`, `User`, `Character`, `Connection`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    /// Section name (`N` attribute).
    pub name: String,
    /// Section index (`IX`), 0 for the first Geometry, etc.
    pub ix: i64,
    /// Full original attribute list, in document order.
    pub attrs: Attrs,
    /// Section-level cells (e.g. Geometry `NoFill`/`NoLine`), in order.
    pub cells: Vec<(String, Cell)>,
    pub rows: Vec<Row>,
}

impl Section {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }
    pub fn row_by_ix(&self, ix: i64) -> Option<&Row> {
        self.rows.iter().find(|r| r.ix == ix)
    }
    pub fn named_row(&self, name: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.n.as_deref() == Some(name))
    }
}
