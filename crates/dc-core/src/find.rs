//! Find-in-drawing search engine (GitHub #115, phase 1) — headless,
//! read-only over the [`vsdx`] document model. The Find dialog (phase
//! 2) and Find & Replace (phase 3) both sit on this one matcher, so
//! what they find and what they replace are the same set by
//! construction.
//!
//! Targets (spec 2's checkbox set, in that order):
//! - **Shape text** — the same full plain projection the spell check
//!   uses (GitHub #121): field (`<fld>`) cached displays included,
//!   offsets are plain CHARACTERS, and the seed follows the spell
//!   walk's convention (local raw → escaped local plain → the master's
//!   inherited raw → the master's escaped plain).
//! - **Shape data** — `Prop` row `Value` strings.
//! - **Shape name** — `NameU`.
//! - **User-defined cell** — `User` row `Value` strings.
//!
//! Cell targets read the EFFECTIVE rows the Shape Data dialog (#30)
//! shows: the shape's own rows first, then the master-inherited rows
//! (via [`vsdx::model::Document::instance_source`]) that no local
//! value-bearing row of the same name shadows — an `F='Inh'` local row
//! has no value of its own, so the master's stays visible. Locally
//! deleted rows (`Del='1'`, #293) are gone and never searched.
//!
//! Scope (spec 2's radios): Selection / Current page / All pages —
//! always traversing nested `sub_shapes` (a selected group puts its
//! whole subtree in scope). Options: match case, find whole words
//! only. "Match character width" is the phase-2 CJK decision (issue
//! plan) and is deliberately absent here.
//!
//! Match records are the edit coordinates: a text match carries the
//! plain span [`crate::ReplaceTextRange`] splices (a field-bearing
//! shape never goes through [`crate::SetText`]'s flattening store); a
//! cell match carries the row NAME a [`crate::SetRowCell`]
//! `RowSel::Named` edit targets plus the char span inside the value
//! string, so phase 3's replace writes the whole spliced value back
//! through one command. Order is deterministic — page order,
//! shape-tree pre-order, target class, ascending span/row — so Find
//! Next cycles the same sequence every time.

use std::collections::HashSet;

use vsdx::model::{Document, Page, Shape};
use vsdx::text::text_pieces;

/// What a search covers (spec 2's "Search in" radios).
#[derive(Debug, Clone, Copy)]
pub enum FindScope<'a> {
    /// Only the selected shapes — `(page id, shape id)` pairs, the
    /// shell's `sel_set` shape — plus every shape nested inside a
    /// selected one (selecting a group selects its members).
    Selection(&'a [(i64, i64)]),
    /// One page by id (the current page).
    CurrentPage(i64),
    /// Every page in document order.
    AllPages,
}

/// Which target classes a search covers (spec 2's checkboxes; the
/// declaration order is also the per-shape match order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindTargets {
    pub text: bool,
    pub shape_data: bool,
    pub shape_name: bool,
    pub user_cell: bool,
}

impl Default for FindTargets {
    /// Shape text only — the Find dialog's default checked set
    /// (spec 2: "Shape text (Default checked)").
    fn default() -> Self {
        FindTargets {
            text: true,
            shape_data: false,
            shape_name: false,
            user_cell: false,
        }
    }
}

impl FindTargets {
    /// Every target class.
    pub fn all() -> Self {
        FindTargets {
            text: true,
            shape_data: true,
            shape_name: true,
            user_cell: true,
        }
    }
}

/// The search modifiers (spec 2's Options checkboxes). Defaults mirror
/// the dialog: case-insensitive substring search over shape text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FindOptions {
    pub targets: FindTargets,
    pub match_case: bool,
    pub whole_words: bool,
}

/// One target class of a match (spec 2's checkbox set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindTargetClass {
    Text,
    ShapeData,
    ShapeName,
    UserCell,
}

/// Where one match sits. Text spans are plain CHARACTERS over the
/// full plain projection (field cached displays included) — the
/// coordinates [`crate::ReplaceTextRange`] consumes. Cell spans are
/// characters within the row's `Value` string, and `row` is the name a
/// [`crate::SetRowCell`] `RowSel::Named` edit targets.
#[derive(Debug, Clone, PartialEq)]
pub enum FindTarget {
    Text {
        start: usize,
        len: usize,
    },
    ShapeData {
        row: String,
        start: usize,
        len: usize,
    },
    ShapeName {
        start: usize,
        len: usize,
    },
    UserCell {
        row: String,
        start: usize,
        len: usize,
    },
}

