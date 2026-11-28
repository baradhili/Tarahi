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
//! whole subtree in scope). Options: match case, match character
//! width, find whole words only.
//!
//! The needle takes Visio's caret codes (the Find dialog's Special
//! menu inserts them): `^t` tab, `^p` manual return (a `\n` in the
//! text runs), `^-` optional hyphen (U+00AD), `^^` a literal caret,
//! `^?` any ONE character (a wildcard). A caret ahead of anything
//! else — or at the needle's end — is a literal caret. "Match
//! character width" is the East Asian width fold: the fullwidth
//! ASCII range (U+FF01..U+FF5E) compares against its halfwidth
//! counterpart and the ideographic space (U+3000) against the plain
//! space.
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
    /// East Asian width folding — fullwidth forms compare against
    /// their halfwidth counterparts (spec 2's "Match character
    /// width").
    pub match_width: bool,
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

/// One needle token after caret-code compilation: a literal
/// character or the any-character wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Lit(char),
    /// `^?` — matches exactly one character.
    Any,
}

/// Compile a needle into match tokens, interpreting the caret codes
/// the Special menu inserts: `^t` tab, `^p` manual return, `^-`
/// optional hyphen, `^^` literal caret, `^?` any one character. A
/// caret ahead of anything else — or trailing — stays literal.
fn compile(needle: &str) -> Vec<Tok> {
    let chars: Vec<char> = needle.chars().collect();
    let mut toks = Vec::with_capacity(chars.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '^' {
            let special = match chars.get(i + 1) {
                Some('t') => Some(Tok::Lit('\t')),
                Some('p') => Some(Tok::Lit('\n')),
                Some('-') => Some(Tok::Lit('\u{00AD}')),
                Some('^') => Some(Tok::Lit('^')),
                Some('?') => Some(Tok::Any),
                _ => None,
            };
            if let Some(tok) = special {
                toks.push(tok);
                i += 2;
                continue;
            }
            // else: a literal caret; the next char is its own token
        }
        toks.push(Tok::Lit(c));
        i += 1;
    }
    toks
}

/// Every occurrence of `needle` in `hay`, leftmost-first and
/// non-overlapping, as CHARACTER spans. Case-insensitive matching uses
/// a simple per-character fold (`char::to_lowercase`'s first char —
/// 1:1, so span offsets stay aligned with the original characters);
/// whole-word matching requires a non-word neighbor (or an edge) on
/// both sides, with word characters being Unicode alphanumerics plus
/// `_`. Every caret-code token — including the `^?` wildcard — is one
/// character, so a span's length is always the needle's.
pub fn spans(hay: &str, needle: &str, opts: &FindOptions) -> Vec<(usize, usize)> {
    let toks = compile(needle);
    let mut out = Vec::new();
    if toks.is_empty() {
        return out;
    }
    let hay: Vec<char> = hay.chars().collect();
    let eq = |a: char, t: &Tok| match t {
        Tok::Any => true,
        Tok::Lit(b) => {
            if a == *b {
                return true;
            }
            let (a, b) = if opts.match_width {
                (fold_width(a), fold_width(*b))
            } else {
                (a, *b)
            };
            if a == b {
                return true;
            }
            !opts.match_case && fold_case(a) == fold_case(b)
        }
    };
    let n = toks.len();
    let mut i = 0usize;
    while i + n <= hay.len() {
        let hit = hay[i..i + n].iter().zip(&toks).all(|(a, t)| eq(*a, t));
        if hit && (!opts.whole_words || word_edge(&hay, i, i + n)) {
            out.push((i, n));
            i += n; // non-overlapping: the next scan starts past the hit
        } else {
            i += 1;
        }
    }
    out
}

