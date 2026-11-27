use super::snap_glue::set_settings_child;
use super::*;
use crate::opc::Package;

/// #294: a Visio page carries NO inline layout cells — `LineStyle="0"`
/// hands resolution to the "No Style" stylesheet in document.xml
/// (the shipped repro: LineToLineX 0.125, LineJumpFactorX 0.66667
/// on stylesheet 0, nothing on the PageSheet). Reading the
/// PageSheet alone fell back to 1.0 and drew 3× jumps.
#[test]
fn page_layout_cells_resolve_through_the_style_chain() {
    let mut doc = Document::default();
    let mut page = Page::default();
    let mut sheet = Sheet::default();
    sheet.attrs.push(("LineStyle".to_string(), "0".to_string()));
    page.children.push(IndexChild::Sheet {
        tag: "PageSheet".to_string(),
        sheet,
    });
    // NON-default values: the fixture must fail if resolution
    // skipped style 0 and fell through to the Visio defaults (the
    // bare-page case below pins those separately, #294 review).
    let mut style = StyleSheet {
        id: 0,
        ..Default::default()
    };
    style.sheet.set_cell("LineToLineX", Cell::value("0.2"));
    style.sheet.set_cell("LineJumpFactorX", Cell::value("1.5"));
    doc.stylesheets.push(style);
    doc.pages.push(page);

    assert!(
        (doc.page_line_to_line_x(&doc.pages[0]) - 0.2).abs() < 1e-9,
        "LineToLineX comes off the style chain"
    );
    assert!(
        (doc.page_line_jump_factor(&doc.pages[0]) - 1.5).abs() < 1e-9,
        "LineJumpFactorX comes off the style chain"
    );
    let radius =
        doc.page_line_to_line_x(&doc.pages[0]) * doc.page_line_jump_factor(&doc.pages[0]) / 2.0;
    assert!((radius - 0.15).abs() < 1e-9);

    // An inline PageSheet cell wins over the chain (the GUI's jump
    // size dialog writes here).
    doc.pages[0]
        .sheet_mut()
        .unwrap()
        .set_cell("LineJumpFactorX", Cell::value("2"));
    assert!((doc.page_line_jump_factor(&doc.pages[0]) - 2.0).abs() < 1e-9);

    // Attribute-only parent links (#294 review): the schema carries
    // the parent in the StyleSheet's LineStyle ATTRIBUTE as well as
    // the cell — a style without the linking cell used to stop the
    // walk at the fallback.
    let mut chain = Document::default();
    let mut page = Page::default();
    let mut sheet = Sheet::default();
    sheet.attrs.push(("LineStyle".to_string(), "7".to_string()));
    page.children.push(IndexChild::Sheet {
        tag: "PageSheet".to_string(),
        sheet,
    });
    let mut mid = StyleSheet {
        id: 7,
        ..Default::default()
    };
    mid.attrs.push(("LineStyle".to_string(), "8".to_string()));
    let mut top = StyleSheet {
        id: 8,
        ..Default::default()
    };
    top.sheet.set_cell("LineJumpFactorX", Cell::value("2"));
    chain.stylesheets.extend([mid, top]);
    chain.pages.push(page);
    assert!(
        (chain.page_line_jump_factor(&chain.pages[0]) - 2.0).abs() < 1e-9,
        "the attribute parent link continues the walk"
    );

    // An EXPLICIT 0 is a zero-size hop, not the default (#294
    // review) — the overlay skips radius-0 pages.
    doc.pages[0]
        .sheet_mut()
        .unwrap()
        .set_cell("LineJumpFactorX", Cell::value("0"));
    assert_eq!(doc.page_line_jump_factor(&doc.pages[0]), 0.0);

    // Nothing anywhere — Visio's defaults, never 1.0.
    let mut bare = Document::default();
    let mut bare_page = Page::default();
    bare_page.children.push(IndexChild::Sheet {
        tag: "PageSheet".to_string(),
        sheet: Sheet::default(),
    });
    bare.pages.push(bare_page);
    assert!((bare.page_line_to_line_x(&bare.pages[0]) - 0.125).abs() < 1e-9);
    assert!((bare.page_line_jump_factor(&bare.pages[0]) - 2.0 / 3.0).abs() < 1e-9);
}

