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
    // The jumper is CUT open (no paper-white gap painted over the
    // background — the hop is part of the line, #322/#324 phase 3):
    // connector 1 renders inside its jump clip, and no white stroke
    // is drawn.
    assert!(
        svg.contains(r#"<g clip-path="url(#dc-jumpcut1)">"#),
        "the jumping connector is clipped open: {svg}"
    );
    assert!(
        !svg.contains(r##"stroke="#ffffff""##),
        "no white gap stroke"
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
    // A VERTICAL-class jumper bulges page-LEFT (Visio's default
    // hop directions, PageLineJumpDirX/Y = 0 — the b01 witness,
    // #322): the horizontal carries ConLineJumpCode 3 ("other
    // line"), so the vertical hops — and it runs SOUTH (its rows
    // walk top to bottom), the heading where flipping toward
    // page-up alone used to bulge the arc sideways RIGHT.
    let (doc, page) = {
        let mut h = connector(1, (3.0, 5.0), 4.0, 0.0);
        h.sheet.set_cell("ConLineJumpCode", Cell::value("3"));
        let mut v = shape_at(2, (3.0, 5.0), 0.0, 4.0, 0.0);
        v.sheet.set_cell("ObjType", Cell::value("2"));
        let row = |ix: i64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value("0")),
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
                    ("Y".to_string(), Cell::value("4")),
                ],
                ..Row::default()
            },
            row(2, 0.0),
        ];
        v.sheet.children.push(SheetChild::Section(sec));
        page_fixture(vec![h, v])
    };
    let svg = render_page(&doc, &page).unwrap();
    let apex = format!("{:.5} {:.5}", 3.0 - 0.125 * (2.0 / 3.0) / 2.0, 6.0);
    assert!(svg.contains(&apex), "the arc apex is page-left: {apex}");
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
    let connector = |id: i64, pin: (f64, f64), w: f64, h: f64, stored_jump: Option<(f64, f64)>| {
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
        svg.matches(r#"data-dc="hop-arc""#).count(),
        1,
        "one hop arc at the crossing"
    );
    // The cut slot spans the crossing ± the page's resolved radius
    // (prefix match: the slot writes full-precision floats, page x
    // first).
    assert!(
        svg.contains("M 2.95833 ") && svg.contains("L 3.04167 "),
        "the cut is LineToLineX × factor / 2 to each side: {svg}"
    );
    // The stale case: the stored arc sits at page x = 3 while the
    // crossing moved to x = 4.5 — the arc vanishes, one hop at the
    // CURRENT crossing.
    let (doc, page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0, Some((2.0, jump_half))),
        connector(2, (4.5, 5.0), 0.0, 4.0, None),
    ]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(!svg.contains(" A "), "the stale stored arc does not render");
    assert!(
        svg.contains("M 4.45833 ") && !svg.contains("M 2.95833 "),
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

/// Two hops closer together than their radii still open the shaft
/// along BOTH: the clips NEST one per hop (clip groups intersect),
/// where a single even-odd path with overlapping slots would re-keep
/// the overlap and the shaft would reappear between the arcs
/// (CodeRabbit round on #324 phase 3).
#[test]
fn export_cuts_overlapping_hop_slots_apart() {
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
    // FactorX 0.8 → radius 0.05: two verticals 0.06 apart cross the
    // horizontal jumper — the slots [2.95, 3.05] and [3.01, 3.11]
    // overlap.
    let (mut doc, mut page) = page_fixture(vec![
        connector(1, (3.0, 5.0), 4.0, 0.0),
        connector(2, (3.0, 5.0), 0.0, 4.0),
        connector(3, (3.06, 5.0), 0.0, 4.0),
    ]);
    doc.pages[0]
        .sheet_mut()
        .unwrap()
        .set_cell("LineJumpFactorX", Cell::value("0.8"));
    let _ = &mut page;
    let svg = render_page(&doc, &page).unwrap();
    assert_eq!(
        svg.matches(r#"data-dc="hop-arc""#).count(),
        2,
        "two hop arcs: {svg}"
    );
    // The jumper's clips NEST: one group per hop (the outer id, then
    // the -1 twin), so the intersection cuts both slots.
    assert_eq!(
        svg.matches(r#"<clipPath id="dc-jumpcut1""#).count(),
        1,
        "the outer clip"
    );
    assert_eq!(
        svg.matches(r#"<clipPath id="dc-jumpcut1-1""#).count(),
        1,
        "the nested second-hop clip"
    );
}

/// The backdrop's ink box only REJECTS: a point inside a triangle's
/// box but outside its fill is NOT the triangle's color (the even-odd
/// ray cast decides — CodeRabbit round on #324 phase 3).
#[test]
fn backdrop_reads_the_fill_not_the_ink_box() {
    let mut t = shape_at(1, (2.0, 2.0), 2.0, 2.0, 0.0);
    t.sheet.set_cell("FillForegnd", Cell::value("#ff0000"));
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    let row = |ix: i64, x: f64, y: f64| Row {
        t: Some("LineTo".into()),
        ix,
        cells: vec![
            ("X".to_string(), Cell::value(format!("{x}"))),
            ("Y".to_string(), Cell::value(format!("{y}"))),
        ],
        ..Row::default()
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
        row(2, 2.0, 0.0),
        row(3, 0.0, 2.0),
    ];
    t.sheet.children.push(SheetChild::Section(sec));
    // A connector ABOVE the triangle (the `below` anchor).
    let c = shape_at(2, (2.0, 2.0), 2.0, 0.0, 0.0);
    let (doc, page) = page_fixture(vec![t, c]);
    // (1.3, 1.3) page: inside the fill; (2.7, 2.7) is inside its 2×2
    // box but outside the fill.
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (1.3, 1.3), 2, &[]),
        (0xff, 0, 0),
        "a point inside the fill reads the fill"
    );
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (2.7, 2.7), 2, &[]),
        (0xff, 0xff, 0xff),
        "the box's empty corner reads the page's white"
    );
}

