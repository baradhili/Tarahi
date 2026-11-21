//! Field-row authoring and live re-evaluation damage (GitHub #267):
//! `InsertField` writes the row and the `<fld>` marker in one undo
//! step, and a `User`/`Prop` row edit touches every field-bearing
//! shape whose formula references it.

use dc_core::commands::RowSel;
use dc_core::Command as _;
use dc_core::{Editor, FieldInsert, InsertField, Scope, SetRowCell, Touch};

fn doc_with_shapes() -> Editor {
    let mut a = vsdx::model::Shape {
        id: 1,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    a.sheet.set_cell("Width", vsdx::model::Cell::value("2"));
    a.sheet.set_cell("Height", vsdx::model::Cell::value("1"));
    // A User row a field can read.
    a.sheet
        .children
        .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
            name: "User".into(),
            rows: vec![vsdx::model::Row {
                n: Some("Label".into()),
                cells: vec![("Value".to_string(), vsdx::model::Cell::value("Hello"))],
                ..vsdx::model::Row::default()
            }],
            ..vsdx::model::Section::default()
        }));
    // A plain second shape with no field.
    let mut b = vsdx::model::Shape {
        id: 2,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    b.sheet.set_cell("Width", vsdx::model::Cell::value("2"));
    b.sheet.set_cell("Height", vsdx::model::Cell::value("1"));
    b.sheet.text = Some("plain".into());
    let page = vsdx::model::Page {
        id: 0,
        name: "P1".into(),
        attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
        shapes: vec![a, b],
        ..vsdx::model::Page::default()
    };
    Editor::new(vsdx::Document {
        pages: vec![page],
        ..vsdx::Document::default()
    })
}

fn shape(ed: &Editor, id: i64) -> &vsdx::model::Shape {
    ed.doc.pages[0].shapes.iter().find(|s| s.id == id).unwrap()
}

#[test]
fn insert_field_writes_row_and_marker_in_one_undo_step() {
    let mut ed = doc_with_shapes();
    let scope = Scope::Shape { page: 0, shape: 1 };
    let cmd = InsertField::new(scope.clone(), "User.Label", FieldInsert::End);
    ed.apply(cmd).unwrap();

    // The row: the corpus's verbatim cell shape.
    let s = shape(&ed, 1);
    let sec = s.sheet.section("Field", 0).expect("Field section");
    assert_eq!(sec.rows.len(), 1);
    let row = &sec.rows[0];
    assert_eq!(row.ix, 0);
    let value = row.cell("Value").unwrap();
    assert_eq!(value.f.as_deref(), Some("User.Label"));
    assert_eq!(value.v.as_deref(), Some(""));
    assert_eq!(value.u.as_deref(), Some("STR"));
    let format = row.cell("Format").unwrap();
    assert_eq!(format.f.as_deref(), Some("FIELDPICTURE(0)"));
    assert_eq!(format.v.as_deref(), Some("esc(0)"));
    for cell in ["UICat", "UICod", "UIFmt", "Calendar", "ObjectKind"] {
        let c = row.cell(cell).unwrap();
        assert_eq!(c.v.as_deref(), Some("0"), "{cell}");
        assert_eq!(c.f.as_deref(), Some("No Formula"), "{cell}");
    }
    // The marker appended to the raw text; plain text unchanged (a
    // self-closing marker contributes nothing).
    assert_eq!(s.sheet.text_raw.as_deref(), Some("<fld IX='0'/>"));
    assert_eq!(s.sheet.text.as_deref(), Some(""));

    // Undo restores both.
    ed.undo().unwrap();
    let s = shape(&ed, 1);
    assert!(s.sheet.section("Field", 0).is_none());
    assert_eq!(s.sheet.text_raw, None);
    assert_eq!(s.sheet.text, None);
}

#[test]
fn insert_field_at_offset_and_two_fields() {
    let mut ed = doc_with_shapes();
    let scope = Scope::Shape { page: 0, shape: 1 };
    // Pre-existing text with formatting markers that must survive.
    let s = shape(&ed, 1);
    let raw = "A <cp IX='0'/>B".to_string();
    let _ = s;
    {
        let page = ed.doc.pages[0].shapes[0].sheet.text_raw.insert(raw.clone());
        let _ = page;
        ed.doc.pages[0].shapes[0].sheet.text = Some("A B".into());
    }
    // Insert at the caret between A and the marker (raw offset 2).
    let cmd = InsertField::new(scope, "PAGENAME()", FieldInsert::RawOffset(2));
    ed.apply(cmd).unwrap();
    let s = shape(&ed, 1);
    assert_eq!(
        s.sheet.text_raw.as_deref(),
        Some("A <fld IX='0'/><cp IX='0'/>B")
    );
    // A second field gets IX 1 and appends at the end.
    let cmd2 = InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::End,
    );
    ed.apply(cmd2).unwrap();
    let s = shape(&ed, 1);
    let sec = s.sheet.section("Field", 0).unwrap();
    assert_eq!(sec.rows.len(), 2);
    assert_eq!(sec.rows[1].ix, 1);
    assert_eq!(
        s.sheet.text_raw.as_deref(),
        Some("A <fld IX='0'/><cp IX='0'/>B<fld IX='1'/>")
    );
}