/// Themed cells without an evaluated theme (Visio 2021 witness,
/// stencil masters dropped into a theme-less drawing):
/// - `F='Inh'` Themed inherits up the style chain, which follows each
///   style's FillStyle/LineStyle attribute (Normal → Theme → No Style);
/// - `THEMEVAL()` Themed takes the No Style value outright;
/// - an instance without style attributes uses its master shape's
///   styles, not No Style.
#[test]
fn themed_cells_and_master_styles_resolve_like_visio() {
    let style = |id: i64, parent: Option<&str>, fill: Option<(&str, Option<&str>)>| {
        let mut st = StyleSheet {
            id,
            ..Default::default()
        };
        if let Some(p) = parent {
            for a in ["LineStyle", "FillStyle", "TextStyle"] {
                st.attrs.push((a.to_string(), p.to_string()));
            }
        }
        if let Some((v, f)) = fill {
            st.sheet.set_cell(
                "FillForegnd",
                Cell {
                    v: Some(v.into()),
                    f: f.map(Into::into),
                    ..Cell::default()
                },
            );
        }
        st
    };
    let mut doc = Document::default();
    doc.stylesheets.extend([
        style(0, None, Some(("1", None))),
        style(6, Some("0"), Some(("Themed", Some("THEMEVAL()")))),
        style(3, Some("6"), None),
        style(22, Some("6"), Some(("#dde2cd", Some("THEMEGUARD(…)")))),
        style(14, Some("6"), Some(("#cabb97", None))),
    ]);
    let shape = |fill_style: Option<i64>| Shape {
        id: 1,
        fill_style,
        ..Shape::default()
    };
    // Normal → Theme (THEMEVAL) → No Style: white, never "Themed".
    let normal = shape(Some(3));
    let v = doc.resolve_cell(&normal, &[], "FillForegnd").unwrap().v;
    assert_eq!(v.as_deref(), Some("1"));
    // A master shape whose own cell is THEMEVAL(): No Style, not the
    // style's cached tint.
    let mut master_root = shape(Some(22));
    master_root.sheet.set_cell(
        "FillForegnd",
        Cell {
            v: Some("Themed".into()),
            f: Some("THEMEVAL()".into()),
            ..Cell::default()
        },
    );
    // A master shape relying on its style: the style's value.
    let tower = shape(Some(14));
    for (id, root) in [(12, master_root), (13, tower)] {
        doc.masters.push(Master {
            id,
            shapes: vec![root],
            ..Master::default()
        });
    }
    let mut box_inst = shape(None);
    box_inst.master = Some(12);
    let v = doc.resolve_cell(&box_inst, &[], "FillForegnd").unwrap().v;
    assert_eq!(v.as_deref(), Some("1"), "THEMEVAL() → No Style");
    let mut tower_inst = shape(None);
    tower_inst.master = Some(13);
    let v = doc.resolve_cell(&tower_inst, &[], "FillForegnd").unwrap().v;
    assert_eq!(
        v.as_deref(),
        Some("#cabb97"),
        "a style-less instance cascades into its master's FillStyle"
    );
}

