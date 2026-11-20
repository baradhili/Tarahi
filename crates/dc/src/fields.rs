//! Field-row substitution (GitHub #267): the `<fld IX='n'/>` markers
//! of a shape's `<Text>` resolved against the `Field` section (own
//! sheet first, then the master root — the corpus keeps its Field
//! rows on the master), evaluated through the formula engine's
//! string-cell layer, and formatted through the format-picture
//! engine. The SUBSTITUTED string is what lays out, wraps, and paints
//! (the `TEXTWIDTH`/`TEXTHEIGHT` bridge measures what renders, #147).
//!
//! Fallback chain per marker (a field must never render emptier than
//! its own cache): evaluated `Value` → the row's cached `V` → the
//! marker's cached display string → empty.

use vsdx::fieldfmt::{self, UnitSystem};
use vsdx::formula::{self, FieldCtx};
use vsdx::model::{Document, Page, Shape, Sheet};
use vsdx::text::{self, Piece};

/// Page/document facts for field evaluation, built once per render
/// walk (the page render and the GUI layer path both carry the page;
/// a master contact sheet renders page-less with the master's own
/// shape tree as the `Sheet.N!` sibling set).
pub struct FieldsCtx<'a> {
    /// 1-based page number (`PAGENUMBER()`); `None` page-less.
    pub page_number: Option<f64>,
    /// Page name (`PAGENAME()`); `None` page-less.
    pub page_name: Option<String>,
    /// `docProps/core.xml` modified (DOCLASTSAVE/DOCLASTEDIT).
    pub doc_last_save: Option<String>,
    /// `docProps/core.xml` created (DOCCREATION).
    pub doc_creation: Option<String>,
    /// Core-properties facts (CREATOR/TITLE/SUBJECT/KEYWORDS/
    /// DESCRIPTION).
    pub facts: DocFacts,
    /// Default display units (ISO-A paper → metric).
    pub units: UnitSystem,
    /// The sibling shapes `Sheet.N!` cross-refs resolve against —
    /// the page's tree, or the master's for a contact sheet.
    pub siblings: &'a [Shape],
}

