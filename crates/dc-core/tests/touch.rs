//! `TouchList` classification and merge units (#97 §6 item 4): the #93
//! invalidation surface the canvas composites on. A wrong class or a
//! broken merge means stale layer textures — silent visual corruption
//! no golden reliably catches — so the lattice is pinned directly at
//! the unit layer. (The aggregate behavior — a move commit re-renders
//! zero layers — is guarded in dc-ui's render_perf/raster_async gates.)

use dc_core::commands::RowSel;
use dc_core::commands::{AddShape, SetCell, SetRowCell, SetText};
use dc_core::{Editor, Scope, Touch, TouchList, Touched};

fn doc_with_shape() -> Editor {
    let mut shape = vsdx::model::Shape {
        id: 1,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    shape
        .sheet
        .set_cell("PinX", vsdx::model::Cell::value("1.5"));
    shape
        .sheet
        .set_cell("PinY", vsdx::model::Cell::value("2.5"));
    shape.sheet.set_cell("Width", vsdx::model::Cell::value("2"));
    let page = vsdx::model::Page {
        id: 0,
        name: "P1".into(),
        attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
        shapes: vec![shape],
        ..vsdx::model::Page::default()
    };
    Editor::new(vsdx::Document {
        pages: vec![page],
        ..vsdx::Document::default()
    })
}

fn touched_of<C: dc_core::Command>(ed: &Editor, cmd: &C) -> TouchList {
    cmd.touched(&ed.doc)
}

#[test]
fn top_level_pin_writes_are_pure_moves_everything_else_is_ink() {
    let ed = doc_with_shape();
    let shape_scope = Scope::Shape { page: 0, shape: 1 };

    // PinX/PinY on the top-level shape: the texture is
    // translation-invariant — only the quad moves.
    for cell in ["PinX", "PinY"] {
        let t = touched_of(
            &ed,
            &SetCell::new(shape_scope.clone(), cell, vsdx::model::Cell::value("3")),
        );
        assert_eq!(
            t.shapes,
            vec![Touched {
                page: 0,
                shape: 1,
                touch: Touch::Move,
                sub: None
            }],
            "{cell}"
        );
    }

    // Any other cell on the same shape changes ink within its bounds.
    for cell in ["Width", "FillForegnd", "Angle"] {
        let t = touched_of(
            &ed,
            &SetCell::new(shape_scope.clone(), cell, vsdx::model::Cell::value("1")),
        );
        assert_eq!(t.shapes[0].touch, Touch::Ink, "{cell}");
    }

    // Text edits are ink.
    let t = touched_of(&ed, &SetText::new(shape_scope.clone(), "hello"));
    assert_eq!(t.shapes[0].touch, Touch::Ink);

    // A geometry-row cell is ink.
    let t = touched_of(
        &ed,
        &SetRowCell::new(
            shape_scope,
            "Geometry",
            0,
            RowSel::Ix(1),
            "X",
            vsdx::model::Cell::value("2"),
        ),
    );
    assert_eq!(t.shapes[0].touch, Touch::Ink);
}

#[test]
fn page_sheet_and_structural_edits_fall_back_to_whole_page_dirty() {
    let ed = doc_with_shape();
    // Page-sheet geometry dirties the whole page conservatively.
    let t = touched_of(
        &ed,
        &SetCell::new(
            Scope::PageSheet { page: 0 },
            "PageWidth",
            vsdx::model::Cell::value("11"),
        ),
    );
    assert_eq!(t, TouchList::page(0));

    // Membership: a shape entering the page is a precise layer-set
    // change on that shape — not ink, not a page fallback.
    let mut shape = vsdx::model::Shape {
        id: 2,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    shape.sheet.set_cell("PinX", vsdx::model::Cell::value("1"));
    let t = touched_of(
        &ed,
        &AddShape {
            page: 0,
            parent: None,
            shape,
        },
    );
    assert_eq!(
        t,
        TouchList {
            pages: vec![],
            shapes: vec![Touched {
                page: 0,
                shape: 2,
                touch: Touch::Membership,
                sub: None
            }],
        }
    );
}

#[test]
fn nested_cell_edits_carry_the_edited_sub_shape_id() {
    // #219: a cell edit on a sub-shape rolls up to its top-level
    // ancestor but NAMES the edited shape — the canvas layer store's
    // damage-region key. A top-level edit carries no sub (its layer is
    // its own subtree; there is nothing smaller to patch).
    let mut group = vsdx::model::Shape {
        id: 1,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    group
        .sheet
        .set_cell("PinX", vsdx::model::Cell::value("1.5"));
    group
        .sheet
        .set_cell("PinY", vsdx::model::Cell::value("2.5"));
    group.sheet.set_cell("Width", vsdx::model::Cell::value("2"));
    group
        .sheet
        .set_cell("Height", vsdx::model::Cell::value("2"));
    let mut member = vsdx::model::Shape {
        id: 2,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    member
        .sheet
        .set_cell("PinX", vsdx::model::Cell::value("0.5"));
    member
        .sheet
        .set_cell("PinY", vsdx::model::Cell::value("0.5"));
    group.sub_shapes.push(member);
    let page = vsdx::model::Page {
        id: 0,
        name: "P1".into(),
        attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
        shapes: vec![group],
        ..vsdx::model::Page::default()
    };
    let ed = Editor::new(vsdx::Document {
        pages: vec![page],
        ..vsdx::Document::default()
    });

    let t = touched_of(
        &ed,
        &SetCell::new(
            Scope::Shape { page: 0, shape: 2 },
            "FillForegnd",
            vsdx::model::Cell::value("2"),
        ),
    );
    assert_eq!(t.shapes[0].shape, 1, "the touch rolls up to the ancestor");
    assert_eq!(t.shapes[0].sub, Some(2), "…and names the edited sub-shape");

    let t = touched_of(
        &ed,
        &SetText::new(Scope::Shape { page: 0, shape: 2 }, "hello"),
    );
    assert_eq!(t.shapes[0].sub, Some(2));

    let t = touched_of(
        &ed,
        &SetCell::new(
            Scope::Shape { page: 0, shape: 1 },
            "FillForegnd",
            vsdx::model::Cell::value("2"),
        ),
    );
    assert_eq!(t.shapes[0].sub, None, "a top-level edit has no sub");
}

#[test]
fn merge_unions_per_shape_with_the_strongest_touch_winning() {
    let mut base = TouchList {
        pages: vec![0],
        shapes: vec![Touched {
            page: 0,
            shape: 1,
            touch: Touch::Move,
            sub: None,
        }],
    };
    // Move + Ink on the same shape → Ink (an ink re-render subsumes).
    base.merge(TouchList {
        pages: vec![],
        shapes: vec![Touched {
            page: 0,
            shape: 1,
            touch: Touch::Ink,
            sub: None,
        }],
    });
    assert_eq!(base.shapes[0].touch, Touch::Ink);

    // Ink then Move stays Ink — a pure move never DOWNGRADES to skip a
    // needed re-render.
    base.merge(TouchList {
        pages: vec![],
        shapes: vec![Touched {
            page: 0,
            shape: 1,
            touch: Touch::Move,
            sub: None,
        }],
    });
    assert_eq!(base.shapes[0].touch, Touch::Ink);

    // Different shapes accumulate; pages union + dedup.
    base.merge(TouchList {
        pages: vec![0, 1],
        shapes: vec![Touched {
            page: 0,
            shape: 2,
            touch: Touch::Move,
            sub: None,
        }],
    });
    assert_eq!(base.shapes.len(), 2);
    assert_eq!(base.pages, vec![0, 1], "page ids sort + dedup");
    base.merge(TouchList::page(0));
    assert_eq!(base.pages, vec![0, 1], "duplicate page id dropped");
}

#[test]
fn editor_apply_and_undo_route_their_touches() {
    // The wiring contract: apply/undo/redo return the command's
    // TouchList so the canvas can invalidate precisely (#93's seam).
    let mut ed = doc_with_shape();
    let (_, touched) = ed
        .apply(SetCell::new(
            Scope::Shape { page: 0, shape: 1 },
            "PinX",
            vsdx::model::Cell::value("9"),
        ))
        .unwrap();
    assert_eq!(
        touched.shapes,
        vec![Touched {
            page: 0,
            shape: 1,
            touch: Touch::Move,
            sub: None
        }]
    );

    // Undo restores the old pin — itself a pure translation, so the
    // reverse touch is also a Move (undo routes the command's own
    // classification, not a blanket ink).
    let (_, touched) = ed.undo().unwrap().unwrap();
    assert_eq!(touched.shapes[0].touch, Touch::Move);

    let (_, touched) = ed.redo().unwrap().unwrap();
    assert_eq!(touched.shapes[0].touch, Touch::Move);
}

/// #267 live field re-eval: a `Prop.X` row edit damages every
/// field-bearing shape whose Value formula references it, on top of
/// the edited shape's own touch; an unrelated row edit does not.
#[test]
fn prop_row_edits_damage_field_shapes_referencing_the_row() {
    use vsdx::model::{Cell, Row, Section, SheetChild};

    let mut ed = doc_with_shape();
    // A second shape carrying a Field row referencing Prop.Name.
    let mut field_shape = vsdx::model::Shape {
        id: 2,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    field_shape.sheet.children.push(SheetChild::Section(Section {
        name: "Field".into(),
        ix: 0,
        attrs: vec![("N".into(), "Field".into())],
        cells: Vec::new(),
        rows: vec![Row {
            t: None,
            ix: 0,
            n: None,
            attrs: vec![("IX".into(), "0".into())],
            cells: vec![
                (
                    "Value".into(),
                    Cell {
                        v: Some(String::new()),
                        u: Some("STR".into()),
                        f: Some("Prop.Name".into()),
                        ..Cell::default()
                    },
                ),
                ("Format".into(), Cell::value("esc(0)")),
            ],
        }],
    }));
    let page = ed
        .doc
        .pages
        .iter_mut()
        .find(|p| p.id == 0)
        .unwrap();
    page.shapes.push(field_shape);

    // A Prop.Name edit touches BOTH shapes.
    let cmd = SetRowCell::new(Scope::Shape { page: 0, shape: 1 }, "Prop", 0, RowSel::Named("Name".into()), "Value", vsdx::model::Cell::value("New"));
    let touched = touched_of(&ed, &cmd);
    let touched_shapes: Vec<i64> = touched.shapes.iter().map(|t| t.shape).collect();
    assert!(touched_shapes.contains(&1), "the edited shape");
    assert!(touched_shapes.contains(&2), "the field shape referencing Prop.Name");
    assert!(
        touched
            .shapes
            .iter()
            .all(|t| t.touch == Touch::Ink),
        "field damage is ink"
    );

    // An unrelated row edit does not drag the field shape in.
    let cmd = SetRowCell::new(Scope::Shape { page: 0, shape: 1 }, "Prop", 0, RowSel::Named("Other".into()), "Value", vsdx::model::Cell::value("New"));
    let touched = touched_of(&ed, &cmd);
    assert!(
        !touched.shapes.iter().any(|t| t.shape == 2),
        "unrelated row: no field damage"
    );
}

/// #267 Insert Field: the row lands, the marker appends to the raw
/// text, and undo restores both in one step.
#[test]
fn insert_field_writes_row_and_marker_and_undoes_whole() {
    use dc_core::commands::InsertField;
    use vsdx::model::SheetChild;

    let mut ed = doc_with_shape();
    let scope = Scope::Shape { page: 0, shape: 1 };
    ed.apply(InsertField::new(scope.clone(), "MASTERNAME()", "esc(0)", 0.0))
        .unwrap();
    let page = ed.doc.pages.iter().find(|p| p.id == 0).unwrap();
    let s = &page.shapes[0];
    let rows = s
        .sheet
        .sections_named("Field")
        .into_iter()
        .next()
        .expect("the Field section materializes");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0].ix, 0);
    assert_eq!(
        rows.rows[0].cell("Value").and_then(|c| c.f.as_deref()),
        Some("MASTERNAME()")
    );
    assert_eq!(rows.rows[0].cell("Format").and_then(|c| c.v.as_deref()), Some("esc(0)"));
    assert_eq!(s.sheet.text_raw.as_deref(), Some("<fld IX='0'/>"));

    // A second insert continues the IX sequence.
    ed.apply(InsertField::new(scope.clone(), "PAGENUMBER()", "0", 0.0))
        .unwrap();
    let page = ed.doc.pages.iter().find(|p| p.id == 0).unwrap();
    let s = &page.shapes[0];
    let rows = s.sheet.sections_named("Field").into_iter().next().unwrap();
    assert_eq!(rows.rows[1].ix, 1);
    assert_eq!(s.sheet.text_raw.as_deref(), Some("<fld IX='0'/><fld IX='1'/>"));

    // One undo removes the second row AND its marker.
    ed.undo().unwrap();
    let page = ed.doc.pages.iter().find(|p| p.id == 0).unwrap();
    let s = &page.shapes[0];
    let rows = s.sheet.sections_named("Field").into_iter().next().unwrap();
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(s.sheet.text_raw.as_deref(), Some("<fld IX='0'/>"));

    // Undoing the first insert removes the whole section and marker.
    ed.undo().unwrap();
    let page = ed.doc.pages.iter().find(|p| p.id == 0).unwrap();
    let s = &page.shapes[0];
    assert!(
        !s.sheet
            .children
            .iter()
            .any(|c| matches!(c, SheetChild::Section(sec) if sec.name == "Field")),
        "the created Field section disappears on undo"
    );
    // The fixture carried no text at all — undo restores the absence.
    assert_eq!(s.sheet.text_raw.as_deref(), None);
}
