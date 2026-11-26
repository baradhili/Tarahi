//! Tests for the export path's line-jump rendering and stored-arc
//! reconciliation.
use super::*;

/// A minimal page: PageWidth/Height sheet plus `shapes`.
/// #187: the export draws line jumps — a crossing fixture's page
/// SVG carries the jump group with the white gap, the under
/// redraw, and an arc that bulges page-up (smaller SVG y).
#[test]
fn export_renders_line_jumps() {
    let connector = |id: i64, pin: (f64, f64), w: f64, h: f64| {
        let mut s = shape_at(id, pin, w, h, 0.0);
        s.sheet.set_cell("ObjType", Cell::value("2"));
        let line = |ix: i64, x: f64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), Cell::value("0")),
                    ("Y".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            },
            line(2, w, h),
        ];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    };
    // A horizontal connector crossed by a vertical one at (3, 5)
    // strictly inside both segments.
    let (doc, page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0),
        connector(2, (3.0, 5.0), 0.0, 4.0),
    ]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        svg.contains(r#"data-dc="line-jumps""#),
        "the jump overlay group is present"
    );
    assert!(
        svg.contains(r##"stroke="#ffffff""##),
        "the white gap stroke is present"
    );
    // The arc bulges page-UP: the apex sits at a SMALLER SVG y
    // than the crossing's (page y 5 → SVG y 6 on an 11in page).
    // The radius is the page's resolved LineToLineX × factor / 2
    // (#294 — the fixture page sets neither cell, so Visio's
    // defaults apply: 0.125 × 2/3 / 2).
    // Independent expectation — not the renderer's own helper
    // (a tautology would survive a resolver regression, #294
    // review): 0.125 in × 2/3 / 2.
    let apex = format!("{:.5} {:.5}", 3.0, 6.0 - 0.125 * (2.0 / 3.0) / 2.0);
    assert!(svg.contains(&apex), "the arc apex is page-up: {apex}");
}

/// #251: the page's Show-Line-Jumps switch (`ConLineJumpCode=1`
/// on the PageSheet) suppresses the jump overlay entirely; the
/// `LineJumpFactorX` size multiplier scales the hop arc.
#[test]
fn export_gates_line_jumps_on_the_page_setting() {
    let connector = |id: i64, pin: (f64, f64), w: f64, h: f64| {
        let mut s = shape_at(id, pin, w, h, 0.0);
        s.sheet.set_cell("ObjType", Cell::value("2"));
        let line = |ix: i64, x: f64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), Cell::value("0")),
                    ("Y".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            },
            line(2, w, h),
        ];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    };
    let (doc, _page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0),
        connector(2, (3.0, 5.0), 0.0, 4.0),
    ]);
    // Hidden: the PageSheet switch suppresses the group (0 =
    // visPLOJumpNone, the documented off switch per [MS-Learn];
    // #218 review — 6 was never in the enumeration, and a page
    // set to None must not draw hop arcs).
    for off in ["0", "6"] {
        let mut hidden = doc.clone();
        hidden.pages[0]
            .sheet_mut()
            .unwrap()
            .set_cell("LineJumpCode", Cell::value(off));
        let svg = render_page(&hidden, &hidden.pages[0]).unwrap();
        assert!(
            !svg.contains(r#"data-dc="line-jumps""#),
            "the page set to LineJumpCode {off} draws no jump group"
        );
    }
    // An explicit LineJumpFactorX=0 is a zero-size hop (#294
    // review) — no overlay at all, never degenerate hops.
    let mut zero = doc.clone();
    zero.pages[0]
        .sheet_mut()
        .unwrap()
        .set_cell("LineJumpFactorX", Cell::value("0"));
    let svg = render_page(&zero, &zero.pages[0]).unwrap();
    assert!(
        !svg.contains(r##"data-dc="line-jumps""##),
        "a zero jump factor draws no hop group"
    );
    // Shown at 2×: the radius is LineToLineX × factor / 2 (#294) —
    // the fixture page sets only the factor, so 0.125 × 2 / 2.
    let mut big = doc.clone();
    big.pages[0]
        .sheet_mut()
        .unwrap()
        .set_cell("LineJumpFactorX", Cell::value("2"));
    let svg = render_page(&big, &big.pages[0]).unwrap();
    // 0.125 in × 2 / 2, by hand.
    let apex = format!("{:.5} {:.5}", 3.0, 6.0 - 0.125);
    assert!(svg.contains(&apex), "the 2× arc apex: {apex}");
}

/// #187: a page with NO crossings draws no jump group.
/// #294: a connector's stored jump `ArcTo` straightens — the ink
/// follows the run (no `A` command) and the overlay draws the ONE
/// arc at the current crossing with the page's radius (Visio's
/// defaults here: 0.125 × 2/3 / 2 ≈ 0.04167, not the old 0.12).
/// A stored arc sitting OFF any crossing simply vanishes.
#[test]
fn export_straightens_stored_jump_arcs() {
    let connector =
        |id: i64, pin: (f64, f64), w: f64, h: f64, stored_jump: Option<(f64, f64)>| {
            let mut s = shape_at(id, pin, w, h, 0.0);
            s.sheet.set_cell("ObjType", Cell::value("2"));
            let line = |ix: i64, x: f64, y: f64| Row {
                t: Some("LineTo".into()),
                ix,
                cells: vec![
                    ("X".to_string(), Cell::value(format!("{x}"))),
                    ("Y".to_string(), Cell::value(format!("{y}"))),
                ],
                ..Row::default()
            };
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            let mut rows = vec![Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), Cell::value("0")),
                    ("Y".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            }];
            // A stored jump mid-run (the #294 repro's row shape):
            // chord along the run, |A| = the jump radius.
            match stored_jump {
                Some((mid, half)) => {
                    rows.push(line(2, mid - half, 0.0));
                    rows.push(Row {
                        t: Some("ArcTo".into()),
                        ix: 3,
                        cells: vec![
                            ("X".to_string(), Cell::value(format!("{}", mid + half))),
                            ("Y".to_string(), Cell::value("0")),
                            ("A".to_string(), Cell::value(format!("{}", -half))),
                        ],
                        ..Row::default()
                    });
                    rows.push(line(4, w, h));
                }
                None => rows.push(line(2, w, h)),
            }
            sec.rows = rows;
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
    let jump_half = 0.125 * (2.0 / 3.0) / 2.0;
    // Stored jump centered on the crossing (page x = 3): the ink
    // is a straight run, one hop at the crossing.
    let (doc, page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0, Some((2.0, jump_half))),
        connector(2, (3.0, 5.0), 0.0, 4.0, None),
    ]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        !svg.contains(" A "),
        "the stored jump arc renders as a straight run"
    );
    assert_eq!(
        svg.matches(r##"stroke="#ffffff""##).count(),
        1,
        "one jump at the crossing"
    );
    // The gap spans the crossing ± the page's resolved radius
    // (prefix match: the gap line writes full-precision floats).
    assert!(
        svg.contains(r#"x1="2.95833"#) && svg.contains(r#"x2="3.04166"#),
        "the gap is LineToLineX × factor / 2 wide"
    );
    // The stale case: the stored arc sits at page x = 3 while the
    // crossing moved to x = 4.5 — the arc vanishes, one hop at the
    // CURRENT crossing.
    let (doc, page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0, Some((2.0, jump_half))),
        connector(2, (4.5, 5.0), 0.0, 4.0, None),
    ]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        !svg.contains(" A "),
        "the stale stored arc does not render"
    );
    assert!(
        svg.contains(r#"x1="4.45833"#) && !svg.contains(r#"x1="2.95833"#),
        "one hop at the current crossing, nothing at the stale arc"
    );
}