fn fold_case(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The East Asian width fold ("match character width"): fullwidth
/// ASCII (U+FF01..U+FF5E, ！..～) maps onto its halfwidth counterpart
/// and the ideographic space (U+3000) onto the plain space.
fn fold_width(c: char) -> char {
    let cp = c as u32;
    match cp {
        0xFF01..=0xFF5E => char::from_u32(cp - 0xFF00 + 0x20).unwrap_or(c),
        0x3000 => ' ',
        _ => c,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_edge(hay: &[char], start: usize, end: usize) -> bool {
    (start == 0 || !is_word_char(hay[start - 1])) && (end == hay.len() || !is_word_char(hay[end]))
}

// ---------------------------------------------------------------------------
// Replace (#115 phase 3) — match records become commands. Text matches
// splice through [`crate::ReplaceTextRange`] (field markers survive);
// cell matches write the spliced value through [`crate::SetRowCell`]
// on the instance row by name (a master-inherited row materializes).
// ---------------------------------------------------------------------------

/// Decode a REPLACEMENT string's caret codes into the literal
/// characters written into the document: `^t` tab, `^p` manual
/// return, `^-` optional hyphen, `^^` caret. `^?` is a search
/// wildcard and stays the two typed characters here; any other caret
/// sequence keeps its caret literal (the needle compiler's rule).
pub fn decode_replacement(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '^' {
            let lit = match chars.get(i + 1) {
                Some('t') => Some('\t'),
                Some('p') => Some('\n'),
                Some('-') => Some('\u{00AD}'),
                Some('^') => Some('^'),
                _ => None,
            };
            if let Some(l) = lit {
                out.push(l);
                i += 2;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Resolve a named `Prop`/`User` row for the replace path: the
/// section IX its write targets and the effective value string the
/// matcher saw — the local value-bearing row first, else the
/// master-inherited row of the same name (the matcher's layering).
/// A locally deleted row resolves nowhere (the tombstone shadows).
fn resolve_cell(
    doc: &Document,
    page: i64,
    shape: i64,
    section: &str,
    row: &str,
) -> Option<(i64, String)> {
    let p = doc.pages.iter().find(|p| p.id == page)?;
    let sh = vsdx::model::find_shape_by_id(&p.shapes, shape)?;
    let hit = |sheet: &vsdx::model::Sheet| -> Option<(i64, String)> {
        for sec in sheet.sections_named(section) {
            for r in &sec.rows {
                if r.n.as_deref() == Some(row) && !r.is_deleted() {
                    if let Some(v) = value_of(r) {
                        return Some((sec.ix, v.to_string()));
                    }
                }
            }
        }
        None
    };
    // A LOCAL TOMBSTONE is terminal (#293, the matcher's rule): the
    // row is deleted in the effective view, and the inherited
    // fallback must not resurrect it into a replace write
    // (CodeRabbit round on phase 3). A non-deleted local row without
    // a Value (F='Inh') still falls through to the master.
    let tombstoned = sh.sheet.sections_named(section).iter().any(|sec| {
        sec.rows
            .iter()
            .any(|r| r.n.as_deref() == Some(row) && r.is_deleted())
    });
    if tombstoned {
        return None;
    }
    hit(&sh.sheet).or_else(|| {
        let chain = crate::scope::shape_chain(doc, page, shape).ok()?;
        let ancestors: Vec<_> = chain.into_iter().filter(|c| c.id != shape).collect();
        let src = doc.instance_source(sh, &ancestors)?;
        hit(&src.sheet)
    })
}

/// The command replacing ONE match (one undo step). `None` when the
/// match's row no longer resolves (the document moved under the
/// dialog — the caller recomputes).
pub fn replace_command(
    doc: &Document,
    m: &FindMatch,
    replacement: &str,
) -> Option<Box<dyn crate::Command>> {
    let scope = crate::Scope::Shape {
        page: m.page,
        shape: m.shape,
    };
    let replacement = decode_replacement(replacement);
    match &m.target {
        FindTarget::Text { start, len } => Some(Box::new(crate::ReplaceTextRange::new(
            scope,
            *start,
            *len,
            replacement,
        ))),
        FindTarget::ShapeData { row, start, len } | FindTarget::UserCell { row, start, len } => {
            let section = match m.target.class() {
                FindTargetClass::ShapeData => "Prop",
                _ => "User",
            };
            let (ix, value) = resolve_cell(doc, m.page, m.shape, section, row)?;
            let new = splice(&value, *start, *len, &replacement);
            Some(Box::new(crate::SetRowCell::new(
                scope,
                section,
                ix,
                crate::commands::RowSel::Named(row.clone()),
                "Value",
                vsdx::model::Cell::value(new),
            )))
        }
        // A name match replaces nothing editable (NameU is authored,
        // not replaceable text) — the Replace buttons skip it.
        FindTarget::ShapeName { .. } => None,
    }
}

/// One text with `[start, start+len)` replaced (CHAR offsets).
fn splice(value: &str, start: usize, len: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(value.len());
    out.extend(value.chars().take(start));
    out.push_str(replacement);
    out.extend(value.chars().skip(start + len));
    out
}

/// The single-undo Replace All sweep (#115 phase 3): every scoped
/// match, in the deterministic walk order, as ONE [`crate::Macro`] —
/// one undo step restores the exact prior state. Within one shape
/// the text splices run LAST-match-first so earlier spans keep their
/// offsets; a row with several matches becomes ONE full-value write.
/// Name matches are skipped (not replaceable). The count of
/// replacements the sweep will make (for the status note).
pub fn replace_all_command(
    doc: &Document,
    matches: &[FindMatch],
    replacement: &str,
) -> (Option<crate::Macro>, usize) {
    let replacement = decode_replacement(replacement);
    let mut cmds: Vec<Box<dyn crate::Command>> = Vec::new();
    let mut count = 0usize;
    // Per shape: the text matches (descending start) and the per-row
    // cell splices, in walk order.
    let mut i = 0usize;
    while i < matches.len() {
        let (page, shape) = (matches[i].page, matches[i].shape);
        let mut texts: Vec<(usize, usize)> = Vec::new(); // (start, len)
        let mut cells: Vec<CellRow> = Vec::new();
        while i < matches.len() && matches[i].page == page && matches[i].shape == shape {
            match &matches[i].target {
                FindTarget::Text { start, len } => texts.push((*start, *len)),
                FindTarget::ShapeData { row, start, len } => {
                    bucket_cell(doc, &mut cells, (page, shape), "Prop", row, *start, *len);
                }
                FindTarget::UserCell { row, start, len } => {
                    bucket_cell(doc, &mut cells, (page, shape), "User", row, *start, *len);
                }
                FindTarget::ShapeName { .. } => {}
            }
            i += 1;
        }
        let scope = crate::Scope::Shape { page, shape };
        // Text: descending start — each splice leaves earlier spans'
        // offsets untouched.
        texts.sort_by_key(|(start, _)| std::cmp::Reverse(*start));
        for (start, len) in texts {
            cmds.push(Box::new(crate::ReplaceTextRange::new(
                scope.clone(),
                start,
                len,
                replacement.clone(),
            )));
            count += 1;
        }
        // Cells: one full-value write per row, all its spans spliced.
        for cell in &mut cells {
            cell.spans
                .sort_by_key(|(start, _)| std::cmp::Reverse(*start));
            for (start, len) in cell.spans.clone() {
                cell.value = splice(&cell.value, start, len, &replacement);
                count += 1;
            }
            cmds.push(Box::new(crate::SetRowCell::new(
                scope.clone(),
                cell.section,
                cell.ix,
                crate::commands::RowSel::Named(cell.row.clone()),
                "Value",
                vsdx::model::Cell::value(cell.value.clone()),
            )));
        }
    }
    let macro_ = if cmds.is_empty() {
        None
    } else {
        Some(crate::Macro::new(
            format!("replace all {:?}", replacement),
            cmds,
        ))
    };
    (macro_, count)
}

/// One row's pending Replace All writes: the section and its IX, the
/// effective value the spans index, and the spans.
struct CellRow {
    row: String,
    section: &'static str,
    ix: i64,
    value: String,
    spans: Vec<(usize, usize)>,
}

/// Add one match's span to its row's bucket, resolving the row's
/// section IX and effective value on first sight.
fn bucket_cell(
    doc: &Document,
    cells: &mut Vec<CellRow>,
    at: (i64, i64),
    section: &'static str,
    row: &str,
    start: usize,
    len: usize,
) {
    if let Some(bucket) = cells
        .iter_mut()
        .find(|c| c.row == row && c.section == section)
    {
        bucket.spans.push((start, len));
        return;
    }
    if let Some((ix, value)) = resolve_cell(doc, at.0, at.1, section, row) {
        cells.push(CellRow {
            row: row.to_string(),
            section,
            ix,
            value,
            spans: vec![(start, len)],
        });
    }
}
