use super::*;

#[test]
fn my_shapes_defaults_under_documents() {
    assert_eq!(
        my_shapes_path(Some(PathBuf::from("/home/bret"))),
        PathBuf::from("/home/bret/Documents/My Shapes")
    );
}

/// #266: the My Shapes menu toggle opens a user stencil through
/// the file path — exactly a picker result (title = file stem,
/// pane identity = the path) — and the toggle on an open entry
/// closes it (the menu checkbox state). An unparseable file
/// surfaces as a status note, never a panic.
#[test]
fn my_shapes_toggle_opens_and_closes_the_file_stencil() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("community-pack.vssx");
    let basic = crate::stencil::builtin::by_slug("basic").expect("basic ships");
    std::fs::write(&path, basic.bytes).unwrap();

    let mut app = App::with_doc_opt(None);
    app.toggle_my_shapes_stencil(path.clone());
    assert_eq!(
        app.debug_open_stencil_titles(),
        vec!["community-pack".to_string()],
        "the menu open lands like a picker result: stem title, file identity"
    );
    app.toggle_my_shapes_stencil(path.clone());
    assert!(
        app.debug_open_stencil_titles().is_empty(),
        "the toggle on an open entry closes it"
    );
    app.toggle_my_shapes_stencil(path);
    assert_eq!(
        app.debug_open_stencil_titles(),
        vec!["community-pack".to_string()],
        "a closed entry re-opens through the same menu path"
    );

    let bad = tmp.path().join("broken.vssx");
    std::fs::write(&bad, b"not a package").unwrap();
    app.toggle_my_shapes_stencil(bad);
    assert!(!app.status.is_empty(), "an unreadable stencil is surfaced");
}