/// #187: a page with NO crossings draws no jump group.
#[test]
fn export_without_crossings_has_no_jump_group() {
    let connector = |id: i64, pin: (f64, f64), w: f64, h: f64| {
        let mut s = shape_at(id, pin, w, h, 0.0);
        s.sheet.set_cell("ObjType", Cell::value("2"));
        let line = |ix: i64, x: f64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), Cell::value("0")),
                    ("Y".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            },
            line(2, w, h),
        ];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    };
    let (doc, page) = page_fixture(vec![connector(1, (3.0, 5.0), 4.0, 0.0)]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        !svg.contains(r##"data-dc="line-jumps""##),
        "no crossings, no overlay"
    );
}

/// #324 step 1 (the live-measured rules, mk-git 2026-11-25): a
/// curved connector never hops and is never hopped over — it leaves
/// the crossing set whole, while the same crossing between two
/// straight connectors hops. The curved route's samples DO cross the
/// straight run geometrically, so only the eligibility gate explains
/// the empty set.
#[test]
fn curved_connectors_leave_the_crossing_set() {
    let connector = |id: i64, pin: (f64, f64), w: f64, h: f64| {
        let mut s = shape_at(id, pin, w, h, 0.0);
        s.sheet.set_cell("ObjType", Cell::value("2"));
        let line = |ix: i64, x: f64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![
                    ("X".to_string(), Cell::value("0")),
                    ("Y".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            },
            line(2, w, h),
        ];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    };
    // Control: straight H × straight V — one hop.
    let (doc, page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0),
        connector(2, (3.0, 5.0), 0.0, 4.0),
    ]);
    assert_eq!(crate::geom::page_jumps(&doc, &page, &[]).len(), 1);
    // The curved twin: a vertical route whose second half is an
    // authored arc bulging far off the run (sagitta 2 over a 2.5
    // chord — far outside the stored-hop band, so it classifies as a
    // curve, not a hop). Its samples cross the horizontal run, but
    // the crossing set stays empty: neither side hops.
    let mut curved = shape_at(2, (3.0, 5.0), 0.0, 4.0, 0.0);
    curved.sheet.set_cell("ObjType", Cell::value("2"));
    let row = |ix: i64, t: &str, cells: Vec<(&str, f64)>| Row {
        t: Some(t.into()),
        ix,
        cells: cells
            .into_iter()
            .map(|(n, v)| (n.to_string(), Cell::value(format!("{v}"))))
            .collect(),
        ..Row::default()
    };
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    sec.rows = vec![
        row(1, "MoveTo", vec![("X", 0.0), ("Y", 0.0)]),
        row(2, "LineTo", vec![("X", 0.0), ("Y", 1.5)]),
        row(3, "ArcTo", vec![("X", 0.0), ("Y", 4.0), ("A", -2.0)]),
    ];
    curved.sheet.children.push(SheetChild::Section(sec));
    let (doc, page) = page_fixture(vec![connector(1, (3.0, 5.0), 4.0, 0.0), curved]);
    assert!(
        crate::geom::page_jumps(&doc, &page, &[]).is_empty(),
        "the curved connector neither hops nor is hopped over"
    );
}
