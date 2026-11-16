//! Field substitution (#267): a shape's `<fld>` text markers resolve
//! to their Field rows, evaluate, and format — the substituted string
//! is what the renderer lays out, wraps, and paints, and what the
//! TEXTWIDTH/TEXTHEIGHT measurement bridge sees (the measure measures
//! what renders).
//!
//! Evaluation is best-effort by construction: a `Value` formula the
//! evaluator cannot resolve (unregistered function, missing cell,
//! `NOW()`'s absent clock) falls back to the marker's cached content
//! (`<fld IX='0'>4/17</fld>`), then the Field row's cached `Value` V,
//! then empty — every corpus package renders exactly as it did before
//! this module existed unless the field CAN be evaluated.

use crate::svg::shape_env;
use vsdx::formula::{eval_field, FieldCtx, StrEnv};
use vsdx::model::{find_shape_by_id, Document, Row, Shape};

/// The no-metrics bridge: field formulas never carry
/// TEXTWIDTH/TEXTHEIGHT in the corpus census, and `TheText` inside a
/// field would recurse (the field renders INSIDE the text) — treat
/// text functions as unresolved so they fall back to the cached V.
struct NoMetrics;

impl vsdx::formula::TextMeasure for NoMetrics {
    fn the_text(&self) -> Option<String> {
        None
    }
    fn text_width(&self, _text: &str) -> Option<f64> {
        None
    }
    fn text_height(&self, _text: &str, _width: f64) -> Option<f64> {
        None
    }
}

/// The page-level field context: page identity and the document
/// facts. `page_ix` is 0-based (Visio's PAGENUMBER() is 1-based).
pub fn page_ctx(page: &vsdx::Page, page_ix: usize) -> vsdx::formula::FieldCtx {
    vsdx::formula::FieldCtx {
        page_name: Some(page.name.clone()),
        page_number: Some(page_ix as f64 + 1.0),
        master_name: None,
        doc_last_save: None,
        doc_last_edit: None,
    }
}

/// Per-shape context: the master name joined to the page context.
fn shape_ctx(base: &FieldCtx, doc: &Document, s: &Shape) -> FieldCtx {
    let mut ctx = base.clone();
    if let Some(mid) = s.master {
        if let Some(m) = doc.masters.iter().find(|m| m.id == mid) {
            ctx.master_name = m.name_u.clone();
        }
    }
    if ctx.master_name.is_none() {
        // Visio's sentinel for a masterless shape (the corpus's
        // STRSAME(MASTERNAME(),"<no master>") guards compare it).
        ctx.master_name = Some("<no master>".to_string());
    }
    ctx
}

/// Whether the shape's text carries any field marker — the fast-path
/// gate so the 499-of-679 marker-free packages pay nothing.
pub fn has_fields(s: &Shape) -> bool {
    s.sheet
        .text_raw
        .as_deref()
        .is_some_and(|raw| raw.contains("<fld"))
}

/// The substituted text: every `<fld>` marker replaced by its
/// evaluated, formatted value; plain runs pass through byte-for-byte.
/// `plain` is the markup-stripped `sheet.text` (the fallback when the
/// raw form is absent).
pub fn substituted_text(
    doc: &Document,
    page_shapes: &[Shape],
    ctx: &FieldCtx,
    s: &Shape,
    chain: &[&Shape],
    plain: &str,
) -> String {
    let Some(raw) = s.sheet.text_raw.as_deref() else {
        return plain.to_string();
    };
    if !raw.contains("<fld") {
        return plain.to_string();
    }
    let ctx = shape_ctx(ctx, doc, s);
    let mut strs = str_env(doc, s, chain);
    let env = shape_env(doc, s, chain);
    let mut out = String::new();
    for piece in vsdx::text_pieces(raw) {
        match piece.fld {
            Some(ix) => out.push_str(&field_value(
                doc, page_shapes, &ctx, s, chain, &env, &mut strs, ix,
                piece.cached.as_deref(),
            )),
            None => out.push_str(&piece.text),
        }
    }
    out
}