impl FindTarget {
    pub fn class(&self) -> FindTargetClass {
        match self {
            FindTarget::Text { .. } => FindTargetClass::Text,
            FindTarget::ShapeData { .. } => FindTargetClass::ShapeData,
            FindTarget::ShapeName { .. } => FindTargetClass::ShapeName,
            FindTarget::UserCell { .. } => FindTargetClass::UserCell,
        }
    }
}

/// One found occurrence: the owning page and shape plus the target
/// coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct FindMatch {
    pub page: i64,
    pub shape: i64,
    pub target: FindTarget,
}

/// Search `doc` for `needle` and return every match in Find-Next
/// order: page order, shape-tree pre-order, target class, ascending
/// span/row. An empty needle matches nothing (the dialog's Find Next
/// with an empty box is a no-op).
pub fn find_matches(
    doc: &Document,
    needle: &str,
    scope: FindScope<'_>,
    opts: &FindOptions,
) -> Vec<FindMatch> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    // Scope → the pages to walk, in document order.
    let selected_pages: Option<HashSet<i64>> = match &scope {
        FindScope::Selection(pairs) => Some(pairs.iter().map(|(p, _)| *p).collect()),
        _ => None,
    };
    let pages: Vec<&Page> = doc
        .pages
        .iter()
        .filter(|p| match &scope {
            FindScope::AllPages => true,
            FindScope::CurrentPage(id) => p.id == *id,
            FindScope::Selection(_) => selected_pages
                .as_ref()
                .is_some_and(|set| set.contains(&p.id)),
        })
        .collect();
    for page in pages {
        let sel: Option<HashSet<i64>> = match &scope {
            FindScope::Selection(pairs) => Some(
                pairs
                    .iter()
                    .filter(|(p, _)| *p == page.id)
                    .map(|(_, s)| *s)
                    .collect(),
            ),
            _ => None,
        };
        let search = Search {
            doc,
            needle,
            opts,
            sel: sel.as_ref(),
        };
        let mut chain: Vec<&Shape> = Vec::new();
        visit(&search, page.id, &page.shapes, &mut chain, false, &mut out);
    }
    out
}

/// The walk's immutable context: the document, the needle, the
/// options, and the page's selection filter (if any).
struct Search<'a> {
    doc: &'a Document,
    needle: &'a str,
    opts: &'a FindOptions,
    sel: Option<&'a HashSet<i64>>,
}

/// Depth-first pre-order walk of a shape tree. `inside` marks that an
/// ancestor is in a Selection scope (its whole subtree searches); a
/// shape with no selection filter at all is always visited.
fn visit<'a>(
    search: &Search<'_>,
    page: i64,
    shapes: &'a [Shape],
    chain: &mut Vec<&'a Shape>,
    inside: bool,
    out: &mut Vec<FindMatch>,
) {
    for s in shapes {
        let now = match search.sel {
            None => true,
            Some(set) => inside || set.contains(&s.id),
        };
        chain.push(s);
        if now {
            let ancestors = &chain[..chain.len() - 1];
            shape_matches(search, page, s, ancestors, out);
        }
        visit(search, page, &s.sub_shapes, chain, now, out);
        chain.pop();
    }
}

/// Every match inside one shape, in target-class order (spec 2's
/// checkbox order: text, shape data, shape name, user-defined cell).
fn shape_matches(
    search: &Search<'_>,
    page: i64,
    shape: &Shape,
    ancestors: &[&Shape],
    out: &mut Vec<FindMatch>,
) {
    let (doc, needle, opts) = (search.doc, search.needle, search.opts);
    if opts.targets.text {
        if let Some(raw) = effective_text_raw(doc, shape, ancestors) {
            let plain: String = text_pieces(&raw).iter().map(|p| p.plain()).collect();
            for (start, len) in spans(&plain, needle, opts) {
                out.push(FindMatch {
                    page,
                    shape: shape.id,
                    target: FindTarget::Text { start, len },
                });
            }
        }
    }
    if opts.targets.shape_data {
        for (row, start, len) in row_value_matches(doc, shape, ancestors, "Prop", needle, opts) {
            out.push(FindMatch {
                page,
                shape: shape.id,
                target: FindTarget::ShapeData { row, start, len },
            });
        }
    }
    if opts.targets.shape_name {
        if let Some(name) = shape.name_u.as_deref() {
            for (start, len) in spans(name, needle, opts) {
                out.push(FindMatch {
                    page,
                    shape: shape.id,
                    target: FindTarget::ShapeName { start, len },
                });
            }
        }
    }
    if opts.targets.user_cell {
        for (row, start, len) in row_value_matches(doc, shape, ancestors, "User", needle, opts) {
            out.push(FindMatch {
                page,
                shape: shape.id,
                target: FindTarget::UserCell { row, start, len },
            });
        }
    }
}