/// Each kind of cell walks its own parent attribute: a style whose
/// LineStyle, FillStyle and TextStyle attributes name three
/// different parents hands each kind the value of its own parent
/// (Greptile on PR #335: one shared parent could not tell them
/// apart).
#[test]
fn style_parents_are_picked_per_kind_of_cell() {
    let mut doc = Document::default();
    for (id, cell, v) in [
        (31, "LineColor", "#110000"),
        (32, "FillForegnd", "#002200"),
        (33, "TextBkgnd", "#000033"),
    ] {
        let mut st = StyleSheet {
            id,
            ..Default::default()
        };
        // Every parent carries all three cells, so a wrong pick
        // returns a wrong color instead of nothing.
        for other in ["LineColor", "FillForegnd", "TextBkgnd"] {
            let val = if other == cell { v } else { "#ffffff" };
            st.sheet.set_cell(other, Cell::value(val));
        }
        doc.stylesheets.push(st);
    }
    let mut child = StyleSheet {
        id: 30,
        ..Default::default()
    };
    for (attr, parent, cell) in [
        ("LineStyle", "31", "LineColor"),
        ("FillStyle", "32", "FillForegnd"),
        ("TextStyle", "33", "TextBkgnd"),
    ] {
        child.attrs.push((attr.to_string(), parent.to_string()));
        // Themed `Inh`: no value here, the walk goes to the parent.
        child.sheet.set_cell(
            cell,
            Cell {
                v: Some("Themed".into()),
                f: Some("Inh".into()),
                ..Cell::default()
            },
        );
    }
    doc.stylesheets.push(child);
    let s = Shape {
        id: 1,
        line_style: Some(30),
        fill_style: Some(30),
        text_style: Some(30),
        ..Shape::default()
    };
    let v = |n: &str| doc.resolve_cell(&s, &[], n).unwrap().v;
    assert_eq!(v("LineColor").as_deref(), Some("#110000"));
    assert_eq!(v("FillForegnd").as_deref(), Some("#002200"));
    assert_eq!(v("TextBkgnd").as_deref(), Some("#000033"));
}

/// A document whose theme part defines `varColor1`.
fn doc_with_theme() -> Document {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        z.start_file("[Content_Types].xml", opts).unwrap();
        z.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#)
            .unwrap();
        z.start_file("visio/theme/theme1.xml", opts).unwrap();
        z.write_all(
            br#"<a:theme><vt:varColor1><a:srgbClr val="4472C4"/></vt:varColor1></a:theme>"#,
        )
        .unwrap();
        z.finish().unwrap();
    }
    buf.set_position(0);
    Document {
        package: Some(Package::from_reader(buf).unwrap()),
        ..Document::default()
    }
}