/// One field's rendered string: resolve the row through the instance
/// → master chain, evaluate `Value`'s formula, format through the
/// `Format` picture; fall back cached.
#[allow(clippy::too_many_arguments)]
fn field_value(
    doc: &Document,
    page_shapes: &[Shape],
    ctx: &FieldCtx,
    s: &Shape,
    chain: &[&Shape],
    env: &vsdx::formula::Env,
    strs: &mut StrEnv,
    ix: usize,
    marker_cached: Option<&str>,
) -> String {
    let Some(row) = resolve_field_row(doc, s, chain, ix) else {
        return marker_cached.unwrap_or_default().to_string();
    };
    let value_cell = row.cell("Value");
    let formula = value_cell
        .and_then(|c| c.f.as_deref())
        .filter(|f| !f.is_empty() && *f != "Inh" && *f != "No Formula");
    let picture = row
        .cell("Format")
        .and_then(|c| c.v.clone())
        .unwrap_or_else(|| "esc(0)".to_string());
    let row_cached = value_cell.and_then(|c| c.v.clone());

    let evaluate = |strs: &mut StrEnv| -> Option<(Option<f64>, Option<String>)> {
        let f = formula?;
        match eval_field(f, env, strs, ctx, &NoMetrics) {
            Ok(vsdx::formula::Value::Num(n)) => Some((Some(n), None)),
            Ok(vsdx::formula::Value::Str(s)) => Some((None, Some(s))),
            Err(err) => {
                // Cross-sheet refs (`Sheet.5!User.ValueText`): seed the
                // named shape's User/Prop row value and signal the
                // caller to retry — the issue's page-level walk in
                // place of a dependency graph.
                let cell = err
                    .strip_prefix("unknown cell '")
                    .and_then(|c| c.strip_suffix('\''))?;
                let (prefix, name) = cell.split_once('!')?;
                let sid: i64 = prefix.strip_prefix("Sheet.")?.parse().ok()?;
                let target = find_shape_by_id(page_shapes, sid)?;
                let v = target_row_value(&target.sheet, name)?;
                strs.insert(cell.to_string(), v);
                None
            }
        }
    };

    let mut evaluated = evaluate(strs);
    if evaluated.is_none() {
        // The first pass may have seeded new cross-sheet strings into
        // `strs`; a second evaluation sees them.
        evaluated = evaluate(strs);
    }
    let (num, text) = evaluated.unwrap_or((None, None));
    if num.is_none() && text.is_none() {
        // Unresolved: the marker's cached content, then the row's
        // cached Value, then empty.
        return marker_cached
            .map(str::to_string)
            .or(row_cached)
            .unwrap_or_default();
    }
    let applied = vsdx::fieldfmt::apply_picture(num, text.as_deref(), &picture);
    if applied.is_empty() {
        return marker_cached.unwrap_or_default().to_string();
    }
    applied
}

/// The Field row for `ix` through the inheritance chain: the
/// instance's own Field section first, then the master source's
/// (instances carry `F='Inh'` rows whose formulas live on the master).
fn resolve_field_row<'a>(
    doc: &'a Document,
    s: &'a Shape,
    chain: &[&'a Shape],
    ix: usize,
) -> Option<&'a Row> {
    let own = s.sheet.sections_named("Field").into_iter().find_map(|sec| sec.row_by_ix(ix as i64));
    if own.is_some() {
        return own;
    }
    let src = doc.instance_source(s, chain)?;
    if std::ptr::eq(src, s) {
        return None;
    }
    let mut cur = src;
    for _ in 0..4 {
        if let Some(row) = cur.sheet.sections_named("Field").into_iter().find_map(|sec| sec.row_by_ix(ix as i64)) {
            return Some(row);
        }
        match doc.instance_source(cur, &[]) {
            Some(next) if !std::ptr::eq(next, cur) => cur = next,
            _ => break,
        }
    }
    None
}

/// The string layer: `User.Name` / `Prop.Name` → the row's Value V
/// (strings AND numerics — a numeric string still parses at use),
/// own sheet first, then the master source.
fn str_env(doc: &Document, s: &Shape, chain: &[&Shape]) -> StrEnv {
    let mut strs = StrEnv::new();
    let own = doc.effective_sheet(s, chain);
    let mut sheets: Vec<&vsdx::model::Sheet> = Vec::new();
    if let Some(src) = doc.instance_source(s, chain) {
        if !std::ptr::eq(&src.sheet, own.as_ref()) {
            sheets.push(&src.sheet);
        }
    }
    sheets.push(own.as_ref());
    for sheet in sheets {
        for section in ["User", "Prop"] {
            for row in sheet.sections_named(section).iter().flat_map(|sec| &sec.rows) {
                let Some(name) = row.n.as_deref() else { continue };
                let Some(v) = row.cell("Value").and_then(|c| c.v.clone()) else {
                    continue;
                };
                strs.entry(format!("{section}.{name}")).or_insert(v);
            }
        }
    }
    strs
}

/// One `User.X`/`Prop.X` (or `User.X.Value` cell form) value from a
/// sheet — the cross-sheet retry's resolver.
fn target_row_value(sheet: &vsdx::model::Sheet, name: &str) -> Option<String> {
    let (section, row) = name.split_once('.')?;
    let row = row.strip_suffix(".Value").unwrap_or(row);
    sheet
        .sections_named(section)
        .iter()
        .flat_map(|sec| &sec.rows)
        .find(|r| r.n.as_deref() == Some(row))
        .and_then(|r| r.cell("Value"))
        .and_then(|c| c.v.clone())
}


