//! The #115 phase-3 acceptance: the round-trip verifier over a
//! document edited by Replace All — canonical re-serialization of
//! modeled parts, byte equality of untouched parts, model and render
//! equality (`dc roundtrip --verify`'s contract, now
//! [`dc::roundtrip::verify_roundtrip`]).

use dc_core::find::{find_matches, replace_all_command, FindOptions, FindScope, FindTargets};
use dc_core::{Editor, SetText};

fn vsdx_find_all() -> FindTargets {
    FindTargets::all()
}
use vsdx::model::{Cell, Document, Row, Section, Shape, Sheet, SheetChild};

#[test]
fn replace_all_edited_document_round_trips() {
    // A parsed package (the embedded blank) so untouched parts really
    // byte-compare; two shapes' text edited by a Replace All sweep.
    let mut ed = Editor::new_blank();
    let pid = ed.doc.pages[0].id;
    for (sid, text) in [(1, "alpha beta tail"), (2, "beta only")] {
        // Shape 2 carries a Prop row so the sweep also writes a cell
        // value, not just text.
        let sheet = if sid == 2 {
            Sheet {
                children: vec![SheetChild::Section(Section {
                    name: "Prop".into(),
                    rows: vec![Row {
                        n: Some("Owner".into()),
                        cells: vec![("Value".into(), Cell::value("beta owner"))],
                        ..Row::default()
                    }],
                    ..Section::default()
                })],
                ..Sheet::default()
            }
        } else {
            Sheet::default()
        };
        let scope = dc_core::Scope::Shape {
            page: pid,
            shape: sid,
        };
        // AddShape carries the model shape; the text goes through the
        // command layer like a real edit.
        let shape = Shape {
            id: sid,
            type_: Some("Shape".into()),
            sheet,
            ..Shape::default()
        };
        ed.apply(dc_core::AddShape {
            page: pid,
            parent: None,
            shape,
        })
        .unwrap();
        ed.apply(SetText::new(scope, text)).unwrap();
    }

    // The Replace All sweep: every `beta` becomes `gamma`, one undo
    // step, through the shared matcher.
    let opts = FindOptions {
        targets: vsdx_find_all(),
        ..FindOptions::default()
    };
    let matches = find_matches(&ed.doc, "beta", FindScope::AllPages, &opts);
    assert!(matches.len() >= 3, "two texts + one cell value");
    let (mac, count) = replace_all_command(&ed.doc, &matches, "gamma");
    assert_eq!(count, matches.len());
    ed.history
        .apply(Box::new(mac.expect("sweep")), &mut ed.doc)
        .unwrap();
    let text = |ed: &Editor, sid: i64| {
        vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, sid)
            .unwrap()
            .sheet
            .text
            .clone()
            .unwrap()
    };
    assert_eq!(text(&ed, 1), "alpha gamma tail");
    assert_eq!(text(&ed, 2), "gamma only");

    // Save the edited document, then round-trip THE EDITED FILE
    // through the verifier (the `dc roundtrip --verify` contract).
    let tmp = tempfile::tempdir().unwrap();
    let f1 = tmp.path().join("edited.vsdx");
    ed.save_as(&f1).unwrap();
    let doc2: Document = vsdx::parse::parse(&f1).unwrap();
    let f2 = tmp.path().join("roundtrip.vsdx");
    vsdx::write::save(&doc2, &f2).unwrap();
    dc::roundtrip::verify_roundtrip(&doc2, &f1, &f2).expect("round-trip verifies");
}
