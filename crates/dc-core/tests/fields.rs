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
    // An unknown Format formula caches the general picture.
    let cmd = InsertField::new(
        Scope::Shape { page: 0, shape: 2 },
        "Width",
        FieldInsert::End,
    )
    .with_format("IF(User.LUnitsInd=1,\"@\",\"0u\")");
    ed.apply(cmd).unwrap();
    let s = shape(&ed, 2);
    let row = &s.sheet.section("Field", 0).unwrap().rows[1];
    assert_eq!(row.cell("Format").unwrap().v.as_deref(), Some("esc(0)"));
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
