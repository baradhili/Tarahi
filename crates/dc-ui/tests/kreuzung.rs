//! #303's own repro, end to end, against the reporter-supplied
//! fixture `corpus/synthetic/kreuzung.vsdx` (the test skips when it
//! is absent).
//!
//! The page: boxes A(1) B(2) C(3) D(4); A→B is connector 6 (carrying
//! Visio's own stored jump `ArcTo` at the old crossing x=3.5), C→D is
//! connector 5 (three local rows + the master's deleted row 3).
//! Dragging C to Pin (3.0, 0.789) and saving is the issue's exact
//! scenario. Expected, from the reporter's measured Visio 2021 rows:
//! C→D grows to the four-row Z (no diagonal last segment), and the
//! crossing stays at x=3.5 where A→B's stored hop already sits — the
//! reconcile keeps Visio's own rows there (its idempotence) instead
//! of leaving a stale bump.

use dc_core::Command;
use vsdx::model::find_shape_by_id;

const FIXTURE: &str = "../../corpus/synthetic/kreuzung.vsdx";

fn open_fixture() -> Option<dc_core::Editor> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: fixture not available");
        return None;
    }
    match vsdx::parse::parse(std::path::Path::new(FIXTURE)) {
        Ok(doc) => Some(dc_core::Editor::new(doc)),
        Err(e) => panic!("the fixture exists but does not parse: {e}"),
    }
}

fn conn(ed: &dc_core::Editor, id: i64) -> &vsdx::model::Shape {
    find_shape_by_id(&ed.doc.pages[0].shapes, id).unwrap()
}

fn row_val(ed: &dc_core::Editor, id: i64, ix: i64, cell: &str) -> f64 {
    conn(ed, id)
        .sheet
        .sections_named("Geometry")[0]
        .rows
        .iter()
        .find(|r| r.ix == ix)
        .unwrap()
        .cell(cell)
        .and_then(|c| c.numeric())
        .unwrap_or(f64::NAN)
}

fn row_count(ed: &dc_core::Editor, id: i64) -> usize {
    conn(ed, id).sheet.sections_named("Geometry")[0].rows.len()
}

/// The issue's steps: move C, reroute, save — the saved file carries
/// Visio-parity rows (the reporter's measured Z) and A→B's hop stays
/// where the route actually crosses.
#[test]
fn kreuzung_move_grows_the_z_and_keeps_the_hop_current() {
    let Some(mut ed) = open_fixture() else {
        return;
    };
    let page_id = ed.doc.pages[0].id;
    // Sanity: the fixture is the #294/#303 shape — A→B's stored jump
    // and C→D's three rows.
    assert_eq!(row_count(&ed, 5), 3);
    assert!((row_val(&ed, 6, 3, "X") - 1.541_666_7).abs() < 1e-4);
    assert!((row_val(&ed, 6, 3, "A") - (-0.041_666_7)).abs() < 1e-4);

    // The move + reroute, one undo step (the GUI's Macro tail).
    let cmds: Vec<Box<dyn Command>> = vec![
        Box::new(dc_ui::cmd::MoveShape::new(page_id, 3, (3.0, 0.789))),
        Box::new(dc_ui::cmd::RerouteConnectors::new(page_id, vec![3])),
    ];
    ed.apply(dc_core::Macro::new("move C", cmds)).unwrap();

    // C→D: the four-row Z, matching the reporter's measured Visio
    // rows `MoveTo(0,0) LineTo(0,1.1602) LineTo(0.5,1.1602)
    // LineTo(0.5,2.3204)` — no diagonal last segment.
    assert_eq!(row_count(&ed, 5), 4, "the route grew the tail row");
    assert!((row_val(&ed, 5, 2, "X") - 0.0).abs() < 1e-6);
    assert!((row_val(&ed, 5, 2, "Y") - 1.160_187).abs() < 1e-5);
    assert!((row_val(&ed, 5, 3, "X") - 0.5).abs() < 1e-6);
    assert!((row_val(&ed, 5, 3, "Y") - 1.160_187).abs() < 1e-5);
    assert!((row_val(&ed, 5, 4, "X") - 0.5).abs() < 1e-6);
    assert!((row_val(&ed, 5, 4, "Y") - 2.320_375).abs() < 1e-5);
    let geo = &conn(&ed, 5).sheet.sections_named("Geometry")[0];
    assert!(
        geo.rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
        "no jump rows on the vertical C→D"
    );

    // A→B: the new route crosses at x=3.5 — exactly where Visio
    // authored the stored hop — so the reconcile keeps the author's
    // own rows (idempotence), not a stale bump somewhere else.
    assert_eq!(row_count(&ed, 6), 4);
    assert!((row_val(&ed, 6, 3, "X") - 1.541_666_7).abs() < 1e-6);
    assert!((row_val(&ed, 6, 3, "A") - (-0.041_666_7)).abs() < 1e-6);

    // One undo returns the whole move (rows included).
    ed.undo().unwrap();
    assert_eq!(row_count(&ed, 5), 3, "undo restores the three-row pose");

    // Re-apply, then SAVE and re-parse: the file itself carries the
    // parity rows — Visio opening it draws the same Z and hop.
    let cmds: Vec<Box<dyn Command>> = vec![
        Box::new(dc_ui::cmd::MoveShape::new(page_id, 3, (3.0, 0.789))),
        Box::new(dc_ui::cmd::RerouteConnectors::new(page_id, vec![3])),
    ];
    ed.apply(dc_core::Macro::new("move C", cmds)).unwrap();
    // A unique tempdir per run — concurrent runs share /tmp, and a
    // fixed path let one run's save race another's parse; the guard
    // stays alive through the assertions, so cleanup happens on drop
    // even when one fails (review on the #303 test).
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("kreuzung-303-roundtrip.vsdx");
    ed.save_as(&out).unwrap();
    let doc = vsdx::parse::parse(&out).unwrap();
    let ed2 = dc_core::Editor::new(doc);
    assert_eq!(row_count(&ed2, 5), 4, "the saved file carries the four-row Z");
    assert!((row_val(&ed2, 5, 4, "Y") - 2.320_375).abs() < 1e-5);
    assert_eq!(row_count(&ed2, 6), 4);
    assert!((row_val(&ed2, 6, 3, "X") - 1.541_666_7).abs() < 1e-6);
}
