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
fn replace_splits_cdata_terminators_in_the_replacement() {
    // A replacement carrying `]]>` inside a CDATA splice must not
    // terminate the section early — adjacent sections keep the XML
    // valid and the plain text exact.
    let mut doc = doc_with_text(Some("<![CDATA[teh]]> tail"), Some("teh tail"));
    let mut cmd = ReplaceTextRange::new(shape_scope(), 0, 3, "a]]>b");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("<![CDATA[a]]]]><![CDATA[>b]]> tail")
    );
    assert_eq!(sh.sheet.text.as_deref(), Some("a]]>b tail"));
    // And the text round-trips through the piece model.
    let pieces = vsdx::text::text_pieces(sh.sheet.text_raw.as_deref().unwrap());
    let plain: String = pieces.iter().map(|p| p.plain()).collect();
    assert_eq!(plain, "a]]>b tail");
}

#[test]
fn replace_refuses_a_span_crossing_a_cdata_boundary() {
    // A merged word spanning the `]]>` (adjacent CDATA + text merge in
    // text_pieces) errors instead of splicing blind.
    let mut doc = doc_with_text(Some("<![CDATA[shp]]>ae tail"), Some("shpae tail"));
    let mut cmd = ReplaceTextRange::new(shape_scope(), 0, 5, "shape");
    assert!(cmd.apply(&mut doc).is_err());
    // The raw is untouched.
    assert_eq!(
        doc.pages[0].shapes[0].sheet.text_raw.as_deref(),
        Some("<![CDATA[shp]]>ae tail")
    );
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

#[test]
fn replace_refuses_a_span_running_past_the_plain_length() {
    // PR #299 review (CodeRabbit): an end past the target's plain
    // length must ERROR, not splice from the head to the raw end —
    // the mapper's tail falls back to `raw.len()` and would delete
    // everything after it, CDATA terminator included (invalid XML).
    let mut doc = doc_with_text(Some("<![CDATA[abc]]>"), Some("abc"));
    // Plain length 3; [1, 4) runs past it.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 1, 3, "X");
    assert!(cmd.apply(&mut doc).is_err());
    // The raw is untouched.
    assert_eq!(doc.pages[0].shapes[0].sheet.text_raw.as_deref(), Some("<![CDATA[abc]]>"));
    // A zero-length span AT the end stays a legal insertion (the
    // plain-text form; inside a CDATA content end the mapper refuses
    // outright — a head cannot land on a content boundary there).
    let mut doc = doc_with_text(Some("abc"), Some("abc"));
    let mut cmd = ReplaceTextRange::new(shape_scope(), 3, 0, "d");
    cmd.apply(&mut doc).unwrap();
    assert_eq!(doc.pages[0].shapes[0].sheet.text.as_deref(), Some("abcd"));
}

#[test]
fn cdata_terminator_does_not_form_at_the_left_splice_join() {
    // PR #299 review (CodeRabbit): a RETAINED `]]` before the head
    // plus a replacement opening with `>` form `]]>` ACROSS the join —
    // escaping only the replacement misses it. The section rebuilds at
    // content level and splits the terminator canonically.
    let mut doc = doc_with_text(Some("<![CDATA[ab]]cdef]]>"), Some("ab]]cdef"));
    // Plain: ab=0..2, ]]=2..4, c=4, def=5..8. Replace `c` with `>`.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 4, 1, ">");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("<![CDATA[ab]]]]><![CDATA[>def]]>")
    );
    let pieces = vsdx::text::text_pieces(sh.sheet.text_raw.as_deref().unwrap());
    let plain: String = pieces.iter().map(|p| p.plain()).collect();
    assert_eq!(plain, "ab]]>def");
}

#[test]
fn cdata_content_ending_in_brackets_still_closes_exactly() {
    // The section rebuild's other edge: content ENDING with `]` (any
    // count) cannot precede a bare `]]>` close — the close's own `]]`
    // merges with it and the scan eats brackets. The split-before-close
    // form keeps the content exact.
    let mut doc = doc_with_text(Some("<![CDATA[ab]]cdef]]>"), Some("ab]]cdef"));
    // Replace `cdef` (5..8) with `]]`: the content ends `ab]]]]`.
    let mut cmd = ReplaceTextRange::new(shape_scope(), 5, 3, "]]");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    let pieces = vsdx::text::text_pieces(sh.sheet.text_raw.as_deref().unwrap());
    let plain: String = pieces.iter().map(|p| p.plain()).collect();
    assert_eq!(plain, "ab]]c]]");
    // A single trailing bracket splits the same way.
    let mut doc = doc_with_text(Some("<![CDATA[a>b]c]]>"), Some("a>b]c"));
    let mut cmd = ReplaceTextRange::new(shape_scope(), 4, 1, "]");
    cmd.apply(&mut doc).unwrap();
    let sh = &doc.pages[0].shapes[0];
    let pieces = vsdx::text::text_pieces(sh.sheet.text_raw.as_deref().unwrap());
    let plain: String = pieces.iter().map(|p| p.plain()).collect();
    assert_eq!(plain, "a>b]]");
}