/// #188: a NURBSTo row's E-formula interior control points join
/// the selection's vertex handles — one circle per group, at the
/// fraction position over the bbox extents; the row's own X/Y
/// vertex handles stay.
#[test]
fn nurbs_row_gets_e_point_handles() {
    let mut sh = vsdx::model::Shape {
        id: 1,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    for (n, v) in [
        ("PinX", 2.0),
        ("PinY", 1.0),
        ("Width", 4.0),
        ("Height", 2.0),
        ("LocPinX", 2.0),
        ("LocPinY", 1.0),
    ] {
        sh.sheet.set_cell(n, vsdx::Cell::value(format!("{v}")));
    }
    let e = "NURBS(1, 3, 0, 0, 1,0.5,0,1, 3,1.5,0,1)";
    let row = |t: &str, ix: i64, cells: Vec<(&str, f64)>| vsdx::model::Row {
        t: Some(t.into()),
        ix,
        cells: cells
            .into_iter()
            .map(|(n, v)| (n.to_string(), vsdx::Cell::value(format!("{v}"))))
            .collect(),
        ..vsdx::model::Row::default()
    };
    let mut sec = vsdx::model::Section {
        name: "Geometry".into(),
        ix: 0,
        ..vsdx::model::Section::default()
    };
    sec.rows = vec![
        row("MoveTo", 1, vec![("X", 0.0), ("Y", 0.0)]),
        row(
            "NURBSTo",
            2,
            vec![
                ("X", 4.0),
                ("Y", 2.0),
                ("A", 0.0),
                ("B", 1.0),
                ("C", 0.0),
                ("D", 1.0),
            ],
        ),
    ];
    let nurbs_row = &mut sec.rows[1];
    nurbs_row.cells.push((
        "E".to_string(),
        vsdx::Cell {
            v: Some(e.to_string()),
            f: Some(e.to_string()),
            ..vsdx::Cell::default()
        },
    ));
    sh.sheet
        .children
        .push(vsdx::model::SheetChild::Section(std::mem::take(&mut sec)));

    let mut ed = dc_core::Editor::new_blank();
    let page_id = ed.doc.pages[0].id;
    ed.doc.pages[0].shapes.push(sh);
    let mut app = App::with_doc_opt(None);
    app.ed = Some(ed);
    app.page_idx = 0;
    app.sel = Some((page_id, 1));
    let handles = app.selection_handles();
    let nurbs: Vec<_> = handles
        .iter()
        .filter(|h| {
            matches!(
                h.id,
                HandleId::GeoPoint {
                    cell: canvas::GeoCell::NurbsPt(_),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(nurbs.len(), 2, "one handle per E group: {nurbs:?}");
    // Group 0 (1, 0.5) of the 4×2 bbox at origin (0,0): (4, 1).
    if let HandleId::GeoPoint {
        cell: canvas::GeoCell::NurbsPt(0),
        ..
    } = nurbs[0].id
    {
        assert!((nurbs[0].pos.0 - 4.0).abs() < 1e-9, "{}", nurbs[0].pos.0);
        assert!((nurbs[0].pos.1 - 1.0).abs() < 1e-9, "{}", nurbs[0].pos.1);
    } else {
        unreachable!()
    }
    // The row's own X/Y vertex handles stay (two per point row).
    let vertex = handles
        .iter()
        .filter(|h| {
            matches!(
                h.id,
                HandleId::GeoPoint {
                    cell: canvas::GeoCell::X | canvas::GeoCell::Y,
                    ..
                }
            )
        })
        .count();
    assert!(vertex >= 4, "MoveTo + NURBSTo X/Y handles: {vertex}");
}

// Dialogs must never block the UI thread; these tests drive the
// polling half directly (no real native dialog is ever spawned).

/// #132 (URS-1225): a sampled stroke becomes an editable shape —
/// the geometry rows land, the selection carries #137 vertex
/// handles for them, undo removes the shape.
#[test]
fn freeform_stroke_becomes_editable_geometry() {
    let ed = dc_core::Editor::new_blank();
    let page_id = ed.doc.pages[0].id;
    let mut app = App::with_doc_opt(None);
    app.ed = Some(ed);
    app.page_idx = 0;
    let stroke: Vec<(f64, f64)> = (0..=20)
        .map(|i| {
            let t = i as f64 / 20.0;
            (1.0 + t * 2.0, 2.0 + (t * std::f64::consts::PI).sin())
        })
        .collect();
    app.create_freeform(stroke);
    let page = &app.ed.as_ref().unwrap().doc.pages[0];
    assert_eq!(page.shapes.len(), 1, "the freeform shape exists");
    let s = &page.shapes[0];
    let geo = s.sheet.sections_named("Geometry");
    assert_eq!(geo.len(), 1, "one Geometry section");
    assert_eq!(geo[0].rows[0].t.as_deref(), Some("MoveTo"));
    assert!(geo[0].rows.len() >= 2, "at least one segment row");
    assert_eq!(app.debug_sel(), Some((page_id, s.id)), "selected");
    // Vertex handles for the point rows (#137) — editable.
    let handles = app.selection_handles();
    let vertex = handles
        .iter()
        .filter(|h| {
            matches!(
                h.id,
                HandleId::GeoPoint {
                    cell: canvas::GeoCell::X | canvas::GeoCell::Y,
                    ..
                }
            )
        })
        .count();
    assert!(vertex >= 4, "vertex handles on the freeform rows: {vertex}");
    // Undo removes the shape (one add-shape step).
    app.do_undo();
    let page = &app.ed.as_ref().unwrap().doc.pages[0];
    assert_eq!(page.shapes.len(), 0, "undo removes the shape");
}

/// #203: a spline/NURBS-authored CONNECTOR surfaces its
/// positional controls through the 1D handle branch — vertex
/// handles per SplineStart/SplineKnot/NURBSTo row (via the
/// HitFrame) plus NurbsPt handles for the E formula's interior
/// points.
#[test]
fn spline_connector_rows_get_handles() {
    let mut sh = vsdx::model::Shape {
        id: 1,
        type_: Some("Shape".into()),
        ..vsdx::model::Shape::default()
    };
    for (n, v) in [
        ("ObjType", 2.0),
        ("BeginX", 1.0),
        ("BeginY", 1.0),
        ("EndX", 5.0),
        ("EndY", 3.0),
        ("PinX", 3.0),
        ("PinY", 2.0),
        ("Width", 4.0),
        ("Height", 2.0),
        ("LocPinX", 2.0),
        ("LocPinY", 1.0),
    ] {
        sh.sheet.set_cell(n, vsdx::Cell::value(format!("{v}")));
    }
    // The E group (0.5, 1.0) = fractions of W=4/H=2 → local (2, 2)
    // → page (3, 3) through the identity-ish frame.
    let e = "NURBS(1, 3, 0, 0, 0.5,1,0,1)";
    let row = |ix: i64, t: &str, cells: Vec<(&str, f64)>| vsdx::model::Row {
        t: Some(t.into()),
        ix,
        cells: cells
            .into_iter()
            .map(|(n, v)| (n.to_string(), vsdx::Cell::value(format!("{v}"))))
            .collect(),
        ..vsdx::model::Row::default()
    };
    let mut sec = vsdx::model::Section {
        name: "Geometry".into(),
        ix: 0,
        ..vsdx::model::Section::default()
    };
    sec.rows = vec![
        row(1, "MoveTo", vec![("X", 0.0), ("Y", 0.0)]),
        row(
            2,
            "NURBSTo",
            vec![
                ("X", 4.0),
                ("Y", 2.0),
                ("A", 0.0),
                ("B", 1.0),
                ("C", 0.0),
                ("D", 1.0),
            ],
        ),
    ];
    let nurbs_row = &mut sec.rows[1];
    nurbs_row.cells.push((
        "E".to_string(),
        vsdx::Cell {
            v: Some(e.to_string()),
            f: Some(e.to_string()),
            ..vsdx::Cell::default()
        },
    ));
    sh.sheet
        .children
        .push(vsdx::model::SheetChild::Section(std::mem::take(&mut sec)));

    let ed = dc_core::Editor::new_blank();
    let page_id = ed.doc.pages[0].id;
    let mut app = App::with_doc_opt(None);
    app.ed = Some(ed);
    app.page_idx = 0;
    // The shape must be IN the doc for selection_handles: swap the
    // doc's blank shape out — simplest is to keep the fixture in a
    // second doc and copy the shape in.
    {
        let page = &mut app.ed.as_mut().unwrap().doc.pages[0];
        page.shapes.retain(|s| s.id != 1);
        page.shapes.push(sh);
    }
    app.sel = Some((page_id, 1));
    let handles = app.selection_handles();
    // The NURBSTo row's X/Y vertex handle + one NurbsPt per E
    // group.
    let nurbs_vertex = handles.iter().any(|h| {
        matches!(
            h.id,
            HandleId::GeoPoint {
                section_ix: 0,
                row_ix: 2,
                cell: canvas::GeoCell::X,
            }
        )
    });
    assert!(nurbs_vertex, "the NURBSTo endpoint handle exists");
    let pts: Vec<_> = handles
        .iter()
        .filter(|h| {
            matches!(
                h.id,
                HandleId::GeoPoint {
                    cell: canvas::GeoCell::NurbsPt(_),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(pts.len(), 1, "one NurbsPt per E group: {pts:?}");
    // The interior point (2, 2) local → page through the frame
    // (Pin/LocPin identity here: origin (1,1), no rotation).
    assert!((pts[0].pos.0 - 3.0).abs() < 1e-6, "{}", pts[0].pos.0);
    assert!((pts[0].pos.1 - 3.0).abs() < 1e-6, "{}", pts[0].pos.1);
}

/// #187 slice 2: a ROTATED connector joins the crossing set the
/// canvas draws from — `page_jumps`, the same resolver as the
/// export and the reconciliation (#336); the Angle gate is gone;
/// the HitFrame polyline carries the rotated geometry, so it
/// crosses the axis-aligned line and hops.
#[test]
fn rotated_connector_joins_the_jump_lines() {
    let connector = |id: i64, pin: (f64, f64), w: f64, angle: f64| {
        let mut s = vsdx::model::Shape {
            id,
            type_: Some("Shape".into()),
            ..vsdx::model::Shape::default()
        };
        for (n, v) in [
            ("PinX", pin.0),
            ("PinY", pin.1),
            ("Width", w),
            ("Height", 0.0),
            ("LocPinX", w / 2.0),
            ("LocPinY", 0.0),
            ("Angle", angle),
            ("ObjType", 2.0),
        ] {
            s.sheet.set_cell(n, vsdx::Cell::value(format!("{v}")));
        }
        let mut sec = vsdx::model::Section {
            name: "Geometry".into(),
            ix: 0,
            ..vsdx::model::Section::default()
        };
        sec.rows = vec![
            vsdx::model::Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), vsdx::Cell::value("0")),
                    ("Y".to_string(), vsdx::Cell::value("0")),
                ],
                ..vsdx::model::Row::default()
            },
            vsdx::model::Row {
                t: Some("LineTo".into()),
                ix: 2,
                cells: vec![
                    ("X".to_string(), vsdx::Cell::value(format!("{w}"))),
                    ("Y".to_string(), vsdx::Cell::value("0")),
                ],
                ..vsdx::model::Row::default()
            },
        ];
        s.sheet.children.push(vsdx::model::SheetChild::Section(sec));
        s
    };
    let mut ed = dc_core::Editor::new_blank();
    ed.doc.pages[0]
        .shapes
        .push(connector(1, (3.0, 3.0), 4.0, 0.0));
    // The 45° rotated line through pin (3, 2.5) crosses the
    // horizontal at (3.5, 3), strictly inside both.
    ed.doc.pages[0].shapes.push(connector(
        2,
        (3.0, 2.5),
        2.8284271,
        std::f64::consts::FRAC_PI_4,
    ));
    let mut app = App::with_doc_opt(None);
    app.ed = Some(ed);
    app.page_idx = 0;
    let doc = &app.ed.as_ref().unwrap().doc;
    let jumps = dc::geom::page_jumps(doc, &doc.pages[0], &[]);
    assert_eq!(
        jumps.len(),
        1,
        "the rotated crossing hops once: {:?}",
        jumps
    );
    // And the crossing hops: the axis line (flatter) jumps.
    assert!(jumps[0].dir.1.abs() < 1e-6, "the axis line hops");
}

#[test]
fn dialog_pending_keeps_polling() {
    let ctx = Context::default();
    let mut app = App::with_doc_opt(None);
    let (_tx, rx) = mpsc::channel::<Option<PathBuf>>();
    app.dialog = Some(PendingDialog::Open(rx));
    app.poll_dialog(&ctx);
    assert!(app.dialog.is_some(), "unanswered dialog stays pending");
}

#[test]
fn dialog_answered_open_consumes_result() {
    let ctx = Context::default();
    let mut app = App::with_doc_opt(None);
    let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
    tx.send(Some(PathBuf::from("/nonexistent/x.vsdx"))).unwrap();
    app.dialog = Some(PendingDialog::Open(rx));
    app.poll_dialog(&ctx);
    assert!(app.dialog.is_none(), "answered dialog is consumed");
    // #224: the parse went to the worker — the opening status
    // shows, the poll lands the failure, the status flips.
    assert!(app.status.starts_with("Opening"), "status = {}", app.status);
    assert!(app.debug_loading_for_test());
    // The worker thread races this thread — pump the poll until the
    // result arrives (bounded; the failure path is fast).
    for _ in 0..2000 {
        if app.poll_loading().is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        app.status.starts_with("open failed"),
        "status = {}",
        app.status
    );
    assert!(!app.debug_loading_for_test());
}

#[test]
fn dialog_cancelled_is_silent() {
    let ctx = Context::default();
    let mut app = App::with_doc_opt(None);
    let (tx, rx) = mpsc::channel::<Option<PathBuf>>();
    tx.send(None).unwrap();
    app.dialog = Some(PendingDialog::SaveAs(rx));
    app.poll_dialog(&ctx);
    assert!(app.dialog.is_none());
    assert!(!app.dirty);
}

#[test]
fn new_doc_from_nothing_creates_blank() {
    let mut app = App::with_doc_opt(None);
    assert!(app.ed.is_none());
    app.do_new();
    let ed = app.ed.as_ref().expect("blank doc opened");
    assert_eq!(ed.doc.pages.len(), 1);
    assert_eq!(ed.doc.pages[0].name, "Page-1");
    assert!(!app.dirty);
    assert!(app.path.as_os_str().is_empty());
}

#[test]
fn new_doc_with_dirty_state_prompts_instead_of_discarding() {
    let mut app = App::with_doc_opt(None);
    app.do_new();
    assert!(app.ed.is_some());
    app.dirty = true;
    app.do_new();
    assert!(app.confirm_new, "dirty doc must prompt, not discard");
    assert!(app.ed.is_some(), "document stays open while prompting");
    // Discard proceeds; Cancel keeps it.
    app.start_new();
    assert!(!app.confirm_new);
    assert!(!app.dirty);
}

// Recent-files (MRU) wiring: successful loads/saves record, failed
// ones must not (GitHub #16).

#[test]
fn opening_a_file_records_it_in_recent() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("recent-src.vsdx");
    Editor::new_blank().save_as(&p).unwrap();
    let mut app = App::with_doc_opt(None);
    app.debug_load_sync_for_test(&p);
    assert_eq!(
        app.debug_recent(),
        std::slice::from_ref(&p),
        "successful open records the path"
    );
    // Re-opening moves it to the (single) front.
    app.debug_load_sync_for_test(&p);
    assert_eq!(app.debug_recent(), &[p]);
}

#[test]
fn failed_open_does_not_record() {
    let mut app = App::with_doc_opt(None);
    app.debug_load_sync_for_test(&PathBuf::from("/nonexistent/missing.vsdx"));
    assert!(
        app.debug_recent().is_empty(),
        "a failed open must not record anything"
    );
}

#[test]
fn corrupt_package_open_surfaces_an_error_and_pollutes_nothing() {
    // #97 item 8: a truncated/garbage .vsdx must degrade to a
    // status error — never a panic — and leave the session state
    // (recent files, the open document) untouched.
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("corrupt.vsdx");
    std::fs::write(&bad, b"this is not a zip archive at all").unwrap();
    let mut app = App::with_blank();
    app.debug_load_sync_for_test(&bad);
    assert!(
        app.status.starts_with("open failed"),
        "status = {}",
        app.status
    );
    assert!(
        !app.debug_recent()
            .iter()
            .any(|p| p.ends_with("corrupt.vsdx")),
        "a corrupt open must not be recorded in recent files"
    );
    assert!(app.ed.is_some(), "the previously open document survives");
}

#[test]
fn saving_records_the_saved_path() {
    let tmp = tempfile::tempdir().unwrap();
    let mut app = App::with_blank();
    let p = tmp.path().join("saved.vsdx");
    app.save_to(&p);
    assert_eq!(
        app.debug_recent(),
        &[p],
        "successful save-as records the path"
    );
    assert!(!app.dirty, "save clears the dirty flag");
}

// EventDblClick classification (GitHub #20): OPENTEXTWIN and
// DEFAULTEVENT open the text editor, no formula / No Formula keeps
// Visio's default (edit text), everything else defers.

#[test]
fn dbl_click_classifier_opens_text_for_default_formulas() {
    for f in [
        None,
        Some(""),
        Some("   "),
        Some("No Formula"),
        Some("  no formula "),
        Some("OPENTEXTWIN()"),
        Some("opentextwin()"),
        Some("DEFAULTEVENT()"),
    ] {
        assert_eq!(
            classify_dbl_click(f),
            DblClick::OpenText,
            "formula {f:?} must open the text editor"
        );
    }
}

#[test]
fn dbl_click_classifier_defers_other_formulas() {
    for f in [
        Some("RUNADDON(\"MyMacro\")"),
        Some("RUNMACRO(\"ThisDocument.m\")"),
        Some("GOTOPAGE(\"Page-2\")"),
        Some("DONOTHING()"),
        Some("OPENSHEETWINDOW()"),
    ] {
        match classify_dbl_click(f) {
            DblClick::Deferred(msg) => assert!(
                msg.contains(f.unwrap()),
                "deferral message names the formula: {msg}"
            ),
            other => panic!("formula {f:?} must defer, got {other:?}"),
        }
    }
}

/// A shape with `EventDblClick = RUNADDON(…)`: double-click reports
/// the deferral and opens no editor; with no formula the editor opens.
#[test]
fn dbl_click_honors_the_event_cell() {
    fn app_with_event_formula(f: Option<&str>) -> (App, i64) {
        let mut app = App::with_blank();
        let page_id = app.page_id().unwrap();
        let mut sh = vsdx::model::Shape {
            id: 42,
            ..Default::default()
        };
        for (n, v) in [
            ("PinX", "4.0"),
            ("PinY", "5.5"),
            ("Width", "2.0"),
            ("Height", "1.0"),
        ] {
            sh.sheet.set_cell(n, vsdx::Cell::value(v));
        }
        if let Some(f) = f {
            sh.sheet.set_cell(
                "EventDblClick",
                vsdx::Cell {
                    f: Some(f.into()),
                    ..Default::default()
                },
            );
        }
        app.apply(dc_core::commands::AddShape {
            page: page_id,
            parent: None,
            shape: sh,
        });
        (app, 42)
    }

    // No EventDblClick cell: Visio's default — open the editor.
    let (mut app, sid) = app_with_event_formula(None);
    app.on_canvas_dbl_click((4.0, 5.5));
    let sess = app.text_edit.as_ref().expect("default opens the editor");
    assert_eq!((sess.page, sess.shape), (app.page_id().unwrap(), sid));
    assert_eq!(sess.draft, "", "draft seeds from the shape's text");
    app.end_text_edit(true);
    assert!(app.text_edit.is_none(), "a no-change exit applies nothing");

    // RUNADDON: deferral status, no editor.
    let (mut app, _) = app_with_event_formula(Some("RUNADDON(\"x\")"));
    app.on_canvas_dbl_click((4.0, 5.5));
    assert!(app.text_edit.is_none(), "RUNADDON must not open the editor");
    assert!(
        app.status.contains("EventDblClick") && app.status.contains("RUNADDON"),
        "status explains the deferral: {}",
        app.status
    );

    // Double-click on empty page: nothing happens.
    let (mut app, _) = app_with_event_formula(None);
    app.on_canvas_dbl_click((0.1, 0.1));
    assert!(
        app.text_edit.is_none(),
        "empty page double-click is a no-op"
    );
}

/// A stencil placement consumes its click pair: the double-click
/// completing it must not open an editor over the placed shape,
/// while a later, separate double-click works normally.
#[test]
fn dbl_click_after_stencil_placement_defers_to_the_placement() {
    let mut app = App::with_blank();
    let master = app
        .ed
        .as_ref()
        .unwrap()
        .doc
        .masters
        .first()
        .map(|m| m.id)
        .unwrap();
    app.stencil.armed = Some((StencilId::Document, master));
    app.on_canvas_click((4.0, 5.0), false, false);
    assert_eq!(app.debug_shape_count(), 1, "armed master placed on click");
    assert!(app.placed_last_click);

    app.on_canvas_dbl_click((4.0, 5.0));
    assert!(
        app.text_edit.is_none(),
        "the placement's click pair must not open an editor"
    );
    assert_eq!(app.debug_shape_count(), 1, "exactly one placement");

    app.on_canvas_dbl_click((4.0, 5.0));
    assert!(
        app.text_edit.is_some(),
        "a later double-click on the placed shape opens the editor"
    );
}

/// #389: the frame's jump overlay is cached — an idle frame reuses the
/// last pass while the document version, the page, the hidden layers
/// and the radii are unchanged, and any edit (a version bump) recomputes.
/// The per-frame recomputation ran the backdrop walk for every hop on
/// every frame (tens of seconds per frame on large pages).
#[test]
fn jump_overlay_is_cached_until_the_document_changes() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dc/tests/fixtures/b01-linejumps.vsdx");
    let mut app = App::with_doc(&path);
    let r = {
        let doc = &app.ed.as_ref().unwrap().doc;
        crate::connectors::jump_radii_in(doc, &doc.pages[app.page_idx])
    };
    let fresh = app.jump_fx_cached(r, &[]);
    assert!(!fresh.is_empty(), "the fixture page has hops");
    // Poison the stored pass: an unchanged key must serve it as is.
    let key = app.jump_fx_cache.as_ref().unwrap().0.clone();
    app.jump_fx_cache = Some((key, Vec::new()));
    assert!(
        app.jump_fx_cached(r, &[]).is_empty(),
        "an idle frame reuses the pass"
    );
    // Any key change recomputes: other hidden layers, other radii, an
    // edit.
    assert_eq!(app.jump_fx_cached(r, &[999]).len(), fresh.len());
    app.jump_fx_cache = Some((app.jump_fx_cache.as_ref().unwrap().0.clone(), Vec::new()));
    app.simulate_edit_for_test();
    assert_eq!(
        app.jump_fx_cached(r, &[999]).len(),
        fresh.len(),
        "an edit recomputes"
    );
    // A test hook that writes geometry outside the command layer bumps
    // the version too (CodeRabbit on PR #391).
    let (pid, sid) = {
        let doc = &app.ed.as_ref().unwrap().doc;
        let page = &doc.pages[app.page_idx];
        (page.id, page.shapes[0].id)
    };
    app.jump_fx_cache = Some((app.jump_fx_cache.as_ref().unwrap().0.clone(), Vec::new()));
    app.debug_set_geo_row_for_test(pid, sid, 0, 1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]);
    assert!(
        !app.jump_fx_cached(r, &[999]).is_empty(),
        "a hook write recomputes"
    );
    // The index-backed pass matches the per-hop lookup it replaced.
    let doc = &app.ed.as_ref().unwrap().doc;
    let page = &doc.pages[app.page_idx];
    let jumps = dc::geom::page_jumps(doc, page, &[]);
    assert_eq!(fresh.len(), jumps.len(), "one effect per hop");
    for (fx, j) in fresh.iter().zip(&jumps) {
        let (gr, gg, gb) = dc::svg::backdrop_color_at(doc, page, j.at, j.jumper, &[]);
        assert_eq!(fx.gap_color, egui::Color32::from_rgb(gr, gg, gb));
    }
}

/// PR #415 review: the background-underlay chain shares the JOINT
/// texture budget with the foreground layers — the chain degrades
/// against what the store holds (the starvation math is pinned by
/// `pages::tests::underlay_chain_ppi_shares_the_joint_budget`; it
/// only binds on real devices, where the 8192 ceiling and a fat
/// foreground push the chain past the budget), and the bytes the
/// chain RETAINS are re-reserved so the store's own degradation sees
/// them. This test pins the wiring end-to-end: the builder lands the
/// chain at full density when the budget is healthy, and its live
/// bytes land in the store's reservation.
#[test]
fn underlay_chain_reserves_its_bytes_against_the_store_budget() {
    let mut app = App::with_blank();
    // One background page under the current page (no PageWidth cells
    // → the builder's 8.5×11 defaults; an empty page renders its
    // paper, which is all the raster needs).
    let doc = vsdx::Document {
        pages: vec![
            vsdx::model::Page {
                id: 0,
                name: "BG".into(),
                ..Default::default()
            },
            vsdx::model::Page {
                id: 1,
                name: "FG".into(),
                attrs: vec![("BackPage".to_string(), "0".to_string())],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    app.ed = Some(dc_core::Editor::new(doc));
    app.page_idx = 1;
    let ctx = egui::Context::default();
    let out = app.build_bg_underlays(&ctx);
    assert_eq!(out.len(), 1, "the chain's one background still lands");
    // A healthy budget keeps full density: 8.5 in at 96 ppi ≈ 816 px
    // (the early return never touches the ppi).
    let [w, h] = app.bg_underlays[0].3.size();
    assert!(w > 700 && h > 900, "full-density underlay: {w}×{h}");
    // The chain's landed bytes are re-reserved for the foreground's
    // next degradation decision — the store sees exactly what the
    // retained handles hold.
    let held = (w * h * 4) as u64;
    assert_eq!(
        app.layers.reserved_bytes_for_test(),
        held,
        "the held reservation matches the texture's own bytes"
    );
    // Rebuilding is stable: the cache hit keeps the texture and the
    // reservation idempotent.
    let out = app.build_bg_underlays(&ctx);
    assert_eq!(out.len(), 1);
    assert_eq!(app.bg_underlays[0].3.size(), [w, h]);
    assert_eq!(app.layers.reserved_bytes_for_test(), held);
    // Dropping the background (attrs change) empties the chain, and
    // the reservation follows it to zero — a stale reservation would
    // starve the foreground forever.
    app.ed.as_mut().unwrap().doc.pages[1].attrs.clear();
    let out = app.build_bg_underlays(&ctx);
    assert!(out.is_empty(), "no chain, no underlays");
    assert_eq!(
        app.layers.reserved_bytes_for_test(),
        0,
        "the reservation drops with the chain"
    );
}