impl<'a> FieldsCtx<'a> {
    /// The context of a page render.
    pub fn for_page(doc: &Document, page: &'a Page) -> FieldsCtx<'a> {
        let page_number = doc
            .pages
            .iter()
            .position(|p| p.id == page.id)
            .map(|i| (i + 1) as f64);
        FieldsCtx {
            page_number,
            page_name: Some(page.name.clone()),
            doc_last_save: doc_last_save(doc),
            doc_creation: doc_element(doc, "dcterms:created"),
            facts: doc_facts(doc),
            units: paper_units(page),
            siblings: &page.shapes,
        }
    }

    /// The page-less context of a master contact sheet.
    pub fn for_master(doc: &Document, siblings: &'a [Shape]) -> FieldsCtx<'a> {
        FieldsCtx {
            page_number: None,
            page_name: None,
            doc_last_save: doc_last_save(doc),
            doc_creation: doc_element(doc, "dcterms:created"),
            facts: doc_facts(doc),
            units: UnitSystem::Us,
            siblings,
        }
    }

    /// The per-shape formula context (the master name varies per
    /// shape; the rest is walk-level).
    fn formula_ctx(&self, master_name: Option<String>) -> FieldCtx {
        FieldCtx {
            page_number: self.page_number,
            page_name: self.page_name.clone(),
            master_name,
            doc_last_save: self.doc_last_save.clone(),
            doc_creation: self.doc_creation.clone(),
            doc_creator: self.facts.creator.clone(),
            doc_title: self.facts.title.clone(),
            doc_subject: self.facts.subject.clone(),
            doc_keywords: self.facts.keywords.clone(),
            doc_description: self.facts.description.clone(),
            units: self.units,
        }
    }
}

/// The core-properties facts (dc:/cp: element values) the Document
/// Info functions read.
#[derive(Debug, Clone, Default)]
pub struct DocFacts {
    pub creator: Option<String>,
    pub title: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub description: Option<String>,
}

fn doc_facts(doc: &Document) -> DocFacts {
    DocFacts {
        creator: doc_element(doc, "dc:creator"),
        title: doc_element(doc, "dc:title"),
        subject: doc_element(doc, "dc:subject"),
        keywords: doc_element(doc, "cp:keywords"),
        description: doc_element(doc, "dc:description"),
    }
}

/// `docProps/core.xml` `dcterms:modified` (the DOCLASTSAVE/EDIT
/// source); ISO timestamps are passed through verbatim.
fn doc_last_save(doc: &Document) -> Option<String> {
    doc_element(doc, "dcterms:modified")
}

/// One `<PREFIX:NAME>value</…>` element of core.xml (the opening
/// tag text, e.g. `<dcterms:modified` or `<dc:creator`).
fn doc_element(doc: &Document, tag: &str) -> Option<String> {
    let pkg = doc.package.as_ref()?;
    let core = pkg.part("docProps/core.xml")?;
    let s = std::str::from_utf8(core).ok()?;
    let open = format!("<{tag}");
    let p = s.find(&open)?;
    // Skip to the end of the opening tag (it carries attributes).
    let gt = s[p..].find('>')? + p;
    if s.as_bytes()[gt - 1] == b'/' {
        return None;
    }
    let rest = &s[gt + 1..];
    let close = format!("</{tag}>");
    let end = rest.find(&close)?;
    let v = decode_xml_entities(rest[..end].trim());
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Decode the five predefined XML entities in element text — the
/// raw source carries `&amp;` and the label must show `&`
/// (Greptile round 1).
fn decode_xml_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// ISO-A paper → metric display units (the `_M`/`_U` stencil twins
/// carry A-series vs ANSI pages; A-sizes in inches). Unrecognized
/// pages stay US — the corpus default.
fn paper_units(page: &Page) -> UnitSystem {
    let inches = |n: &str| {
        page.sheet()
            .and_then(|s| s.cell(n))
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    let (Some(w), Some(h)) = (inches("PageWidth"), inches("PageHeight")) else {
        return UnitSystem::Us;
    };
    const MM: f64 = 1.0 / 25.4;
    // ISO A-series, either orientation.
    const A_SERIES: &[(f64, f64)] = &[
        (841.0, 1189.0),
        (594.0, 841.0),
        (420.0, 594.0),
        (297.0, 420.0),
        (210.0, 297.0),
        (148.0, 210.0),
    ];
    for (aw, ah) in A_SERIES {
        let (long, short) = (w.max(h), w.min(h));
        let tol = 0.02;
        let m_long = ah * MM;
        let m_short = aw * MM;
        if (long - m_long).abs() < tol && (short - m_short).abs() < tol {
            return UnitSystem::Metric;
        }
    }
    UnitSystem::Us
}

/// The SUBSTITUTED plain text of a shape — `None` when the raw text
/// carries no `<fld>` marker (the fast path that keeps the 499/679
/// marker-free packages byte-identical through this engine).
pub fn substituted_text(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    fields: Option<&FieldsCtx<'_>>,
) -> Option<String> {
    let raw = s.sheet.text_raw.as_deref()?;
    if !raw.contains("<fld") {
        return None;
    }
    let pieces = text::text_pieces(raw);
    let (env, strs, fctx) = field_envs(doc, s, chain, fields);
    let fctx_ref = fctx.as_ref();

    let mut out = String::new();
    let mut any = false;
    for piece in &pieces {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Fld { ix, content } => {
                any = true;
                out.push_str(&render_field(
                    doc,
                    s,
                    chain,
                    *ix,
                    content.as_deref(),
                    &env,
                    &strs,
                    fctx_ref,
                ));
            }
            _ => {}
        }
    }
    if !any {
        return None;
    }
    Some(out)
}

/// The evaluation environments for a shape's field formulas: the
/// numeric env (svg's `shape_env`), the string env (own + master
/// User/Prop rows under bare and qualified names, plus the sibling
/// sheets for `Sheet.N!` refs), and the per-shape formula context.
fn field_envs(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    fields: Option<&FieldsCtx<'_>>,
) -> (formula::Env, formula::StrEnv, Option<FieldCtx>) {
    let env = crate::svg::shape_env(doc, s, chain);
    let mut strs = vsdx::formula::StrEnv::new();
    seed_sheet_strings(doc, s, chain, &mut strs);
    if let Some(f) = fields {
        // Cross-sheet refs: every sibling's User/Prop rows under
        // `Sheet.N!` (the corpus's `Sheet.5!User.ValueText` refs) —
        // walking INTO groups: a grouped shape's rows are reachable
        // by its own Sheet.N id (Greptile round 1).
        fn seed_tree(s: &Shape, strs: &mut formula::StrEnv) {
            seed_shape_strings_prefixed(s, strs);
            for sub in &s.sub_shapes {
                seed_tree(sub, strs);
            }
        }
        for sib in f.siblings {
            seed_tree(sib, &mut strs);
        }
    }
    let master_name = s
        .master
        .and_then(|mid| doc.master(mid).and_then(|m| m.name_u.clone()));
    let fctx = fields.map(|f| f.formula_ctx(master_name));
    (env, strs, fctx)
}

/// Preview one field's display string (the Insert Field dialog): the
/// `Value` formula evaluated through the shape's field environment
/// and formatted through a Format formula (`FIELDPICTURE(n)` or a
/// literal picture). Empty when nothing resolves — the dialog shows
/// the formula instead.
pub fn preview_field(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    fields: Option<&FieldsCtx<'_>>,
    value_f: &str,
    format_f: Option<&str>,
) -> String {
    let (env, strs, fctx) = field_envs(doc, s, chain, fields);
    let fctx_ref = fctx.as_ref();
    let Some(v) = formula::eval_field(value_f, &env, &strs, fctx_ref)
        .ok()
        .and_then(|v| value_string(&v))
    else {
        return String::new();
    };
    match format_f {
        Some(f) if !f.is_empty() => {
            // A LITERAL picture ("0.00", "{{MMM yyyy}}") is not a
            // formula — evaluating it turns `0.00` into numeric zero
            // and drops the decimals; treat it exactly like
            // `InsertField` does (Greptile round 1).
            let pic = if f.trim().contains('(') {
                formula::eval_field(f, &env, &strs, fctx_ref)
                    .ok()
                    .and_then(|p| value_string(&p))
                    .unwrap_or_default()
            } else {
                f.trim().to_string()
            };
            let units = fctx_ref.map_or(UnitSystem::Us, |c| c.units);
            // The percent picture implies a percent fraction
            // (Greptile round 1: `0.25` + FIELDPICTURE(3) → `25%`).
            let hint = if pic.trim() == "esc(3)" {
                Some("PER")
            } else {
                None
            };
            fieldfmt::format_value(&v, hint, &pic, units)
        }
        _ => v,
    }
}

/// One marker's display string through the fallback chain.
#[allow(clippy::too_many_arguments)] // the pre-built env bundle rides one flat set
fn render_field(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    ix: i64,
    cached: Option<&str>,
    env: &formula::Env,
    strs: &formula::StrEnv,
    fctx: Option<&FieldCtx>,
) -> String {
    let row = field_row(doc, s, chain, ix);
    let Some(row) = row else {
        return cached.unwrap_or("").to_string();
    };
    let value_cell = row.cell("Value");
    // Evaluate the Value formula; fall back to the cached V.
    let (value, unit) = match value_cell {
        Some(c) => {
            let v =
                c.f.as_deref()
                    .filter(|f| !f.is_empty() && *f != "No Formula" && *f != "Inh")
                    .and_then(|f| formula::eval_field(f, env, strs, fctx).ok())
                    .and_then(|v| value_string(&v))
                    // An empty cached V (the corpus's common `V=''`)
                    // is NO value — the marker's own display
                    // outranks it.
                    .or_else(|| c.v.clone().filter(|v| !v.is_empty()));
            (v, c.u.as_deref())
        }
        None => (None, None),
    };
    let Some(value) = value else {
        return cached.unwrap_or("").to_string();
    };
    // The format picture: the Format cell's F (FIELDPICTURE(n) — or
    // a formula producing a picture) evaluated live, else its V.
    let picture = row.cell("Format").and_then(|c| {
        c.f.as_deref()
            .filter(|f| !f.is_empty() && *f != "No Formula" && *f != "Inh")
            .and_then(|f| formula::eval_field(f, env, strs, fctx).ok())
            .map(|v| value_string(&v).unwrap_or_default())
            .or_else(|| c.v.clone())
    });
    let units = fctx.map_or(UnitSystem::Us, |c| c.units);
    match picture {
        Some(p) if !p.is_empty() => {
            // The percent picture implies a percent fraction even
            // when the Value cell's U says something else — the
            // corpus's esc(3) rows carry U='PER', but a
            // dialog-inserted or hand-authored row may not
            // (Greptile round 1: `0.25` rendered `6 mm.` on A4).
            let unit = if p.trim() == "esc(3)" {
                Some("PER")
            } else {
                unit
            };
            fieldfmt::format_value(&value, unit, &p, units)
        }
        _ => value,
    }
}

/// A [`formula::Value`] as the string a picture formats: numbers keep
/// the evaluator's compact shape (the corpus's cached `V`s are
/// likewise plain).
fn value_string(v: &formula::Value) -> Option<String> {
    match v {
        formula::Value::Str(s) => Some(s.clone()),
        formula::Value::Num(n) if n.is_finite() => Some(format!("{n}")),
        _ => None,
    }
}

/// The Field row `IX` selects, through the instance→master layering.
fn field_row<'a>(
    doc: &'a Document,
    s: &'a Shape,
    chain: &[&'a Shape],
    ix: i64,
) -> Option<&'a vsdx::model::Row> {
    if let Some(sec) = s.sheet.section("Field", 0) {
        if let Some(r) = sec.row_by_ix(ix) {
            return Some(r);
        }
    }
    let src = doc.instance_source(s, chain)?;
    src.sheet
        .section("Field", 0)
        .and_then(|sec| sec.row_by_ix(ix))
}

/// Seed the string/numeric environments from the shape's own sheet
/// and its master chain (the `resolve_cell` layering for rows).
fn seed_sheet_strings(doc: &Document, s: &Shape, chain: &[&Shape], strs: &mut formula::StrEnv) {
    // The master root's rows inherit under the BARE name (the
    // `effective_user_row` layering — the corpus's instanced
    // `User.Label` fields read the master's row), then the
    // instance's own rows override.
    if let Some(src) = doc.instance_source(s, chain) {
        seed_shape_strings(src, strs);
    }
    seed_shape_strings(s, strs);
}

/// A shape's User/Prop rows: bare names plus `Sheet.ID!` qualified.
fn seed_shape_strings(s: &Shape, strs: &mut formula::StrEnv) {
    seed_rows(&s.sheet, "User", "User", strs);
    seed_rows(&s.sheet, "Prop", "Prop", strs);
    let q = format!("Sheet.{}!", s.id);
    seed_rows(&s.sheet, "User", &format!("{q}User"), strs);
    seed_rows(&s.sheet, "Prop", &format!("{q}Prop"), strs);
}

/// Qualified-only variant (a sibling's or master's rows).
fn seed_shape_strings_prefixed(s: &Shape, strs: &mut formula::StrEnv) {
    let q = format!("Sheet.{}!", s.id);
    seed_rows(&s.sheet, "User", &format!("{q}User"), strs);
    seed_rows(&s.sheet, "Prop", &format!("{q}Prop"), strs);
}

/// Seed `Prefix.Name` from a section's named rows' Value cells —
/// string V cache first, numeric into the numeric env.
fn seed_rows(sheet: &Sheet, sec_name: &str, prefix: &str, strs: &mut formula::StrEnv) {
    for sec in sheet.sections_named(sec_name) {
        for row in sec.rows.iter().filter(|r| r.n.is_some()) {
            let Some(cell) = row.cell("Value") else {
                continue;
            };
            let Some(name) = row.n.as_deref() else {
                continue;
            };
            // EMPTY values seed too: a cleared local row must shadow
            // the master's value (an empty string resolves and blanks
            // the field) instead of falling back to the master's
            // non-empty cache (Greptile round 1).
            if let Some(v) = cell.v.as_deref() {
                strs.insert(format!("{prefix}.{name}"), v.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package carrying a normal core.xml (the blank fixture's own
    /// shape: dc:/cp: elements, attributes on the date tags) — one
    /// real zip member, `docProps/core.xml`.
    fn doc_with_core_properties() -> Document {
        let core = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\"",
            " xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
            "<dc:title>Blank Drawing</dc:title>",
            "<dc:creator>Tarahi</dc:creator>",
            "<dc:description>synthetic fixture</dc:description>",
            "<dcterms:created xsi:type=\"dcterms:W3CDTF\">2012-07-26T10:23:29</dcterms:created>",
            "<dcterms:modified xsi:type=\"dcterms:W3CDTF\">2012-07-18T23:57:59</dcterms:modified>",
            "</cp:coreProperties>"
        );
        let buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(buf);
        w.start_file(
            "[Content_Types].xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        std::io::Write::write_all(
            &mut w,
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
</Types>"#,
        )
        .unwrap();
        w.start_file(
            "docProps/core.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        std::io::Write::write_all(&mut w, core.as_bytes()).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        Document {
            package: Some(
                vsdx::opc::Package::from_reader(std::io::Cursor::new(bytes))
                    .expect("core fixture parses"),
            ),
            ..Document::default()
        }
    }

    /// The owner-prefixed tags must round the WHOLE name into the
    /// closing-tag search — the round-1 fix regressed to
    /// `</c:title` and every property read returned None
    /// (CodeRabbit round 2).
    #[test]
    fn core_properties_read_through_the_owner_prefix() {
        let doc = doc_with_core_properties();
        let page = Page::default();
        let f = FieldsCtx::for_page(&doc, &page);
        assert_eq!(f.facts.title.as_deref(), Some("Blank Drawing"));
        assert_eq!(f.facts.creator.as_deref(), Some("Tarahi"));
        assert_eq!(f.facts.description.as_deref(), Some("synthetic fixture"));
        assert_eq!(f.doc_creation.as_deref(), Some("2012-07-26T10:23:29"));
        assert_eq!(f.doc_last_save.as_deref(), Some("2012-07-18T23:57:59"));
    }

    #[test]
    fn missing_or_empty_properties_read_none() {
        // No package at all.
        let doc = Document::default();
        let page = Page::default();
        let f = FieldsCtx::for_page(&doc, &page);
        assert_eq!(f.facts.title, None);
        assert_eq!(f.doc_last_save, None);
    }

    /// Element text arrives entity-DECODED — a title of
    /// `Research &amp; Development` must read `Research & Development`
    /// (Greptile round 1).
    #[test]
    fn core_properties_decode_entities() {
        let core = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\"",
            " xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
            "<dc:title>Research &amp; Development &lt;v2&gt;</dc:title>",
            "</cp:coreProperties>"
        );
        let buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(buf);
        w.start_file(
            "[Content_Types].xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        std::io::Write::write_all(
            &mut w,
            br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
</Types>"#,
        )
        .unwrap();
        w.start_file(
            "docProps/core.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        std::io::Write::write_all(&mut w, core.as_bytes()).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let doc = Document {
            package: Some(
                vsdx::opc::Package::from_reader(std::io::Cursor::new(bytes))
                    .expect("core fixture parses"),
            ),
            ..Document::default()
        };
        let page = Page::default();
        let f = FieldsCtx::for_page(&doc, &page);
        assert_eq!(
            f.facts.title.as_deref(),
            Some("Research & Development <v2>")
        );
    }

    /// The percent picture renders a percent fraction WITHOUT a
    /// declared U (Greptile round 1: `0.25` + esc(3) → `25%`, not
    /// `6 mm.`), and a LITERAL decimal picture keeps its decimals in
    /// the preview instead of evaluating to numeric zero.
    #[test]
    fn preview_percent_and_literal_pictures() {
        let doc = Document::default();
        let mut s = Shape {
            id: 1,
            ..Shape::default()
        };
        s.sheet.set_cell("Width", vsdx::model::Cell::value("2"));
        s.sheet.set_cell("Height", vsdx::model::Cell::value("1"));
        let chain = [&s];
        let page = Page::default();
        let fctx = FieldsCtx::for_page(&doc, &page);
        // Percent through FIELDPICTURE(3).
        assert_eq!(
            preview_field(
                &doc,
                &s,
                &chain,
                Some(&fctx),
                "0.25",
                Some("FIELDPICTURE(3)")
            ),
            "25%"
        );
        // A literal decimal picture stays literal (not numeric 0.00).
        assert_eq!(
            preview_field(&doc, &s, &chain, Some(&fctx), "2.5", Some("0.00")),
            "2.50"
        );
        // A bare digit picture never unit-converts (page number).
        assert_eq!(
            preview_field(&doc, &s, &chain, Some(&fctx), "2", Some("0")),
            "2"
        );
    }
}