/// A Themed `THEMEVAL("VariantColorN")` cell that the document's
/// theme evaluates keeps its formula for the renderer — on the
/// shape, on its master and in a style — instead of falling to the
/// No Style value (Greptile on PR #335). Without that variant it
/// still takes the No Style value.
#[test]
fn evaluable_theme_variants_keep_their_formula() {
    let themed = |f: &str| Cell {
        v: Some("Themed".into()),
        f: Some(f.into()),
        ..Cell::default()
    };
    let mut doc = doc_with_theme();
    // The theme XML is read once and cached on the document.
    assert!(doc.theme_variants.get().is_none());
    assert_eq!(doc.theme_variant_colors(), vec!["#4472C4".to_string()]);
    assert_eq!(doc.theme_variants.get().map(Vec::len), Some(1));
    let mut no_style = StyleSheet::default();
    no_style.sheet.set_cell("FillForegnd", Cell::value("1"));
    let mut theme = StyleSheet {
        id: 6,
        ..Default::default()
    };
    theme
        .sheet
        .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor1")"#));
    doc.stylesheets.extend([no_style, theme]);
    let f = |c: Option<Cell>| c.and_then(|c| c.f);
    // On the shape itself.
    let mut s = Shape {
        id: 1,
        ..Shape::default()
    };
    s.sheet
        .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor1")"#));
    assert_eq!(
        f(doc.resolve_cell(&s, &[], "FillForegnd")).as_deref(),
        Some(r#"THEMEVAL("VariantColor1")"#)
    );
    assert!(doc
        .resolve_cell_explicit(&s, &[], "FillForegnd")
        .is_some_and(|c| c.is_themed()));
    // In the shape's style.
    let styled = Shape {
        id: 2,
        fill_style: Some(6),
        ..Shape::default()
    };
    assert_eq!(
        f(doc.resolve_cell(&styled, &[], "FillForegnd")).as_deref(),
        Some(r#"THEMEVAL("VariantColor1")"#)
    );
    // A variant the theme does not define: the No Style value.
    s.sheet
        .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor5")"#));
    let c = doc.resolve_cell(&s, &[], "FillForegnd").unwrap();
    assert_eq!(c.v.as_deref(), Some("1"));
}

/// A `THEMEVAL()` cell that finds nothing in No Style goes to the
/// terminal defaults; it does not resume the master's or the
/// style's value below it (CodeRabbit on PR #335).
#[test]
fn a_missed_themeval_fallback_does_not_resume_inheritance() {
    let mut doc = Document::default();
    // No Style without a FillForegnd cell; style 14 with a tint.
    doc.stylesheets.push(StyleSheet::default());
    let mut tint = StyleSheet {
        id: 14,
        ..Default::default()
    };
    tint.sheet.set_cell("FillForegnd", Cell::value("#cabb97"));
    doc.stylesheets.push(tint);
    let mut s = Shape {
        id: 1,
        fill_style: Some(14),
        ..Shape::default()
    };
    s.sheet.set_cell(
        "FillForegnd",
        Cell {
            v: Some("Themed".into()),
            f: Some("THEMEVAL()".into()),
            ..Cell::default()
        },
    );
    let builtin = builtin_defaults()
        .iter()
        .find(|(n, _)| *n == "FillForegnd")
        .map(|(_, v)| v.to_string());
    let v = doc.resolve_cell(&s, &[], "FillForegnd").and_then(|c| c.v);
    assert_eq!(v, builtin, "not the style's #cabb97");
    assert!(doc.resolve_cell_explicit(&s, &[], "FillForegnd").is_none());
}

/// A minimal document carrying one verbatim `DocumentSettings`
/// element (exactly what the parser produces for the corpus shape
/// — the RawEl round-trip itself is covered by the corpus tests).
fn doc_with_settings(attrs: Attrs, inner: &str) -> Document {
    let mut doc = Document::default();
    doc.items.push(DocItem::Raw(RawEl {
        name: "DocumentSettings".to_string(),
        attrs,
        inner: inner.to_string(),
    }));
    doc.items.push(DocItem::DocumentSheet);
    doc
}

/// The shipped corpus element (attrs + children in Visio order).
const CORPUS_INNER: &str = "<GlueSettings>9</GlueSettings><SnapSettings>295</SnapSettings>\
<SnapExtensions>34</SnapExtensions><SnapAngles/>\
<DynamicGridEnabled>1</DynamicGridEnabled><ProtectStyles>0</ProtectStyles>";

#[test]
fn reads_corpus_defaults() {
    let doc = doc_with_settings(
        vec![
            ("TopPage".to_string(), "0".to_string()),
            ("DefaultTextStyle".to_string(), "3".to_string()),
        ],
        CORPUS_INNER,
    );
    let s = doc.doc_snap_glue().expect("DocumentSettings present");
    assert_eq!(s.snap, 295);
    assert_eq!(s.glue, 9);
    assert!(s.dynamic_grid);
    // No corpus element carries <AutoConnect> — the missing child
    // reads as ON (#209 step 2).
    assert!(s.autoconnect);
}

#[test]
fn missing_element_reads_none() {
    let doc = Document::default();
    assert!(doc.doc_snap_glue().is_none());
}

#[test]
fn setting_patches_only_the_four_children() {
    let mut doc = doc_with_settings(vec![("TopPage".to_string(), "0".to_string())], CORPUS_INNER);
    let before = doc
        .items
        .iter()
        .find_map(|it| match it {
            DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    doc.set_doc_snap_glue(DocSnapGlue {
        snap: 65847,
        glue: 0,
        dynamic_grid: false,
        autoconnect: false,
    });
    let after = doc
        .items
        .iter()
        .find_map(|it| match it {
            DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    // Attributes and every unmodeled child stay verbatim.
    assert_eq!(before.attrs, after.attrs);
    assert!(after.inner.contains("<SnapExtensions>34</SnapExtensions>"));
    assert!(after.inner.contains("<SnapAngles/>"));
    assert!(after.inner.contains("<ProtectStyles>0</ProtectStyles>"));
    // The four modeled children carry the new values, in canonical
    // child order (AutoConnect follows DynamicGridEnabled).
    let g = after.inner.find("<GlueSettings>").unwrap();
    let s = after.inner.find("<SnapSettings>").unwrap();
    let d = after.inner.find("<DynamicGridEnabled>").unwrap();
    let a = after.inner.find("<AutoConnect>").unwrap();
    assert!(g < s && s < d && d < a, "canonical child order: {}", after.inner);
    assert!(after.inner.contains("<GlueSettings>0</GlueSettings>"));
    assert!(after.inner.contains("<SnapSettings>65847</SnapSettings>"));
    assert!(after
        .inner
        .contains("<DynamicGridEnabled>0</DynamicGridEnabled>"));
    assert!(after.inner.contains("<AutoConnect>0</AutoConnect>"));
    // And reading back yields exactly what was written.
    let read = doc.doc_snap_glue().unwrap();
    assert_eq!(read.snap, 65847);
    assert_eq!(read.glue, 0);
    assert!(!read.dynamic_grid);
    assert!(!read.autoconnect);
}

#[test]
fn setting_creates_a_missing_element_in_corpus_shape() {
    let mut doc = Document::default();
    doc.items.push(DocItem::DocumentSheet);
    doc.set_doc_snap_glue(DocSnapGlue::CORPUS_DEFAULT);
    // First child of VisioDocument, like the corpus.
    assert!(matches!(doc.items.first(), Some(DocItem::Raw(r)) if r.name == "DocumentSettings"));
    let inner = match doc.items.first() {
        Some(DocItem::Raw(r)) => r.inner.clone(),
        _ => panic!("DocumentSettings element"),
    };
    assert_eq!(
        inner,
        "<GlueSettings>9</GlueSettings><SnapSettings>295</SnapSettings>\
         <SnapExtensions>34</SnapExtensions><SnapAngles/>\
         <DynamicGridEnabled>1</DynamicGridEnabled>\
         <AutoConnect>1</AutoConnect>"
    );
    assert_eq!(doc.doc_snap_glue(), Some(DocSnapGlue::CORPUS_DEFAULT));
}

#[test]
fn patching_inserts_missing_children_in_canonical_position() {
    // An element with only a later-order child: the missing
    // GlueSettings/SnapSettings must land before SnapExtensions,
    // and DynamicGridEnabled after it.
    let mut inner =
        String::from("<SnapExtensions>34</SnapExtensions><ProtectStyles>0</ProtectStyles>");
    inner = set_settings_child(&inner, "GlueSettings", "1");
    inner = set_settings_child(&inner, "SnapSettings", "2");
    inner = set_settings_child(&inner, "DynamicGridEnabled", "0");
    assert_eq!(
        inner,
        "<GlueSettings>1</GlueSettings><SnapSettings>2</SnapSettings>\
         <SnapExtensions>34</SnapExtensions><DynamicGridEnabled>0</DynamicGridEnabled>\
         <ProtectStyles>0</ProtectStyles>"
    );
}

#[test]
fn patching_expands_a_self_closing_child() {
    let inner = set_settings_child("<SnapSettings/>x", "SnapSettings", "295");
    assert_eq!(inner, "<SnapSettings>295</SnapSettings>x");
}
