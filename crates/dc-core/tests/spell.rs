//! `ReplaceTextRange` (GitHub #121): the spell check's `Change` splices
//! the RAW text at a plain-char range — every other marker survives
//! byte-identical (unlike `SetText`'s plain store, which would flatten a
//! field-bearing shape), the plain projection recomputes from the
//! pieces, and undo restores the pre-state exactly.

use dc_core::scope::Scope;
use dc_core::{Command as _, ReplaceTextRange};
use vsdx::model::{Cell, Document, IndexChild, Master, Page, Shape};

fn shape_scope() -> Scope {
    Scope::Shape { page: 0, shape: 1 }
}

fn doc_with_text(raw: Option<&str>, text: Option<&str>) -> Document {
    let sheet = vsdx::model::Sheet {
        text_raw: raw.map(|s| s.to_string()),
        text: text.map(|s| s.to_string()),
        ..vsdx::model::Sheet::default()
    };
    let shape = Shape {
        id: 1,
        type_: Some("Shape".into()),
        sheet,
        ..Shape::default()
    };
    let page = Page {
        id: 0,
        name: "P1".into(),
        attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
        children: vec![IndexChild::Sheet {
            tag: "PageSheet".into(),
            sheet: vsdx::model::Sheet::default(),
        }],
        shapes: vec![shape],
        ..Page::default()
    };
    Document {
        pages: vec![page],
        ..Document::default()
    }
}

/// A master whose root shape carries text, and a page instance pointing
/// at it with no local text of its own.
fn doc_with_master_instance() -> Document {
    let master_sheet = vsdx::model::Sheet {
        text_raw: Some("master text".into()),
        ..vsdx::model::Sheet::default()
    };
    let master_shape = Shape {
        id: 1,
        type_: Some("Shape".into()),
        sheet: master_sheet,
        ..Shape::default()
    };
    let master = Master {
        id: 5,
        name_u: Some("M".into()),
        attrs: vec![
            ("ID".into(), "5".into()),
            ("NameU".into(), "M".into()),
            ("Name".into(), "M".into()),
        ],
        shapes: vec![master_shape],
        ..Master::default()
    };
    let mut sheet = vsdx::model::Sheet::default();
    sheet.set_cell("PinX", Cell::value("1"));
    let instance = Shape {
        id: 1,
        type_: Some("Shape".into()),
        master: Some(5),
        sheet,
        ..Shape::default()
    };
    let page = Page {
        id: 0,
        name: "P1".into(),
        attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
        children: vec![IndexChild::Sheet {
            tag: "PageSheet".into(),
            sheet: vsdx::model::Sheet::default(),
        }],
        shapes: vec![instance],
        ..Page::default()
    };
    Document {
        pages: vec![page],
        masters: vec![master],
        ..Document::default()
    }
}

#[test]
fn replace_splices_raw_and_recomputes_plain() {
    let mut doc = doc_with_text(
        Some("a <fld IX='0'>42</fld> b"),
        Some("a 42 b"),
    );
    // "b" is plain char 5 (a=0, ' '=1, 4=2, 2=3, ' '=4, b=5).
    let mut cmd = ReplaceTextRange::new(shape_scope(), 5, 1, "end");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw.as_deref(), Some("a <fld IX='0'>42</fld> end"));
    assert_eq!(sh.sheet.text.as_deref(), Some("a 42 end"));
    // Undo restores both bytes exactly.
    cmd.undo(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw.as_deref(), Some("a <fld IX='0'>42</fld> b"));
    assert_eq!(sh.sheet.text.as_deref(), Some("a 42 b"));
}

#[test]
fn replace_escapes_the_replacement() {
    let mut doc = doc_with_text(Some("keep <fld IX='0'/>word"), Some("keep word"));
    // "word" sits at plain chars 5..9.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 5, 4, "a&b<c>");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("keep <fld IX='0'/>a&amp;b&lt;c&gt;")
    );
    assert_eq!(sh.sheet.text.as_deref(), Some("keep a&b<c>"));
}

#[test]
fn replace_materializes_master_text() {
    let mut doc = doc_with_master_instance();
    // "text" is plain chars 7..11 of the inherited "master text".
    let mut cmd = ReplaceTextRange::new(shape_scope(), 7, 4, "words");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw.as_deref(), Some("master words"));
    assert_eq!(sh.sheet.text.as_deref(), Some("master words"));
    // Undo empties the local text again (the instance inherits).
    cmd.undo(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw, None);
    assert_eq!(sh.sheet.text, None);
}

#[test]
fn replace_inside_cdata_writes_unescaped() {
    // A CDATA-wrapped text (the corpus wraps texts carrying markup-ish
    // characters): the span maps INTO the content and the replacement
    // is literal there — an escaped `&amp;` would read as six characters.
    let mut doc = doc_with_text(Some("<![CDATA[alpha & <b> beta]]>"), Some("alpha & <b> beta"));
    // Plain: alpha=0..5, ' '=5, &=6, ' '=7, <b>=8..11, ' '=11, beta=12..16.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 12, 4, "gamma&more");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("<![CDATA[alpha & <b> gamma&more]]>")
    );
    assert_eq!(sh.sheet.text.as_deref(), Some("alpha & <b> gamma&more"));
}

#[test]
fn replace_never_swallows_a_marker_between_runs() {
    // A word BEFORE and a word AFTER a zero-plain marker: replacing the
    // word after the marker must not eat it (the head resolves into the
    // following segment).
    let mut doc = doc_with_text(Some("go <cp IX='0'/> stop"), Some("go  stop"));
    // Plain: go=0..2, ' '=2, (cp: 0), ' '=3, stop=4..8.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 4, 4, "halt");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw.as_deref(), Some("go <cp IX='0'/> halt"));
    assert_eq!(sh.sheet.text.as_deref(), Some("go  halt"));
    // And replacing the word BEFORE the marker keeps it too.
    let mut doc = doc_with_text(Some("go <cp IX='0'/> stop"), Some("go  stop"));
    let mut cmd = ReplaceTextRange::new(shape_scope(), 0, 2, "run");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(sh.sheet.text_raw.as_deref(), Some("run <cp IX='0'/> stop"));
}