/// The raw text a shape effectively carries — the spell check's
/// projection (GitHub #121): local raw → escaped local plain → the
/// master's inherited raw → the master's escaped plain. Field markers
/// and cached displays stay in the string the match spans index.
fn effective_text_raw(doc: &Document, shape: &Shape, ancestors: &[&Shape]) -> Option<String> {
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    if let Some(raw) = &shape.sheet.text_raw {
        return Some(raw.clone());
    }
    if let Some(t) = &shape.sheet.text {
        return Some(escape(t));
    }
    let src = doc.instance_source(shape, ancestors)?;
    src.sheet
        .text_raw
        .clone()
        .or_else(|| src.sheet.text.clone().map(|t| escape(&t)))
}

/// Matches over one named-section family's row `Value` strings: the
/// shape's own rows (section IX order, then row order), then the
/// master-inherited rows no local value-bearing row of the same name
/// shadows — the Shape Data dialog's effective view (#30). Deleted
/// rows (#293) and value-less rows never match; a span's coordinates
/// are characters within the value string.
fn row_value_matches(
    doc: &Document,
    shape: &Shape,
    ancestors: &[&Shape],
    section: &str,
    needle: &str,
    opts: &FindOptions,
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut shadowed: HashSet<String> = HashSet::new();
    scan_rows(
        &shape.sheet,
        section,
        needle,
        opts,
        true,
        &mut shadowed,
        &mut out,
    );
    if let Some(src) = doc.instance_source(shape, ancestors) {
        scan_rows(
            &src.sheet,
            section,
            needle,
            opts,
            false,
            &mut shadowed,
            &mut out,
        );
    }
    out
}

/// One sheet's rows of a named section, in section-IX then row order.
/// `shadow` records row names (the local pass) so the master's
/// same-named rows drop out; the inherited pass leaves `shadowed`
/// untouched.
fn scan_rows(
    sheet: &vsdx::model::Sheet,
    section: &str,
    needle: &str,
    opts: &FindOptions,
    shadow: bool,
    shadowed: &mut HashSet<String>,
    out: &mut Vec<(String, usize, usize)>,
) {
    for sec in sheet.sections_named(section) {
        for row in &sec.rows {
            let Some(name) = row.n.as_deref() else {
                continue;
            };
            if row.is_deleted() {
                // A local tombstone deletes the master's row too
                // (#293: Visio honors `Del` on open) — record the
                // name so the inherited pass skips it. Only the local
                // pass records; a deleted MASTER row is simply gone.
                if shadow {
                    shadowed.insert(name.to_string());
                }
                continue;
            }
            let Some(value) = value_of(row) else {
                continue;
            };
            if shadow {
                shadowed.insert(name.to_string());
            } else if shadowed.contains(name) {
                continue; // a local value-bearing row of this name wins
            }
            for (start, len) in spans(value, needle, opts) {
                out.push((name.to_string(), start, len));
            }
        }
    }
}

/// A row's `Value` cell value string, when the cell carries one (a
/// formula-only or `F='Inh'` cell has nothing to match — the master's
/// value shows through instead).
fn value_of(row: &vsdx::model::Row) -> Option<&str> {
    row.cell("Value").and_then(|c| c.v.as_deref())
}

/// Every occurrence of `needle` in `hay`, leftmost-first and
/// non-overlapping, as CHARACTER spans. Case-insensitive matching uses
/// a simple per-character fold (`char::to_lowercase`'s first char —
/// 1:1, so span offsets stay aligned with the original characters);
/// whole-word matching requires a non-word neighbor (or an edge) on
/// both sides, with word characters being Unicode alphanumerics plus
/// `_`.
pub fn spans(hay: &str, needle: &str, opts: &FindOptions) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let hay: Vec<char> = hay.chars().collect();
    let eq = |a: char, b: char| {
        if opts.match_case {
            a == b
        } else {
            fold(a) == fold(b)
        }
    };
    let n = needle.len();
    let mut i = 0usize;
    while i + n <= hay.len() {
        let hit = hay[i..i + n].iter().zip(&needle).all(|(a, b)| eq(*a, *b));
        if hit && (!opts.whole_words || word_edge(&hay, i, i + n)) {
            out.push((i, n));
            i += n; // non-overlapping: the next scan starts past the hit
        } else {
            i += 1;
        }
    }
    out
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_edge(hay: &[char], start: usize, end: usize) -> bool {
    (start == 0 || !is_word_char(hay[start - 1])) && (end == hay.len() || !is_word_char(hay[end]))
}