/// ReplacePlain's insertion and replacement boundaries stay OUTSIDE
/// existing `<fld>` elements (CodeRabbit agent review question —
/// pinned as the answer): an offset after a display lands after the
/// `</fld>`, and replacing the display swaps the WHOLE marker
/// element for the new one.
#[test]
fn replace_plain_boundaries_stay_outside_existing_fields() {
    let mut ed = doc_with_shapes();
    let scope = Scope::Shape { page: 0, shape: 1 };
    {
        let s = &mut ed.doc.pages[0].shapes[0];
        s.sheet.text_raw = Some("A<fld IX='0'>X</fld>B".into());
        s.sheet.text = Some("AXB".into());
        // The Field row the existing marker pairs with (a real
        // package never carries one without the other).
        s.sheet
            .children
            .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                name: "Field".into(),
                rows: vec![vsdx::model::Row {
                    ix: 0,
                    cells: vec![(
                        "Value".to_string(),
                        vsdx::model::Cell::value("X"),
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            }));
    }
    // Insert at plain offset 2 (after the display): a SIBLING marker
    // after the `</fld>`, never nested inside it.
    let cmd = InsertField::new(
        scope,
        "PAGENAME()",
        FieldInsert::ReplacePlain { offset: 2, len: 0 },
    );
    ed.apply(cmd).unwrap();
    assert_eq!(
        shape(&ed, 1).sheet.text_raw.as_deref(),
        Some("A<fld IX='0'>X</fld><fld IX='1'/>B"),
        "the new marker follows the existing field"
    );
    // Replace the display X (offset 1, len 1): the whole marker
    // element is swapped for the new one.
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::ReplacePlain { offset: 1, len: 1 },
    );
    ed.apply(cmd).unwrap();
    assert_eq!(
        shape(&ed, 1).sheet.text_raw.as_deref(),
        Some("A<fld IX='2'/><fld IX='1'/>B"),
        "replacing a display swaps the entire marker element"
    );
}

/// A plain-selection splice over PART of a CDATA section keeps the
/// unselected remainder: the section splits at the selection
/// boundaries, the splice removes only the selected part, and the
/// marker lands between sections — never inside a wrapper
/// (CodeRabbit round 9: `a<![CDATA[bc]]>d` + offset 1 len 1 used to
/// swallow `c` with `b`).
#[test]
fn replace_plain_over_partial_cdata_keeps_the_unselected_part() {
    let mut ed = doc_with_shapes();
    {
        let s = &mut ed.doc.pages[0].shapes[0];
        s.sheet.text_raw = Some("a<![CDATA[bc]]>d".into());
        s.sheet.text = Some("abcd".into());
    }
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::ReplacePlain { offset: 1, len: 1 },
    );
    ed.apply(cmd).unwrap();
    assert_eq!(
        shape(&ed, 1).sheet.text_raw.as_deref(),
        Some("a<fld IX='0'/><![CDATA[c]]>d"),
        "only the selected `b` goes; `c` survives in its own section"
    );
    assert_eq!(shape(&ed, 1).sheet.text.as_deref(), Some("acd"));

    // Both ends inside ONE section: the middle part is replaced
    // wholesale, the surrounding parts survive.
    let mut ed = doc_with_shapes();
    {
        let s = &mut ed.doc.pages[0].shapes[0];
        s.sheet.text_raw = Some("a<![CDATA[bcd]]>e".into());
        s.sheet.text = Some("abcde".into());
    }
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::ReplacePlain { offset: 2, len: 1 },
    );
    ed.apply(cmd).unwrap();
    assert_eq!(
        shape(&ed, 1).sheet.text_raw.as_deref(),
        Some("a<![CDATA[b]]><fld IX='0'/><![CDATA[d]]>e"),
        "the unselected `b` and `d` keep their wrappers"
    );
}

/// A display with NESTED markup maps its END past the `</fld>` — a
/// replacement of the display swaps the whole marker instead of
/// splicing an empty range beside it (CodeRabbit round 9:
/// `<fld IX='0'><b>X</b></fld>Y` + offset 0 len 1 used to leave the
/// old field intact under the new marker).
#[test]
fn nested_markup_display_end_maps_past_the_marker() {
    let mut ed = doc_with_shapes();
    {
        let s = &mut ed.doc.pages[0].shapes[0];
        s.sheet.text_raw = Some("<fld IX='0'><b>X</b></fld>Y".into());
        s.sheet.text = Some("XY".into());
        s.sheet
            .children
            .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                name: "Field".into(),
                rows: vec![vsdx::model::Row {
                    ix: 0,
                    cells: vec![(
                        "Value".to_string(),
                        vsdx::model::Cell::value("X"),
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            }));
    }
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::ReplacePlain { offset: 0, len: 1 },
    );
    ed.apply(cmd).unwrap();
    assert_eq!(
        shape(&ed, 1).sheet.text_raw.as_deref(),
        Some("<fld IX='1'/>Y"),
        "the whole nested-markup marker is replaced, not duplicated"
    );
}