#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::{Cell, Section, SheetChild, Shape};

    /// A shape whose Field row 0 evaluates `Prop.Name` and whose text
    /// is the corpus's self-closing marker shape.
    fn field_shape() -> Shape {
        let mut s = Shape {
            id: 1,
            ..Default::default()
        };
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.set_cell("Height", Cell::value("1.0"));
        // Prop.Name = "Solution Architect" (a STRING cell — the
        // numeric env cannot carry it).
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Prop".into(),
            ix: 0,
            attrs: vec![("N".into(), "Prop".into())],
            cells: Vec::new(),
            rows: vec![vsdx::Row {
                t: Some("Named".into()),
                ix: 0,
                n: Some("Name".into()),
                attrs: vec![("N".into(), "Name".into())],
                cells: vec![("Value".into(), Cell::value("Solution Architect"))],
            }],
        }));
        // Field row 0: Value F='Prop.Name', Format 'esc(0)'.
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Field".into(),
            ix: 0,
            attrs: vec![("N".into(), "Field".into())],
            cells: Vec::new(),
            rows: vec![vsdx::Row {
                t: None,
                ix: 0,
                n: None,
                attrs: vec![("IX".into(), "0".into())],
                cells: vec![
                    ("Value".into(), Cell { v: Some(String::new()), f: Some("Prop.Name".into()), ..Cell::default() }),
                    ("Format".into(), Cell::value("esc(0)")),
                ],
            }],
        }));
        s.sheet.text = Some(String::new());
        s.sheet.text_raw = Some("<fld IX='0'/>".into());
        s
    }

    #[test]
    fn self_closing_marker_evaluates_the_row_formula() {
        let doc = Document::default();
        let s = field_shape();
        let chain = [&s];
        let ctx = FieldCtx::default();
        let out = substituted_text(&doc, &[], &ctx, &s, &chain, "");
        assert_eq!(out, "Solution Architect", "the field renders its evaluated value");
    }

    #[test]
    fn value_carrying_marker_is_the_fallback() {
        let mut s = field_shape();
        // Break the formula: an unresolvable function sends the
        // marker to its cached content.
        if let Some(SheetChild::Section(sec)) = s
            .sheet
            .children
            .iter_mut()
            .find(|c| matches!(c, SheetChild::Section(sec) if sec.name == "Field"))
        {
            sec.rows[0].cells[0].1.f = Some("NOSUCHFN(Prop.Name)".into());
        }
        s.sheet.text_raw = Some("<fld IX='0'>4/17</fld>".into());
        s.sheet.text = Some("4/17".into());
        let doc = Document::default();
        let chain = [&s];
        let out = substituted_text(&doc, &[], &FieldCtx::default(), &s, &chain, "4/17");
        assert_eq!(out, "4/17", "unresolvable fields fall back to the cached content");
    }

    #[test]
    fn page_context_functions_evaluate() {
        let mut s = field_shape();
        if let Some(SheetChild::Section(sec)) = s
            .sheet
            .children
            .iter_mut()
            .find(|c| matches!(c, SheetChild::Section(sec) if sec.name == "Field"))
        {
            sec.rows[0].cells[0].1.f = Some("PAGENUMBER()&\" / \"&PAGENAME()".into());
        }
        let doc = Document::default();
        let s = s;
        let chain = [&s];
        let ctx = FieldCtx {
            page_name: Some("Process".into()),
            page_number: Some(3.0),
            ..Default::default()
        };
        let out = substituted_text(&doc, &[], &ctx, &s, &chain, "");
        assert_eq!(out, "3 / Process");
    }

    #[test]
    fn marker_free_text_pays_nothing() {
        let mut s = field_shape();
        s.sheet.text_raw = Some("plain &amp; simple".into());
        s.sheet.text = Some("plain & simple".into());
        let doc = Document::default();
        let chain = [&s];
        let out = substituted_text(&doc, &[], &FieldCtx::default(), &s, &chain, "plain & simple");
        assert_eq!(out, "plain & simple");
        // Fast path: no marker, no evaluation.
        assert!(!has_fields(&s));
    }

    #[test]
    fn field_row_inherits_from_the_master_source() {
        // The corpus's F='Inh' shape: the instance's Field row carries
        // no formula; the master source's does.
        let mut master_root = field_shape();
        master_root.id = 10;
        let mut inst = Shape {
            id: 1,
            master: Some(7),
            ..Default::default()
        };
        inst.sheet.text = Some(String::new());
        inst.sheet.text_raw = Some("<fld IX='0'/>".into());
        let mut m = vsdx::Master {
            id: 7,
            name_u: Some("PEINST".into()),
            ..Default::default()
        };
        m.shapes.push(master_root);
        let mut doc = Document::default();
        doc.masters.push(m);
        let chain = [&inst];
        let out = substituted_text(
            &doc,
            &[],
            &FieldCtx::default(),
            &inst,
            &chain,
            "",
        );
        assert_eq!(out, "Solution Architect", "the Field row resolves through the master");
    }
}