/// A PARTIALLY transparent panel composites its fill over the lower
/// backdrop instead of painting uncomposited color or being discarded
/// (CodeRabbit round on #324 phase 3): red at `FillForegndTrans` 0.25
/// over nothing reads red·0.75 + white·0.25.
#[test]
fn backdrop_composites_partial_transparency() {
    let mut p = shape_at(1, (2.0, 2.0), 2.0, 2.0, 0.0);
    p.sheet.set_cell("FillForegnd", Cell::value("#ff0000"));
    p.sheet.set_cell("FillForegndTrans", Cell::value("0.25"));
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    let row = |ix: i64, x: f64, y: f64| Row {
        t: Some("LineTo".into()),
        ix,
        cells: vec![
            ("X".to_string(), Cell::value(format!("{x}"))),
            ("Y".to_string(), Cell::value(format!("{y}"))),
        ],
        ..Row::default()
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
        row(2, 2.0, 0.0),
        row(3, 0.0, 2.0),
    ];
    p.sheet.children.push(SheetChild::Section(sec));
    let c = shape_at(2, (2.0, 2.0), 2.0, 0.0, 0.0);
    let (doc, page) = page_fixture(vec![p, c]);
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (1.3, 1.3), 2, &[]),
        (0xff, 0x40, 0x40),
        "0.75 red over white: the blend, not raw red"
    );
}

/// The even-odd cast counts the renderer's IMPLICIT closing edge: an
/// open M→L→L contour (right angle at (3,3) page) fills its diagonal,
/// so a point past the diagonal in the box's far corner reads the
/// page's white — the open cast counted only the x=3 edge and wrongly
/// read red there (CodeRabbit round on #324 phase 3).
#[test]
fn backdrop_closes_the_contour_in_the_cast() {
    let mut t = shape_at(1, (2.0, 2.0), 2.0, 2.0, 0.0);
    t.sheet.set_cell("FillForegnd", Cell::value("#ff0000"));
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    let row = |ix: i64, x: f64, y: f64| Row {
        t: Some("LineTo".into()),
        ix,
        cells: vec![
            ("X".to_string(), Cell::value(format!("{x}"))),
            ("Y".to_string(), Cell::value(format!("{y}"))),
        ],
        ..Row::default()
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
        row(2, 2.0, 0.0),
        row(3, 2.0, 2.0),
    ];
    t.sheet.children.push(SheetChild::Section(sec));
    let c = shape_at(2, (2.0, 2.0), 2.0, 0.0, 0.0);
    let (doc, page) = page_fixture(vec![t, c]);
    // (2.8, 2.2) page: below the diagonal — inside the closed fill.
    // (2.2, 2.8): above it — outside the fill, inside the box, where
    // the OPEN cast counted the x=3 edge alone and read red.
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (2.8, 2.2), 2, &[]),
        (0xff, 0, 0),
        "below the diagonal the fill reads red"
    );
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (2.2, 2.8), 2, &[]),
        (0xff, 0xff, 0xff),
        "past the diagonal the closed contour reads the page's white"
    );
}

/// The blend runs BOTTOM-UP (painter's algorithm): an upper
/// semi-transparent fill composites over the lower one — red at 0.75
/// above an opaque blue panel reads red·0.75 + blue·0.25, never the
/// lower panel overwriting the upper blend (CodeRabbit round on
/// #324 phase 3).
#[test]
fn backdrop_blends_lower_fills_under_upper() {
    let panel = |id: i64, color: &str, trans: Option<f64>| {
        let mut p = shape_at(id, (2.0, 2.0), 2.0, 2.0, 0.0);
        p.sheet.set_cell("FillForegnd", Cell::value(color));
        if let Some(t) = trans {
            p.sheet
                .set_cell("FillForegndTrans", Cell::value(format!("{t}")));
        }
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        let row = |ix: i64, x: f64, y: f64| Row {
            t: Some("LineTo".into()),
            ix,
            cells: vec![
                ("X".to_string(), Cell::value(format!("{x}"))),
                ("Y".to_string(), Cell::value(format!("{y}"))),
            ],
            ..Row::default()
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
            row(2, 2.0, 0.0),
            row(3, 2.0, 2.0),
            row(4, 0.0, 2.0),
        ];
        p.sheet.children.push(SheetChild::Section(sec));
        p
    };
    // Blue opaque at the LOWEST z, red 0.75 above it, the connector
    // on top (the `below` anchor).
    let blue = panel(1, "#0000ff", None);
    let red = panel(2, "#ff0000", Some(0.25));
    let c = shape_at(3, (2.0, 2.0), 2.0, 0.0, 0.0);
    let (doc, page) = page_fixture(vec![blue, red, c]);
    assert_eq!(
        crate::svg::backdrop_color_at(&doc, &page, (2.0, 2.0), 3, &[]),
        (191, 0, 64),
        "red·0.75 + blue·0.25: the upper fill wins by its own alpha"
    );
}
