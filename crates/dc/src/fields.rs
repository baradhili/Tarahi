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
    /// The page-wide `Sheet.N!`-qualified string layer, seeded once
    /// on first use and shared read-only by every shape's fork —
    /// the per-shape sibling-tree rebuild was quadratic in
    /// field-bearing shapes (CodeRabbit PR review). Marker-free
    /// packages take the `substituted_text` fast path and never
    /// seed it at all.
    qualified: std::cell::OnceCell<std::sync::Arc<formula::StrEnv>>,
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
            qualified: std::cell::OnceCell::new(),
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
            qualified: std::cell::OnceCell::new(),
        }
    }

    /// The shared qualified layer — every sibling's User/Prop rows
    /// under `Sheet.N!`, walking INTO groups (a grouped shape's rows
    /// are reachable by its own Sheet.N id — Greptile round 1). A
    /// shape's INHERITED rows (its master's, the normal state of a
    /// freshly placed instance) seed under the INSTANCE's page ID
    /// first, then the local rows override (Greptile round 6:
    /// `Sheet.2!Prop.Label` must resolve even when shape 2 keeps its
    /// Shape Data on the master).
    fn shared_qualified(&self, doc: &Document) -> &std::sync::Arc<formula::StrEnv> {
        self.qualified.get_or_init(|| {
            let mut strs = formula::StrEnv::new();
            fn seed_tree(
                doc: &Document,
                ancestors: &[&Shape],
                s: &Shape,
                strs: &mut formula::StrEnv,
            ) {
                if let Some(src) = doc.instance_source(s, ancestors) {
                    seed_sheet_rows_under(&src.sheet, s.id, strs);
                }
                seed_shape_strings_prefixed(s, strs);
                let mut below: Vec<&Shape> = ancestors.to_vec();
                below.push(s);
                for sub in &s.sub_shapes {
                    seed_tree(doc, &below, sub, strs);
                }
            }
            for sib in self.siblings {
                seed_tree(doc, &[], sib, &mut strs);
            }
            std::sync::Arc::new(strs)
        })
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

/// Decode XML character references in element text, in ONE pass:
/// the five predefined entities plus decimal (`&#38;`) and
/// hexadecimal (`&#x26;`) references — a document property may carry
/// any of them, and the label must show the decoded character
/// (Greptile round 1; CodeRabbit round 5 for the numeric forms).
/// One-pass means an escaped literal never decodes twice:
/// `&amp;#38;` yields `&#38;`, not `&`.
fn decode_xml_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // A reference ends at the first ';' in a bounded window;
        // anything else (no ';', an over-long name, an unknown
        // entity) is a literal '&'. The window walks CHAR
        // boundaries — a byte slice at the limit could split a
        // multibyte character and panic (CodeRabbit agent review).
        let end = rest
            .char_indices()
            .take_while(|(i, _)| *i < 32)
            .find(|(_, c)| *c == ';')
            .map(|(i, _)| i);
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            match name {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "amp" => Some('&'),
                _ if name.starts_with("#x") || name.starts_with("#X") => u32::from_str_radix(
                    &name[2..],
                    16,
                )
                .ok()
                .and_then(char::from_u32),
                _ if name.starts_with('#') => name[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32),
                _ => None,
            }
        });
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end.map_or(0, |e| e + 1)..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
    // Cross-sheet refs (`Sheet.5!User.ValueText`) resolve through the
    // pass-shared qualified layer with this shape's own bare rows
    // forked on top — the sibling tree seeds once per render, not
    // once per shape (CodeRabbit PR review).
    let strs = match fields {
        Some(f) => {
            let mut layered = formula::StrEnv::fork(f.shared_qualified(doc));
            seed_sheet_strings(doc, s, chain, &mut layered);
            layered
        }
        None => {
            let mut own = formula::StrEnv::new();
            seed_sheet_strings(doc, s, chain, &mut own);
            own
        }
    };
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
    // A SPARSE local row inherits missing cells from the master's row
    // of the same IX, and a local cell marked `F="Inh"` resolves from
    // the master like a missing one — its cached `V` is only the
    // stale snapshot of the inherited formula (the shapesheet's
    // cell-by-cell resolution; CodeRabbit PR review).
    let inherited_row = (|| -> Option<&vsdx::model::Row> {
        let sparse = ["Value", "Format"].into_iter().any(|n| {
            row.cell(n).is_none_or(|c| c.f.as_deref() == Some("Inh"))
        });
        if !sparse {
            return None;
        }
        let src = doc.instance_source(s, chain)?;
        src.sheet
            .section("Field", 0)
            .and_then(|sec| sec.row_by_ix(ix))
    })();
    let cell_of = |name: &str| -> Option<&vsdx::model::Cell> {
        match row.cell(name) {
            // Authored locally — it wins.
            Some(c) if c.f.as_deref() != Some("Inh") => Some(c),
            // `Inh` or missing: the master's live cell outranks the
            // stale local cache, with the cache as fallback when the
            // master's row omits the cell.
            inh => inherited_row.and_then(|r| r.cell(name)).or(inh),
        }
    };
    let value_cell = cell_of("Value");
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
    let picture = cell_of("Format").and_then(|c| {
        c.f.as_deref()
            .filter(|f| !f.is_empty() && *f != "No Formula" && *f != "Inh")
            .and_then(|f| formula::eval_field(f, env, strs, fctx).ok())
            .map(|v| value_string(&v).unwrap_or_default())
            .or_else(|| c.v.clone())
    });
    let units = fctx.map_or(UnitSystem::Us, |c| c.units);
    match picture {
        Some(p) if !p.is_empty() => {
            // The percent picture implies a percent fraction when the
            // Value cell DECLARES no recognized unit — the corpus's
            // esc(3) rows carry U='PER', but a dialog-inserted or
            // hand-authored row may not (Greptile round 1: `0.25`
            // rendered `6 mm.` on A4). A declared length unit stays:
            // `288` + U='FT' must render `24 ft.`, not `28800%`
            // (CodeRabbit/Greptile round 5).
            let unit = if p.trim() == "esc(3)"
                && !unit.is_some_and(|u| fieldfmt::Unit::from_u(u).is_some())
            {
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
    // The master root's rows inherit under the BARE name ONLY (the
    // `effective_user_row` layering — the corpus's instanced
    // `User.Label` fields read the master's row), then the
    // instance's own rows override. Seeding the master's rows under
    // its INTERNAL `Sheet.N!` id here would shadow the page's
    // same-ID shape in the shared layer — a master root with ID 1
    // must never capture a `Sheet.1!` ref that names page shape 1
    // (Greptile round 6).
    if let Some(src) = doc.instance_source(s, chain) {
        seed_rows(&src.sheet, "User", "User", strs);
        seed_rows(&src.sheet, "Prop", "Prop", strs);
    }
    seed_shape_strings(s, strs);
}

/// A shape's User/Prop rows: bare names plus `Sheet.ID!` qualified.
fn seed_shape_strings(s: &Shape, strs: &mut formula::StrEnv) {
    seed_rows(&s.sheet, "User", "User", strs);
    seed_rows(&s.sheet, "Prop", "Prop", strs);
    seed_sheet_rows_under(&s.sheet, s.id, strs);
}

/// A sheet's User/Prop rows under one shape's `Sheet.ID!` prefix.
fn seed_sheet_rows_under(sheet: &Sheet, id: i64, strs: &mut formula::StrEnv) {
    let q = format!("Sheet.{id}!");
    seed_rows(sheet, "User", &format!("{q}User"), strs);
    seed_rows(sheet, "Prop", &format!("{q}Prop"), strs);
}

/// Qualified-only variant (a sibling's or master's rows).
fn seed_shape_strings_prefixed(s: &Shape, strs: &mut formula::StrEnv) {
    seed_sheet_rows_under(&s.sheet, s.id, strs);
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
            let key = format!("{prefix}.{name}");
            // An `Inh`-marked cell never OVERRIDES an already-seeded
            // key — its cached V is the stale snapshot of the
            // inherited formula, and the master pass (which runs
            // first) seeded the live value. With nothing seeded (no
            // master row), the cache stays as the fallback
            // (CodeRabbit agent review, extends the round-5
            // render_field rule to the environment seeding).
            if cell.f.as_deref() == Some("Inh") && strs.get(&key).is_some() {
                continue;
            }
            // EMPTY values seed too: a cleared local row must shadow
            // the master's value (an empty string resolves and blanks
            // the field) instead of falling back to the master's
            // non-empty cache (Greptile round 1).
            if let Some(v) = cell.v.as_deref() {
                strs.insert(key, v.to_string());
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
    /// (Greptile round 1) — including DECIMAL and HEXADECIMAL
    /// character references, without decoding an escaped literal
    /// twice (CodeRabbit round 5).
    #[test]
    fn core_properties_decode_entities() {
        let core = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\"",
            " xmlns:dc=\"http://purl.org/dc/elements/1.1/\">",
            "<dc:title>R&amp;D &#38; D&#x3A; &amp;#38; &lt;v2&gt;</dc:title>",
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
            // `&#38;` → &, `&#x3A;` → :, `&amp;#38;` → `&#38;`
            // (the escaped literal never decodes twice).
            Some("R&D & D: &#38; <v2>")
        );
    }

    /// The entity search window ends on a CHAR boundary — a title
    /// whose byte-32 position falls inside a multibyte character
    /// must not panic the decoder (CodeRabbit agent review).
    #[test]
    fn entity_window_never_splits_a_multibyte_character() {
        // `&amp;` then 10 Han characters (3 bytes each) pushes well
        // past byte 32 with no ';' in the window: a byte-sliced
        // window would cut mid-character and panic.
        let s = format!("&amp;{}", "汉".repeat(10));
        assert_eq!(decode_xml_entities(&s), format!("&{}", "汉".repeat(10)));
        // The same shape WITH a late reference still decodes.
        let s = format!("{}&#38;", "汉".repeat(16));
        assert_eq!(decode_xml_entities(&s), format!("{}&", "汉".repeat(16)));
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

    /// A Field section row `IX=0` with the given cells on a sheet.
    fn field_row_sheet(cells: &[(&str, vsdx::model::Cell)]) -> vsdx::model::Sheet {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "Field".into(),
                rows: vec![vsdx::model::Row {
                    ix: 0,
                    cells: cells
                        .iter()
                        .map(|(n, c)| (n.to_string(), c.clone()))
                        .collect(),
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            },
        ));
        sheet
    }

    /// A declared length unit survives the percent picture: `288` +
    /// `U='FT'` + esc(3) renders `24 ft.`, not `28800%`
    /// (CodeRabbit/Greptile round 5). An ABSENT unit still infers the
    /// percent fraction (Greptile round 1).
    #[test]
    fn field_units_survive_the_percent_picture() {
        let doc = Document::default();
        let page = Page::default();
        let fctx = FieldsCtx::for_page(&doc, &page);
        let mk = |u: Option<&str>, raw: &str| -> Shape {
            let mut s = Shape {
                id: 1,
                sheet: field_row_sheet(&[
                    (
                        "Value",
                        vsdx::model::Cell {
                            v: Some(raw.to_string()),
                            u: u.map(str::to_string),
                            ..vsdx::model::Cell::default()
                        },
                    ),
                    (
                        "Format",
                        vsdx::model::Cell {
                            v: Some("esc(3)".into()),
                            ..vsdx::model::Cell::default()
                        },
                    ),
                ]),
                ..Shape::default()
            };
            s.sheet.text_raw = Some(format!("<fld IX='0'>{raw}</fld>"));
            s
        };
        // Declared feet: 288 internal inches = 24 ft.
        let s = mk(Some("FT"), "288");
        let chain = [&s];
        assert_eq!(
            substituted_text(&doc, &s, &chain, Some(&fctx)).as_deref(),
            Some("24 ft.")
        );
        // Declared percent stays percent.
        let s = mk(Some("PER"), "0.25");
        let chain = [&s];
        assert_eq!(
            substituted_text(&doc, &s, &chain, Some(&fctx)).as_deref(),
            Some("25%")
        );
        // No declaration: the percent picture infers the fraction.
        let s = mk(None, "0.25");
        let chain = [&s];
        assert_eq!(
            substituted_text(&doc, &s, &chain, Some(&fctx)).as_deref(),
            Some("25%")
        );
    }

    /// A Prop row `N='Label'` with a string Value on a sheet.
    fn prop_label_sheet(v: &str) -> vsdx::model::Sheet {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "Prop".into(),
                rows: vec![vsdx::model::Row {
                    n: Some("Label".into()),
                    cells: vec![(
                        "Value".to_string(),
                        vsdx::model::Cell {
                            v: Some(v.to_string()),
                            ..vsdx::model::Cell::default()
                        },
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            },
        ));
        sheet
    }

    /// A shape whose field row reads the given formula.
    fn field_reading(f: &str) -> Shape {
        let mut s = Shape {
            id: 99,
            sheet: field_row_sheet(&[(
                "Value",
                vsdx::model::Cell {
                    f: Some(f.to_string()),
                    ..vsdx::model::Cell::default()
                },
            )]),
            ..Shape::default()
        };
        s.sheet.text_raw = Some("<fld IX='0'>?</fld>".into());
        s
    }

    /// `Sheet.N!` refs stay PAGE-scoped: a master's internal shape
    /// ID never captures a page shape's ref, a freshly placed
    /// instance's INHERITED rows resolve under its page ID, and a
    /// local override still wins (Greptile round 6).
    #[test]
    fn qualified_refs_stay_page_scoped() {
        let mut doc = Document::default();
        // Master 7's root shape carries ID 1 — colliding with page
        // shape 1.
        doc.masters.push(vsdx::model::Master {
            id: 7,
            shapes: vec![Shape {
                id: 1,
                sheet: prop_label_sheet("master-root"),
                ..Shape::default()
            }],
            ..vsdx::model::Master::default()
        });
        let mut page = Page {
            id: 1,
            name: "P".to_string(),
            ..Page::default()
        };
        // Page shape 1: local row.
        page.shapes.push(Shape {
            id: 1,
            sheet: prop_label_sheet("page-one"),
            ..Shape::default()
        });
        // Page shape 2: instance with NO local rows (the freshly
        // placed state — its Shape Data lives on the master).
        page.shapes.push(Shape {
            id: 2,
            master: Some(7),
            ..Shape::default()
        });
        // Page shape 3: instance WITH a local override.
        let mut local3 = vsdx::model::Sheet::default();
        local3.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "Prop".into(),
                rows: vec![vsdx::model::Row {
                    n: Some("Label".into()),
                    cells: vec![(
                        "Value".to_string(),
                        vsdx::model::Cell {
                            v: Some("local-three".to_string()),
                            ..vsdx::model::Cell::default()
                        },
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            },
        ));
        page.shapes.push(Shape {
            id: 3,
            master: Some(7),
            sheet: local3,
            ..Shape::default()
        });
        let fctx = FieldsCtx::for_page(&doc, &page);

        // `Sheet.1!` names PAGE shape 1 — not master 7's root that
        // happens to carry ID 1 (the master-source seeding used to
        // shadow the page's entry).
        let s2 = &page.shapes[1];
        let chain = [s2];
        let reader = field_reading("Sheet.1!Prop.Label");
        assert_eq!(
            substituted_text(&doc, &reader, &chain, Some(&fctx)).as_deref(),
            Some("page-one"),
            "a master's internal ID must not capture a page Sheet.N! ref"
        );

        // `Sheet.2!` resolves the INSTANCE's inherited row (kept on
        // the master), and `Sheet.3!` prefers the local override.
        let s1 = &page.shapes[0];
        let chain = [s1];
        let reader = field_reading("Sheet.2!Prop.Label");
        assert_eq!(
            substituted_text(&doc, &reader, &chain, Some(&fctx)).as_deref(),
            Some("master-root"),
            "an inherited row resolves under the instance's page ID"
        );
        let reader = field_reading("Sheet.3!Prop.Label");
        assert_eq!(
            substituted_text(&doc, &reader, &chain, Some(&fctx)).as_deref(),
            Some("local-three"),
            "a local override outranks the inherited row"
        );
    }

    /// An `Inh`-marked local User row never shadows the master's
    /// live value in the seeded environments — the stale `V` cache
    /// must not become the override (CodeRabbit agent review, the
    /// environment-side twin of `field_inh_resolves_through_the_
    /// master`).
    #[test]
    fn inh_user_row_seeds_the_master_value() {
        let user_row = |v: &str, f: Option<&str>| {
            vsdx::model::Row {
                n: Some("Label".into()),
                cells: vec![(
                    "Value".to_string(),
                    vsdx::model::Cell {
                        v: Some(v.to_string()),
                        f: f.map(str::to_string),
                        ..vsdx::model::Cell::default()
                    },
                )],
                ..vsdx::model::Row::default()
            }
        };
        let mut doc = Document::default();
        doc.masters.push(vsdx::model::Master {
            id: 7,
            shapes: vec![Shape {
                id: 1,
                sheet: {
                    let mut sheet = vsdx::model::Sheet::default();
                    sheet.children.push(vsdx::model::SheetChild::Section(
                        vsdx::model::Section {
                            name: "User".into(),
                            rows: vec![user_row("Current", None)],
                            ..vsdx::model::Section::default()
                        },
                    ));
                    sheet
                },
                ..Shape::default()
            }],
            ..vsdx::model::Master::default()
        });
        let mut sheet = field_row_sheet(&[(
            "Value",
            vsdx::model::Cell {
                f: Some("User.Label".into()),
                ..vsdx::model::Cell::default()
            },
        )]);
        sheet.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "User".into(),
                rows: vec![user_row("Old", Some("Inh"))],
                ..vsdx::model::Section::default()
            },
        ));
        let mut s = Shape {
            id: 2,
            master: Some(7),
            sheet,
            ..Shape::default()
        };
        s.sheet.text_raw = Some("<fld IX='0'>Old</fld>".into());
        let chain = [&s];
        let page = Page::default();
        let fctx = FieldsCtx::for_page(&doc, &page);
        assert_eq!(
            substituted_text(&doc, &s, &chain, Some(&fctx)).as_deref(),
            Some("Current"),
            "the master's live row wins over the stale Inh cache"
        );
    }

    /// A local Value cell marked `F="Inh"` resolves through the
    /// master's live formula — its cached `V` is the stale snapshot
    /// (CodeRabbit round 5).
    #[test]
    fn field_inh_resolves_through_the_master() {
        let mut doc = Document::default();
        doc.masters.push(vsdx::model::Master {
            id: 7,
            shapes: vec![Shape {
                id: 1,
                sheet: field_row_sheet(&[
                    (
                        "Value",
                        vsdx::model::Cell {
                            v: Some("9".into()),
                            f: Some("2+3".into()),
                            ..vsdx::model::Cell::default()
                        },
                    ),
                    (
                        "Format",
                        vsdx::model::Cell {
                            v: Some("esc(0)".into()),
                            ..vsdx::model::Cell::default()
                        },
                    ),
                ]),
                ..Shape::default()
            }],
            ..vsdx::model::Master::default()
        });
        let mut s = Shape {
            id: 1,
            master: Some(7),
            sheet: field_row_sheet(&[(
                "Value",
                vsdx::model::Cell {
                    v: Some("Old".into()),
                    f: Some("Inh".into()),
                    ..vsdx::model::Cell::default()
                },
            )]),
            ..Shape::default()
        };
        s.sheet.text_raw = Some("<fld IX='0'>Old</fld>".into());
        let chain = [&s];
        let page = Page::default();
        let fctx = FieldsCtx::for_page(&doc, &page);
        assert_eq!(
            substituted_text(&doc, &s, &chain, Some(&fctx)).as_deref(),
            Some("5"),
            "the master's live formula outranks the stale Inh cache"
        );
    }
}