#[test]
fn format_override_carries_the_picture() {
    let mut ed = doc_with_shapes();
    let scope = Scope::Shape { page: 0, shape: 2 };
    let cmd = InsertField::new(scope, "Width", FieldInsert::End).with_format("FIELDPICTURE(8)");
    ed.apply(cmd).unwrap();
    let s = shape(&ed, 2);
    let row = &s.sheet.section("Field", 0).unwrap().rows[0];
    assert_eq!(
        row.cell("Format").unwrap().f.as_deref(),
        Some("FIELDPICTURE(8)")
    );
    assert_eq!(row.cell("Format").unwrap().v.as_deref(), Some("esc(8)"));
    // A non-FIELDPICTURE FORMULA keeps F and caches NO picture (the
    // corpus's unit-switch rows evaluate live at render; the cached V
    // is their evaluated result, which dc-core cannot compute). A
    // LITERAL picture caches itself (CodeRabbit round 4).
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 2 },
        "Width",
        FieldInsert::End,
    )
    .with_format("IF(User.LUnitsInd=1,\"@\",\"0u\")");
    ed.apply(cmd).unwrap();
    let literal = InsertField::new(
        Scope::Shape { page: 0, shape: 2 },
        "Width",
        FieldInsert::End,
    )
    .with_format("0.00u");
    ed.apply(literal).unwrap();
    let s = shape(&ed, 2);
    let row = &s.sheet.section("Field", 0).unwrap().rows[1];
    assert_eq!(row.cell("Format").unwrap().v.as_deref(), Some(""));
    assert_eq!(
        row.cell("Format").unwrap().f.as_deref(),
        Some("IF(User.LUnitsInd=1,\"@\",\"0u\")")
    );
    let row = &s.sheet.section("Field", 0).unwrap().rows[2];
    // A literal picture writes NO F (the corpus's bare-V shape) and
    // caches itself as V.
    assert_eq!(row.cell("Format").unwrap().v.as_deref(), Some("0.00u"));
    assert_eq!(row.cell("Format").unwrap().f, None);
}

#[test]
fn user_row_edit_touches_referencing_field_shapes() {
    let mut ed = doc_with_shapes();
    // Shape 1 grows a User.Label field; shape 2 stays plain.
    ed.apply(InsertField::new(
        Scope::Shape { page: 0, shape: 1 },
        "User.Label",
        FieldInsert::End,
    ))
    .unwrap();

    let cmd = SetRowCell::new(
        Scope::Shape { page: 0, shape: 1 },
        "User",
        0,
        RowSel::Named("Label".into()),
        "Value",
        vsdx::model::Cell::value("World"),
    );
    let t = cmd.touched(&ed.doc);
    // The edited shape itself AND the field-bearer (same shape here).
    assert!(
        t.shapes
            .iter()
            .any(|x| x.shape == 1 && x.touch == Touch::Ink),
        "the field-bearing shape re-renders: {t:?}"
    );
    // A NON-referencing edit (a different row) adds nothing.
    ed.apply(cmd).unwrap();
    let other = SetRowCell::new(
        Scope::Shape { page: 0, shape: 1 },
        "User",
        0,
        RowSel::Named("Unrelated".into()),
        "Value",
        vsdx::model::Cell::value("x"),
    );
    ed.apply(other).unwrap();
    let plain = SetRowCell::new(
        Scope::Shape { page: 0, shape: 2 },
        "User",
        0,
        RowSel::Named("Label".into()),
        "Value",
        vsdx::model::Cell::value("y"),
    );
    let t2 = plain.touched(&ed.doc);
    assert!(
        !t2.shapes.iter().any(|x| x.shape == 1),
        "shape 2's own edit only: {t2:?}"
    );
}

#[test]
fn cross_shape_field_damage_rolls_up() {
    // A field on shape 2 reads shape 1's User row: editing shape 1's
    // row must re-render shape 2 (the `Sheet.1!User.Label` corpus
    // ref shape).
    let mut ed = doc_with_shapes();
    ed.apply(InsertField::new(
        Scope::Shape { page: 0, shape: 2 },
        "Sheet.1!User.Label",
        FieldInsert::End,
    ))
    .unwrap();
    let cmd = SetRowCell::new(
        Scope::Shape { page: 0, shape: 1 },
        "User",
        0,
        RowSel::Named("Label".into()),
        "Value",
        vsdx::model::Cell::value("Changed"),
    );
    let t = cmd.touched(&ed.doc);
    assert!(
        t.shapes
            .iter()
            .any(|x| x.shape == 2 && x.touch == Touch::Ink),
        "the cross-referencing field shape re-renders: {t:?}"
    );
    assert!(
        t.shapes.iter().any(|x| x.shape == 1),
        "the edited shape itself too: {t:?}"
    );
}
