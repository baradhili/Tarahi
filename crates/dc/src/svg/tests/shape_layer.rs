//! Tests for the per-shape render walk: master mirroring, geometry
//! rows, arrowheads, paint, and the layer serializations.
use super::*;

#[test]
fn mirrored_proportional_child_keeps_uniform_scale() {
    let (root, inst) = role_master_fixture();
    let (iw, ih) = (2.657480296178819, 5.528301723437821);
    let (mw, mh) = (1.181102362204724, 0.7874015748031495);
    let (kx, ky) = (iw / mw, ih / mh); // 2.25 × 7.020943
    let env = mirror_env(&root, &inst, iw, ih);
    // Container (Sheet.5!Width*1 …): exactly the historical uniform
    // scale — no regression for proportional children.
    let t = mirror_child_transform(&root.sub_shapes[0], &env, (kx, ky)).unwrap();
    assert_eq!(t, "scale(2.250000,7.020943)", "got {t}");
}

#[test]
fn mirrored_guarded_child_keeps_absolute_size() {
    let (root, inst) = role_master_fixture();
    let (iw, ih) = (2.657480296178819, 5.528301723437821);
    let env = mirror_env(&root, &inst, iw, ih);
    // Icon: 4×4.3 mm stays absolute; its pin re-evaluates against the
    // instance Height (User.IconY = H − InfoPanelHeight/2).
    let t = mirror_child_transform(&root.sub_shapes[1], &env, (2.25, 7.02)).unwrap();
    let m = parse_tf(&t);
    let e_py = ih - 0.2362204724409449 / 2.0; // 5.410191487…
    let m_py = 0.6692913385826771;
    let m_h = 0.1692913385826771;
    // wanted ∘ placed⁻¹ for angle 0, scale 1: pure translate by the
    // pin delta (effective LocPin = W/2 in both frames).
    let expected_ty = -((e_py - 0.1692913385826771 / 2.0) - (m_py - m_h / 2.0));
    assert!((m[0] - 1.0).abs() < 1e-6, "a={}", m[0]);
    assert!((m[3] - 1.0).abs() < 1e-6, "d={}", m[3]);
    assert!(m[4].abs() < 1e-6, "e={}", m[4]);
    assert!(
        (m[5] - expected_ty).abs() < 1e-5,
        "f={} expected {expected_ty}",
        m[5]
    );
}

#[test]
fn mirrored_literal_child_falls_back_to_uniform_scale() {
    let (root, inst) = role_master_fixture();
    let env = mirror_env(&root, &inst, 2.657480296178819, 5.528301723437821);
    // No formulas anywhere: behavior is byte-identical to the old
    // uniform wrapper.
    let t = mirror_child_transform(&root.sub_shapes[2], &env, (2.25, 7.020943)).unwrap();
    assert_eq!(t, "scale(2.250000,7.020943)", "got {t}");
}

#[test]
fn mirror_env_resolves_user_formulas_against_instance_height() {
    let (root, inst) = role_master_fixture();
    let env = mirror_env(&root, &inst, 2.657480296178819, 5.528301723437821);
    let v = env.get("User.IconY").copied().unwrap();
    assert!((v - (5.528301723437821 - 0.2362204724409449 / 2.0)).abs() < 1e-9);
}

/// A locally CLEARED User row (`V=''`) shadows the master's
/// numeric binding — the string env seeds the empty value, and
/// the numeric env must not keep the master's 42 for the
/// evaluator to read first (CodeRabbit round 5). A restated
/// local value still overrides, and no local row still inherits.
#[test]
fn cleared_user_row_shadows_the_master_number() {
    let user_sheet = |v: Option<&str>| {
        let mut sheet = Sheet::default();
        sheet.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "User".into(),
                rows: vec![vsdx::model::Row {
                    n: Some("Label".into()),
                    cells: vec![(
                        "Value".to_string(),
                        Cell {
                            v: v.map(str::to_string),
                            ..Cell::default()
                        },
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            },
        ));
        sheet
    };
    let mut doc = Document::default();
    doc.masters.push(vsdx::model::Master {
        id: 7,
        shapes: vec![Shape {
            id: 1,
            sheet: user_sheet(Some("42")),
            ..Shape::default()
        }],
        ..vsdx::model::Master::default()
    });
    // No local row: the master's 42 inherits.
    let inherit = Shape {
        id: 1,
        master: Some(7),
        ..Shape::default()
    };
    let chain = [&inherit];
    assert_eq!(
        shape_env(&doc, &inherit, &chain).get("User.Label").copied(),
        Some(42.0),
        "no local row inherits the master's number"
    );
    // A cleared local row removes the binding.
    let cleared = Shape {
        id: 1,
        master: Some(7),
        sheet: user_sheet(Some("")),
        ..Shape::default()
    };
    let chain = [&cleared];
    assert_eq!(
        shape_env(&doc, &cleared, &chain).get("User.Label"),
        None,
        "a cleared row shadows the master's number"
    );
    // A restated local value overrides.
    let local = Shape {
        id: 1,
        master: Some(7),
        sheet: user_sheet(Some("7")),
        ..Shape::default()
    };
    let chain = [&local];
    assert_eq!(
        shape_env(&doc, &local, &chain).get("User.Label").copied(),
        Some(7.0),
        "a local value overrides the master's"
    );

    // An Inh-marked row inherits instead of overriding — its
    // numeric cache is the stale snapshot; and an Inh-marked
    // EMPTY cache inherits rather than clearing (CodeRabbit
    // agent review).
    let with_row = |f: Option<&str>, v: Option<&str>| {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.children.push(vsdx::model::SheetChild::Section(
            vsdx::model::Section {
                name: "User".into(),
                rows: vec![vsdx::model::Row {
                    n: Some("Label".into()),
                    cells: vec![(
                        "Value".to_string(),
                        vsdx::model::Cell {
                            v: v.map(str::to_string),
                            f: f.map(str::to_string),
                            ..vsdx::model::Cell::default()
                        },
                    )],
                    ..vsdx::model::Row::default()
                }],
                ..vsdx::model::Section::default()
            },
        ));
        sheet
    };
    let mut doc2 = Document::default();
    doc2.masters.push(vsdx::model::Master {
        id: 8,
        shapes: vec![Shape {
            id: 1,
            sheet: user_sheet(Some("42")),
            ..Shape::default()
        }],
        ..vsdx::model::Master::default()
    });
    let stale = Shape {
        id: 1,
        master: Some(8),
        sheet: with_row(Some("Inh"), Some("41")),
        ..Shape::default()
    };
    let chain = [&stale];
    assert_eq!(
        shape_env(&doc2, &stale, &chain).get("User.Label").copied(),
        Some(42.0),
        "an Inh row's stale cache does not override the master"
    );
    let inh_empty = Shape {
        id: 1,
        master: Some(8),
        sheet: with_row(Some("Inh"), Some("")),
        ..Shape::default()
    };
    let chain = [&inh_empty];
    assert_eq!(
        shape_env(&doc2, &inh_empty, &chain).get("User.Label").copied(),
        Some(42.0),
        "an Inh row's empty cache inherits, it does not clear"
    );
}

#[test]
fn line_pattern_dasharrays_scale_with_weight() {
    // #171 measured the INK (the owner-authored lines.vsdx fixture
    // printed at 600 dpi — Analysis/line-patterns-visio2016.md);
    // #235 reconciles it with MisterDiagram's canonical grammar:
    // Visio strokes dashes with ROUND caps, so the emitted
    // geometry is ink − 1w per mark / ink + 1w per gap (pattern 2
    // = the canonical 7/5, 14 = 9/3/3, 21 = 39/9/15/9), and dots
    // (ink 1w) become the 0.0001w tangent lead with the following
    // gap returning it — same painted ink, canonical parameters.
    // The values below are the CANONICAL GEOMETRY × weight, dotted
    // against the three witnessed grammar entries.
    let w = 0.02; // the fixture's weight
    let arr = |p: f64| line_dasharray(p, w);
    assert_eq!(arr(2.0).as_deref(), Some("0.14000 0.10000")); // canonical 7/5
    assert_eq!(arr(3.0).as_deref(), Some("0.0000020000 0.10000"));
    assert_eq!(
        arr(4.0).as_deref(),
        Some("0.14000 0.10000 0.0000020000 0.10000")
    );
    assert_eq!(
        arr(5.0).as_deref(),
        Some("0.14000 0.10000 0.0000020000 0.10000 0.0000020000 0.10000")
    );
    assert_eq!(
        arr(6.0).as_deref(),
        Some("0.14000 0.10000 0.14000 0.10000 0.0000020000 0.10000")
    );
    assert_eq!(arr(7.0).as_deref(), Some("0.38000 0.10000 0.14000 0.10000"));
    assert_eq!(
        arr(8.0).as_deref(),
        Some("0.38000 0.10000 0.14000 0.10000 0.14000 0.10000")
    );
    assert_eq!(arr(9.0).as_deref(), Some("0.06000 0.06000"));
    assert_eq!(arr(10.0).as_deref(), Some("0.0000020000 0.06000"));
    assert_eq!(
        arr(11.0).as_deref(),
        Some("0.06000 0.06000 0.0000020000 0.06000")
    );
    assert_eq!(
        arr(12.0).as_deref(),
        Some("0.06000 0.06000 0.0000020000 0.06000 0.0000020000 0.06000")
    );
    assert_eq!(
        arr(13.0).as_deref(),
        Some("0.06000 0.06000 0.06000 0.06000 0.0000020000 0.06000")
    );
    assert_eq!(
        arr(14.0).as_deref(),
        Some("0.18000 0.06000 0.06000 0.06000") // canonical 9/3/3
    );
    assert_eq!(
        arr(15.0).as_deref(),
        Some("0.18000 0.06000 0.06000 0.06000 0.06000 0.06000")
    );
    assert_eq!(arr(16.0).as_deref(), Some("0.30000 0.18000"));
    assert_eq!(arr(17.0).as_deref(), Some("0.0000020000 0.18000"));
    assert_eq!(
        arr(18.0).as_deref(),
        Some("0.30000 0.18000 0.0000020000 0.18000")
    );
    assert_eq!(
        arr(19.0).as_deref(),
        Some("0.30000 0.18000 0.0000020000 0.18000 0.0000020000 0.18000")
    );
    assert_eq!(
        arr(20.0).as_deref(),
        Some("0.30000 0.18000 0.30000 0.18000 0.0000020000 0.18000")
    );
    assert_eq!(
        arr(21.0).as_deref(),
        Some("0.78000 0.18000 0.30000 0.18000") // canonical 39/9/15/9
    );
    assert_eq!(
        arr(22.0).as_deref(),
        Some("0.78000 0.18000 0.30000 0.18000 0.30000 0.18000")
    );
    assert_eq!(arr(23.0).as_deref(), Some("0.02000 0.04000"));
    // Solid (1) and hidden (0, handled at the call site) render
    // without a dasharray; so do out-of-gallery values.
    assert!(line_dasharray(1.0, w).is_none());
    assert!(line_dasharray(0.0, w).is_none());
    assert!(line_dasharray(24.0, w).is_none());
}

/// #72 + #199 + #185: arrowheads render per value at their
/// measured physical sizes — each gallery entry emits its own
/// head group matching the 600 dpi Visio print (open heads 14–22
/// stroke, the dimension set composes ticks with rings/dots, 12
/// is the single wide V, 43 the double chevron), the head scales
/// with the LineWeight + C[size] unit law, and 0/46–50 emit none.
/// #218 review round: fractional NURBS controls (xType/yType = 0)
/// in the MASTER frame scale exactly once — a fraction lands at
/// gx·W instance units (gx·mw master units through the one
/// master→instance scale), never the old gx·W·scale. A degree-1
/// NURBS flattens through its control points, so the emitted
/// path pins them.
#[test]
fn nurbs_fractional_controls_scale_once_in_master_frame() {
    // Master: a 2×2 canvas; a degree-1 NURBS whose interior
    // controls sit at x fractions 0.5 and 0.75, y fraction 0.5.
    let mut mroot = Shape {
        id: 1,
        ..Shape::default()
    };
    for (n, v) in [("Width", "2"), ("Height", "2")] {
        mroot.sheet.set_cell(n, Cell::value(v));
    }
    mroot.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 0,
        rows: vec![
            Row {
                ix: 1,
                t: Some("MoveTo".into()),
                cells: vec![
                    ("X".into(), Cell::value("0")),
                    ("Y".into(), Cell::value("0")),
                ],
                ..Row::default()
            },
            Row {
                ix: 2,
                t: Some("NURBSTo".into()),
                cells: vec![
                    ("X".into(), Cell::value("2")),
                    ("Y".into(), Cell::value("2")),
                    ("A".into(), Cell::value("2")),
                    ("B".into(), Cell::value("1")),
                    ("C".into(), Cell::value("0")),
                    ("D".into(), Cell::value("1")),
                    (
                        "E".into(),
                        Cell {
                            v: Some("NURBS(3, 1, 0, 0, 0.5,0.5,1,1, 0.75,0.5,2,1)".into()),
                            ..Cell::default()
                        },
                    ),
                ],
                ..Row::default()
            },
        ],
        ..Section::default()
    }));
    let doc = Document {
        masters: vec![vsdx::model::Master {
            id: 7,
            shapes: vec![mroot],
            ..vsdx::model::Master::default()
        }],
        ..Document::default()
    };
    // Instance: 4×2 — double the master width, equal height.
    let mut inst = Shape {
        id: 9,
        master: Some(7),
        ..Shape::default()
    };
    for (n, v) in [
        ("PinX", "3"),
        ("PinY", "3"),
        ("Width", "4"),
        ("Height", "2"),
        ("LocPinX", "0"),
        ("LocPinY", "0"),
    ] {
        inst.sheet.set_cell(n, Cell::value(v));
    }
    let chain: Vec<&Shape> = Vec::new();
    let mut out = String::new();
    render_shape(
        &doc, &inst, 8.5, &chain, "", None, None, None, None, (0.0, 0.0), &mut out,
    );
    // The controls land at gx·W: 0.5 → 2.0 and 0.75 → 3.0; the y
    // fraction 0.5 of H=2 → 1.0 (emitted negated). The old
    // double-scale put them at 4.0 and 6.0.
    assert!(
        out.contains("2.00000 -1.00000"),
        "control 1 (0.5·W) missing: {out}"
    );
    assert!(
        out.contains("3.00000 -1.00000"),
        "control 2 (0.75·W) missing: {out}"
    );
    assert!(
        !out.contains("6.00000"),
        "double-scaled control leaked: {out}"
    );
}

#[test]
fn arrow_heads_render_per_value_and_scale() {
    let doc = Document::default();
    let line_with = |cells: &[(&str, &str)]| {
        let mut s = Shape::default();
        for (n, v) in cells {
            s.sheet.set_cell(n, Cell::value(*v));
        }
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Geometry".into(),
            ix: 0,
            rows: vec![
                Row {
                    ix: 1,
                    t: Some("MoveTo".into()),
                    cells: vec![
                        ("X".into(), Cell::value("0")),
                        ("Y".into(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                Row {
                    ix: 2,
                    t: Some("LineTo".into()),
                    cells: vec![
                        ("X".into(), Cell::value("1")),
                        ("Y".into(), Cell::value("1")),
                    ],
                    ..Row::default()
                },
            ],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        render_shape(&doc, &s, 11.0, &chain, "", None, None, None, None, (0.0, 0.0), &mut out);
        out
    };
    // 0 = no head at all.
    let none = line_with(&[("EndArrow", "0")]);
    assert!(!none.contains("dc-ah"), "value 0 emits no head: {none}");
    // Each gallery value emits a head group with its own unit-space
    // shape (#169 + #199 + the #185 print re-measure).
    for (v, needle) in [
        (1, "M-1,-1 L0,0 L-1,1"),
        (2, "M0,0 L-1,-1 L-1,1 Z"),
        (3, "M-2,-1 L0,0 L-2,1"),
        (4, "M0,0 L-2,-1 L-2,1 Z"),
        (5, "C-1.68524,-0.37049"),
        (6, "C-2.31476,-0.37049"),
        (9, "M1,-1 L-1,1"),
        (11, "M-0.75,-0.75 L0.75,-0.75 L0.75,0.75 L-0.75,0.75 Z"),
        (12, "M-3,-1 L0,0 L-3,1"),
        (13, "M0,0 L-3,-1 L-3,1 Z"),
        (14, "M0,0 L-3,-1 L-3,1 Z"),
        (15, "M0,0 L-1,-1 L-1,1 Z"),
        (22, "M0,0 L-2.25,-1.125 L-4.5,0 L-2.25,1.125 Z"),
        (25, "M-2.25,-0.75 L-2.25,0.75"),
        (27, "M0,-0.75 L-1.5,0 L0,0.75"),
        (40, "M0,0 L-2,-1 L-2,1 Z M-2,0 L-4,-1 L-4,1 Z"),
        (43, "M-4,-1 L-2,0 L-4,1"),
        (45, "M-5,-0.75 L-5,0.75"),
    ] {
        let out = line_with(&[("EndArrow", &v.to_string())]);
        assert!(out.contains("dc-ah"), "value {v} emits a head group: {out}");
        assert!(out.contains(needle), "value {v} draws its shape: {out}");
    }
    // The dots/rings are circle elements (10/42 filled, 20/41
    // rings; the 35–37 dots carry the measured 0.825 radius).
    for (v, ring) in [(10i64, false), (20, true), (41, true), (42, false)] {
        let out = line_with(&[("EndArrow", &v.to_string())]);
        assert!(out.contains("<circle"), "value {v} is a circle: {out}");
        let head = out.split(r#"class="dc-ah""#).nth(1).expect("head group");
        assert_eq!(head.contains("fill=\"none\""), ring, "value {v}: {out}");
    }
    let dot35 = line_with(&[("EndArrow", "35")]);
    assert!(
        dot35.contains(r#"r="0.825""#) && dot35.contains("M-2.25,-0.75 L-2.25,0.75"),
        "value 35 composes tick + 0.825 dot: {dot35}"
    );
    // 8 shares the stealth-dart glyph with 5 (the native SVG alias;
    // the print's concave rear agrees).
    let a5 = line_with(&[("EndArrow", "5")]);
    let a8 = line_with(&[("EndArrow", "8")]);
    let d5 = a5.split(r#"class="dc-ah""#).nth(1).expect("5");
    let d8 = a8.split(r#"class="dc-ah""#).nth(1).expect("8");
    assert_eq!(d5, d8, "8 renders the 5 glyph");
    // #169 regression: 13 is a SINGLE head — one closed triangle —
    // and the other filled singles (2, 4) never stack.
    for v in [2i64, 4, 13] {
        let out = line_with(&[("EndArrow", &v.to_string())]);
        let closes = out.matches(" Z").count();
        assert!(closes <= 1, "value {v} must not stack triangles: {out}");
    }
    // #199: the open heads 14–22 stroke (fill="none"), never fill;
    // the double triangles 39/40 stack two heads, filled vs open.
    for v in 14..=22i64 {
        let out = line_with(&[("EndArrow", &v.to_string())]);
        assert!(
            out.contains("fill=\"none\""),
            "value {v} is an open head: {out}"
        );
    }
    let d39 = line_with(&[("EndArrow", "39")]);
    assert_eq!(d39.matches(" Z").count(), 2, "39 stacks two: {d39}");
    let d40 = line_with(&[("EndArrow", "40")]);
    assert_eq!(d40.matches(" Z").count(), 2, "40 stacks two: {d40}");
    assert!(d40.contains("fill=\"none\""), "40 is the open double");
    // #158: past the gallery, Visio draws a bare end (fixture
    // values 46–50 print no head).
    for v in 46..=50i64 {
        let out = line_with(&[("EndArrow", &v.to_string())]);
        assert!(!out.contains("dc-ah"), "value {v} emits no head: {out}");
    }
    // The unit law (#185): the head scales by LineWeight + C[size]
    // — size 1 (C=1.8 pt) at a 0.05 in pen gives 0.075 in.
    let heavy = line_with(&[
        ("EndArrow", "1"),
        ("EndArrowSize", "1"),
        ("LineWeight", "0.05"),
    ]);
    assert!(
        heavy.contains("scale(0.07500)"),
        "the head rides LineWeight + C[size]: {heavy}"
    );
    // Begin + end heads face outward/along the travel: the
    // diagonal runs to (1, -1) in the y-down frame, so the end
    // head points at -45° and the begin head outward at 135°.
    let both = line_with(&[("BeginArrow", "1"), ("EndArrow", "1")]);
    assert!(
        both.contains("rotate(-45.0000)"),
        "end head along the travel: {both}"
    );
    assert!(
        both.contains("rotate(135.0000)"),
        "begin head faces outward: {both}"
    );
    // The shaft trims at the head's rear (setback 2 × unit for the
    // classic triangle), the head anchoring at the ORIGINAL end.
    let unit = 0.75 / 72.0 + 2.52 / 72.0;
    let t = (2.0f64.sqrt() - 2.0 * unit) / 2.0f64.sqrt();
    let trimmed = line_with(&[("EndArrow", "4")]);
    assert!(
        trimmed.contains(&format!("L {:.5} -{:.5}", t, t)),
        "the shaft stops at the setback: {trimmed}"
    );
    assert!(
        trimmed.contains("translate(1.00000"),
        "the head keeps the original endpoint: {trimmed}"
    );
    // The begin setback is the end setback minus max(1%, 0.36 pt).
    let both4 = line_with(&[("BeginArrow", "4"), ("EndArrow", "4")]);
    let begin_setback = 2.0 * unit - (2.0 * unit * 0.01).max(0.36 / 72.0);
    let bt = begin_setback / 2.0f64.sqrt();
    assert!(
        both4.contains(&format!("M {:.5} -{:.5}", bt, bt)),
        "the begin trim is the smaller one: {both4}"
    );
}

/// #227: a geometry whose walk closes draws no line ends — Visio
/// renders a closed rectangle with EndArrow=5 as a clean corner
/// (MisterDiagram #105 field batch); an open control line beside
/// it keeps its head, and the closed section keeps its stroke.
#[test]
fn closed_geometry_draws_no_arrowheads() {
    let doc = Document::default();
    let render = |rows: Vec<Row>, cells: &[(&str, &str)]| {
        let mut s = Shape::default();
        for (n, v) in cells {
            s.sheet.set_cell(n, Cell::value(*v));
        }
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Geometry".into(),
            ix: 0,
            rows,
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        render_shape(&doc, &s, 11.0, &chain, "", None, None, None, None, (0.0, 0.0), &mut out);
        out
    };
    // A closed rectangle walk (returns to its start) with both
    // arrow cells set.
    let closed = render(
        vec![
            w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "1"), ("Y", "0")]),
            w3_row(3, "LineTo", &[("X", "1"), ("Y", "1")]),
            w3_row(4, "LineTo", &[("X", "0"), ("Y", "1")]),
            w3_row(5, "LineTo", &[("X", "0"), ("Y", "0")]),
        ],
        &[("EndArrow", "5"), ("BeginArrow", "5")],
    );
    assert!(
        !closed.contains("dc-ah"),
        "a closed walk draws no line ends: {closed}"
    );
    assert!(
        closed.contains("<path") && closed.contains("L 0.00000 -0.00000"),
        "the closed contour still strokes: {closed}"
    );
    // The open control keeps its head.
    let open = render(
        vec![
            w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "1"), ("Y", "1")]),
        ],
        &[("EndArrow", "5")],
    );
    assert!(
        open.contains("dc-ah"),
        "the open line keeps its head: {open}"
    );
    // A MULTI-subpath walk takes no heads either — Visio gives
    // them none (the trim_path evidence note; CodeRabbit flagged
    // the last-subpath-closed case, #278).
    let multi = render(
        vec![
            w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "1"), ("Y", "0")]),
            w3_row(3, "MoveTo", &[("X", "2"), ("Y", "0")]),
            w3_row(4, "LineTo", &[("X", "3"), ("Y", "1")]),
        ],
        &[("EndArrow", "5"), ("BeginArrow", "5")],
    );
    assert!(
        !multi.contains("dc-ah"),
        "a multi-subpath walk draws no line ends: {multi}"
    );
}

/// #233: an authored D=0 (zero-width) EllipticalArcTo draws the
/// endpoint CHORD even when its A/B control is off the chord —
/// the old fallback's quadratic-through-m doubled back
/// (MisterDiagram M5.50; 887 corpus masters carry these rows). A
/// positive-width control on the same points still solves as an
/// arc.
#[test]
fn zero_width_elliptical_arc_to_draws_the_chord() {
    let doc = Document::default();
    let render = |d: &str| {
        let mut s = Shape::default();
        s.sheet.set_cell("Width", Cell::value("2"));
        s.sheet.set_cell("Height", Cell::value("2"));
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Geometry".into(),
            ix: 0,
            rows: vec![
                w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
                w3_row(
                    2,
                    "EllipticalArcTo",
                    &[
                        ("X", "1"),
                        ("Y", "0"),
                        ("A", "0.5"),
                        ("B", "1"),
                        ("C", "0"),
                        ("D", d),
                    ],
                ),
            ],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        render_shape(&doc, &s, 11.0, &chain, "", None, None, None, None, (0.0, 0.0), &mut out);
        out
    };
    let flat = render("0");
    assert!(
        flat.contains("L 1.00000 -0.00000"),
        "D=0 draws the endpoint chord: {flat}"
    );
    assert!(!flat.contains("Q "), "no doubled-back quadratic: {flat}");
    assert!(
        !flat.contains(" A "),
        "no solved arc for a zero-width row: {flat}"
    );
    let arc = render("0.5");
    assert!(
        arc.contains(" A "),
        "a positive width still solves an arc: {arc}"
    );
}

/// #235: an explicit-zero LineWeight keeps zero — SVG paints
/// nothing at stroke-width 0, and the old `.max(0.001)` fallback
/// pen drew stray hairlines Visio never shows.
#[test]
fn explicit_zero_line_weight_paints_nothing() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("LineWeight", Cell::value("0"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 0,
        rows: vec![
            w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "1"), ("Y", "1")]),
        ],
        ..Section::default()
    }));
    let chain = [&s];
    let mut out = String::new();
    render_shape(&doc, &s, 11.0, &chain, "", None, None, None, None, (0.0, 0.0), &mut out);
    assert!(
        out.contains(r#"stroke-width="0.00000""#),
        "an explicit zero stays zero: {out}"
    );
    assert!(
        !out.contains(r#"stroke-width="0.001"#),
        "no fallback pen: {out}"
    );
}

/// #228: every fill-eligible Geometry section of one shape joins
/// ONE alternate-fill path (`fill-rule="evenodd"`) under the
/// per-section strokes — nesting and overlap toggle coverage
/// regardless of winding direction (MisterDiagram M5.109: the
/// two rects here wind in OPPOSITE directions, which nonzero
/// fill would paint solid). NoFill opts a section out of the
/// fill only; NoLine leaves the fill in and drops the stroke.
#[test]
fn sections_fill_one_even_odd_path() {
    let doc = Document::default();
    let mut s = Shape::default();
    // Outer rect, clockwise.
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 0,
        rows: vec![
            w3_row(1, "MoveTo", &[("X", "0"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "2"), ("Y", "0")]),
            w3_row(3, "LineTo", &[("X", "2"), ("Y", "2")]),
            w3_row(4, "LineTo", &[("X", "0"), ("Y", "2")]),
            w3_row(5, "LineTo", &[("X", "0"), ("Y", "0")]),
        ],
        ..Section::default()
    }));
    // Inner rect, COUNTER-clockwise — same coverage under
    // even-odd, solid under nonzero.
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 1,
        rows: vec![
            w3_row(1, "MoveTo", &[("X", "0.5"), ("Y", "0.5")]),
            w3_row(2, "LineTo", &[("X", "0.5"), ("Y", "1.5")]),
            w3_row(3, "LineTo", &[("X", "1.5"), ("Y", "1.5")]),
            w3_row(4, "LineTo", &[("X", "1.5"), ("Y", "0.5")]),
            w3_row(5, "LineTo", &[("X", "0.5"), ("Y", "0.5")]),
        ],
        ..Section::default()
    }));
    // A NoFill section: stroke only, excluded from the union.
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 2,
        cells: vec![("NoFill".to_string(), Cell::value("1"))],
        rows: vec![
            w3_row(1, "MoveTo", &[("X", "3"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "3"), ("Y", "1")]),
        ],
        ..Section::default()
    }));
    // A NoLine section: fill only — joins the union, no stroke.
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 3,
        cells: vec![("NoLine".to_string(), Cell::value("1"))],
        rows: vec![
            w3_row(1, "MoveTo", &[("X", "4"), ("Y", "0")]),
            w3_row(2, "LineTo", &[("X", "4"), ("Y", "1")]),
        ],
        ..Section::default()
    }));
    let chain = [&s];
    let mut out = String::new();
    render_shape(&doc, &s, 11.0, &chain, "", None, None, None, None, (0.0, 0.0), &mut out);
    // ONE even-odd fill element, first among the paths (under the
    // ink), carrying exactly the three fill-eligible subpaths.
    assert_eq!(
        out.matches("fill-rule=\"evenodd\"").count(),
        1,
        "one alternate fill per shape: {out}"
    );
    let fill_elem = out
        .split("fill-rule=\"evenodd\"")
        .nth(1)
        .expect("fill element present");
    let fill_elem = &fill_elem[..fill_elem.find("/>").unwrap_or(fill_elem.len())];
    assert_eq!(
        fill_elem.matches("M ").count(),
        3,
        "the two rects (either winding) and the NoLine section join; NoFill stays out: {fill_elem}"
    );
    // The strokes stay per-section: outer, inner, NoFill open
    // line — three stroke paths after the fill element.
    let after_fill = out.split("fill-rule=\"evenodd\"").nth(1).unwrap_or("");
    assert_eq!(
        after_fill.matches("<path").count(),
        3,
        "three per-section strokes follow the fill: {after_fill}"
    );
    assert!(
        after_fill.contains(r#"fill="none""#),
        "stroke paths carry no fill: {after_fill}"
    );
}

/// D is the ratio of the axis along C to the axis across it
/// (Visio 2021 witness t14, Greptile on PR #330): through
/// (0,.5)-(1,1)-(2,.5) in a 2×1 box with C = 0, D = 2 draws the
/// flat half-ellipse (along 1, across 0.5), D = 0.5 a tall one.
/// The ratio-1 solver tests alone would not catch an inverted D.
#[test]
fn elliptical_arc_d_is_along_over_across() {
    let arc = |d: f64| {
        let mut s = shape_at(1, (2.0, 2.0), 2.0, 1.0, 0.0);
        push_geo(
            &mut s,
            vec![
                geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.5)]),
                geo_row(
                    2,
                    "EllipticalArcTo",
                    &[
                        ("X", 2.0),
                        ("Y", 0.5),
                        ("A", 1.0),
                        ("B", 1.0),
                        ("C", 0.0),
                        ("D", d),
                    ],
                ),
            ],
        );
        let (doc, page) = page_fixture(vec![s]);
        let svg = render_page(&doc, &page).unwrap();
        let i = svg.find(" A ").unwrap_or_else(|| panic!("no arc: {svg}"));
        let v: Vec<f64> = svg[i + 3..]
            .split_whitespace()
            .take(3)
            .map(|t| t.parse().unwrap())
            .collect();
        // (along C, across C): rx/ry, swapped when the solver
        // reports the axes rotated by 90°.
        let rot = v[2].rem_euclid(180.0);
        if (rot - 90.0).abs() < 1e-3 {
            (v[1], v[0])
        } else {
            assert!(rot.abs() < 1e-3 || (rot - 180.0).abs() < 1e-3, "rot {rot}");
            (v[0], v[1])
        }
    };
    let (along, across) = arc(2.0);
    assert!(
        (along - 1.0).abs() < 1e-4 && (across - 0.5).abs() < 1e-4,
        "D=2: the flat half-ellipse ({along}, {across})"
    );
    let (along, across) = arc(0.5);
    assert!(
        (along / across - 0.5).abs() < 1e-4,
        "D=0.5: along/across = 0.5 ({along}, {across})"
    );
}

#[test]
fn elliptical_arc_half_circle() {
    // Upper half of the unit circle: start (-1,0), on-arc top (0,1),
    // end (1,0), axis angle 0, ratio 1 → rx = ry = 1, minor arc,
    // CLOCKWISE in the y-up frame (over the top, left → right),
    // which stays clockwise on the y-down page: sweep 1 (Visio 2021
    // witness t14 draws this arc above its chord).
    let (rx, ry, rot, large, sweep) =
        solve_ellipse_arc((-1.0, 0.0), (0.0, 1.0), (1.0, 0.0), 0.0, 1.0, (1.0, 1.0)).unwrap();
    assert!((rx - 1.0).abs() < 1e-9);
    assert!((ry - 1.0).abs() < 1e-9);
    assert!(rot.abs() < 1e-9);
    assert!(!large);
    assert_eq!(sweep, 1);
}

/// The junction-circle specimen from Analysis/06: MoveTo(0, W/2) then
/// two EllipticalArcTo rows (W = 0.0393700787…in) that must reconstruct
/// one shared circle of radius W/2 as two exact half-arcs.
#[test]
fn elliptical_arc_junction_circle() {
    let w = 0.03937007874015748;
    // Row 2: arc from (0, W/2) via on-arc point (W/2, W) to (W, W/2).
    let (rx, ry, _, large, sweep) = solve_ellipse_arc(
        (0.0, w / 2.0),
        (w / 2.0, w),
        (w, w / 2.0),
        0.0,
        1.0,
        (1.0, 1.0),
    )
    .unwrap();
    assert!(
        (rx - ry).abs() < 1e-9,
        "ratio 1 ⇒ circle, got rx={rx} ry={ry}"
    );
    assert!((rx - w / 2.0).abs() < 1e-9, "rx={rx} expected={}", w / 2.0);
    assert!(!large, "exact half arc");
    assert_eq!(sweep, 1, "over the top, left → right: clockwise");
    // Row 3 closes the circle back to (0, W/2) via (W/2, 0).
    let (rx2, ry2, _, large2, sweep2) = solve_ellipse_arc(
        (w, w / 2.0),
        (w / 2.0, 0.0),
        (0.0, w / 2.0),
        0.0,
        1.0,
        (1.0, 1.0),
    )
    .unwrap();
    assert!((rx2 - w / 2.0).abs() < 1e-9);
    assert!((ry2 - w / 2.0).abs() < 1e-9);
    assert!(!large2);
    assert_eq!(sweep2, 1, "under the bottom, right → left: clockwise");
}

#[test]
fn elliptical_arc_scaled_circle_becomes_ellipse() {
    // A unit circle scaled ×2 horizontally is an ellipse with rx=2.
    let (rx, ry, _, _, _) =
        solve_ellipse_arc((-1.0, 0.0), (0.0, 1.0), (1.0, 0.0), 0.0, 1.0, (2.0, 1.0)).unwrap();
    assert!((rx - 2.0).abs() < 1e-9, "rx={rx}");
    assert!((ry - 1.0).abs() < 1e-9, "ry={ry}");
}

#[test]
fn transform_ellipse_identity_and_rotation() {
    let (r1, r2, a) = transform_ellipse(2.0, 0.5, 0.3, (1.0, 1.0));
    assert!((r1 - 2.0).abs() < 1e-9 && (r2 - 1.0).abs() < 1e-9);
    assert!((a - 0.3).abs() < 1e-9);

    // Rotating then anisotropically scaling a circle: a circle has no
    // preferred axis, so scaling (2,1) gives rx=2, ry=1 regardless of
    // the incoming axis angle.
    let (r1, r2, a) = transform_ellipse(1.0, 1.0, std::f64::consts::FRAC_PI_2, (2.0, 1.0));
    assert!((r1 - 2.0).abs() < 1e-9 && (r2 - 1.0).abs() < 1e-9);
    assert!(a.abs() < 1e-9);
}

/// #12 specimen class: a container-heading shape whose master
/// Geometry section is gated by a section-level `NoShow` cell, and a
/// page instance carrying only sparse row caches (no section cells).
/// The merge must inherit the flag, and the formula must re-evaluate
/// per instance so a restyled instance shows its band again even
/// against a stale master cache.
#[test]
fn geometry_noshow_inherits_and_formula_reevaluates() {
    let urow = |n: &str, v: f64| Row {
        n: Some(n.into()),
        cells: vec![(
            "Value".into(),
            Cell {
                v: Some(format!("{v}")),
                ..Cell::default()
            },
        )],
        ..Row::default()
    };
    let grow = |ix: i64, t: &str, x: f64, y: f64| Row {
        ix,
        t: Some(t.into()),
        cells: vec![
            ("X".into(), Cell::value(format!("{x}"))),
            ("Y".into(), Cell::value(format!("{y}"))),
        ],
        ..Row::default()
    };
    // Heading master: band rect 2.0 × 0.5 with a style-gated NoShow.
    let heading_master = |cstyle: f64, noshow_f: Option<&str>| -> vsdx::Master {
        let mut sh = Shape {
            id: 8,
            ..Shape::default()
        };
        let set = |sheet: &mut Sheet, n: &str, v: f64| {
            sheet.set_cell(n, Cell::value(format!("{v}")));
        };
        set(&mut sh.sheet, "Width", 2.0);
        set(&mut sh.sheet, "Height", 0.5);
        sh.sheet.children.push(SheetChild::Section(Section {
            name: "User".into(),
            rows: vec![urow("hidden", 0.0), urow("CStyle", cstyle)],
            ..Section::default()
        }));
        sh.sheet.children.push(SheetChild::Section(Section {
            name: "Geometry".into(),
            ix: 0,
            cells: vec![
                ("NoFill".into(), Cell::value("0".to_string())),
                ("NoLine".into(), Cell::value("0".to_string())),
                (
                    "NoShow".into(),
                    Cell {
                        v: Some("1".into()),
                        f: noshow_f.map(str::to_string),
                        ..Cell::default()
                    },
                ),
            ],
            rows: vec![
                grow(1, "MoveTo", 0.0, 0.0),
                grow(2, "LineTo", 2.0, 0.0),
                grow(3, "LineTo", 2.0, 0.5),
                grow(4, "LineTo", 0.0, 0.5),
                grow(5, "LineTo", 0.0, 0.0),
            ],
            ..Section::default()
        }));
        vsdx::Master {
            id: 42,
            shapes: vec![sh],
            ..Default::default()
        }
    };
    // Sparse instance: only two row caches, no section-level cells —
    // exactly how Visio writes inline copies of master geometry.
    let inst = Shape {
        id: 1228,
        master: Some(42),
        master_shape: Some(8),
        ..Shape::default()
    };
    let mut inst = inst;
    inst.sheet.set_cell("Width", Cell::value("4.0"));
    inst.sheet.set_cell("Height", Cell::value("0.5"));
    inst.sheet.children.push(SheetChild::Section(Section {
        name: "Geometry".into(),
        ix: 0,
        rows: vec![grow(1, "MoveTo", 0.0, 0.0), grow(2, "LineTo", 4.0, 0.0)],
        ..Section::default()
    }));

    // Plain style (CStyle=1): the formula hides the band — the
    // utilisation.vsdx heading case.
    let doc = Document {
        masters: vec![heading_master(1.0, Some("OR(User.hidden,User.CStyle<>4)"))],
        ..Document::default()
    };
    let mut out = String::new();
    render_all_geometry(&doc, &inst, &[], 4.0, 0.5, (0.0, 0.0), &mut out);
    assert!(
        !out.contains("<path"),
        "NoShow'd section must not draw: {out}"
    );

    // Restyled (CStyle=4): the formula re-evaluates to visible even
    // though the master's cached NoShow V=1 says hidden.
    let doc = Document {
        masters: vec![heading_master(4.0, Some("OR(User.hidden,User.CStyle<>4)"))],
        ..Document::default()
    };
    let mut out = String::new();
    render_all_geometry(&doc, &inst, &[], 4.0, 0.5, (0.0, 0.0), &mut out);
    assert!(
        out.contains("<path"),
        "formula must beat the stale cache: {out}"
    );
    assert!(
        out.contains("4.00000"),
        "row merge must scale master X into instance units: {out}"
    );

    // Cache-only flag (no formula): the cached V=1 still hides.
    let doc = Document {
        masters: vec![heading_master(4.0, None)],
        ..Document::default()
    };
    let mut out = String::new();
    render_all_geometry(&doc, &inst, &[], 4.0, 0.5, (0.0, 0.0), &mut out);
    assert!(!out.contains("<path"), "cached NoShow must hide: {out}");
}

// ------------------------------------------------------------------
// Group XForm composition (GitHub #78): the affine helpers must
// reproduce the renderer's nested transform math exactly.

/// #293: a `Del="1"` geometry row stays deleted — an OWN row
/// carrying values (what 0.2.15's corrupted save wrote: the
/// diagonal into row 3) renders nothing, and the phantom jump the
/// row used to feed is gone (the #294 part-4 prediction).
#[test]
fn export_drops_deleted_geometry_rows() {
    // A vertical connector whose deleted row 3 carries the master's
    // hook values (the saved-by-0.2.15 corruption shape), crossed
    // by a horizontal one.
    let mut s = shape_at(5, (3.5, 2.3047), 0.25, 2.2031, 0.0);
    s.sheet.set_cell("ObjType", Cell::value("2"));
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    sec.rows = vec![
        Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: vec![("X".to_string(), Cell::value("0.125"))],
            ..Row::default()
        },
        Row {
            t: Some("LineTo".into()),
            ix: 2,
            cells: vec![
                ("X".to_string(), Cell::value("0.125")),
                ("Y".to_string(), Cell::value("2.2031")),
            ],
            ..Row::default()
        },
        Row {
            t: Some("LineTo".into()),
            ix: 3,
            attrs: vec![("Del".to_string(), "1".to_string())],
            cells: vec![
                ("X".to_string(), Cell::value("1")),
                ("Y".to_string(), Cell::value("-1")),
            ],
            ..Row::default()
        },
    ];
    s.sheet.children.push(SheetChild::Section(sec));
    // The A→B twin: a horizontal connector at page y = 2.297 that
    // BOTH the vertical run (x = 3.5) and the deleted hook (its
    // page chord runs (3.5, 3.4)→(4.375, 0.2)) would cross.
    let mut ab = shape_at(6, (3.0, 2.2969), 3.0, 0.0, 0.0);
    ab.sheet.set_cell("ObjType", Cell::value("2"));
    let mut asec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    asec.rows = vec![
        Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: vec![("X".to_string(), Cell::value("0"))],
            ..Row::default()
        },
        Row {
            t: Some("LineTo".into()),
            ix: 2,
            cells: vec![("X".to_string(), Cell::value("3"))],
            ..Row::default()
        },
    ];
    ab.sheet.children.push(SheetChild::Section(asec));
    let (doc, page) = page_fixture(vec![ab, s]);
    let svg = render_page(&doc, &page).unwrap();
    // The diagonal hook (local (0.125,2.2)→(1,−1) → page
    // (3.5,3.4)→(4.375,0.2)) draws nothing.
    assert!(
        !svg.contains("4.37500"),
        "the deleted row's diagonal is not drawn"
    );
    // The crossing hops once — the phantom jump the deleted row
    // fed is gone.
    assert_eq!(
        svg.matches(r##"stroke="#ffffff""##).count(),
        1,
        "one jump at the real crossing"
    );
}

/// #294 review: an AUTHORED shallow ArcTo on a connector is a
/// curve — the semicircle band leaves it alone, and with no
/// crossing nothing replaces it.
#[test]
fn export_preserves_authored_shallow_connector_arcs() {
    let mut s = shape_at(1, (3.0, 5.0), 4.0, 0.0, 0.0);
    s.sheet.set_cell("ObjType", Cell::value("2"));
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
        Row {
            t: Some("ArcTo".into()),
            ix: 2,
            cells: vec![
                ("X".to_string(), Cell::value("4")),
                ("Y".to_string(), Cell::value("0")),
                ("A".to_string(), Cell::value("0.25")),
            ],
            ..Row::default()
        },
    ];
    s.sheet.children.push(SheetChild::Section(sec));
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(svg.contains(" A "), "the shallow arc renders as an arc");
    assert!(
        !svg.contains(r##"data-dc="line-jumps""##),
        "no crossing, no overlay"
    );
}

/// #203: the golden fixture page — (1) a SplineStart+2×SplineKnot
/// cubic, (2) a NURBSTo with uniform weights, (3) a NURBSTo with
/// NON-uniform weights (the rational case the chord approximates
/// wrongly). Committed as an SVG golden; regenerate with
/// UPDATE_SPLINE_GOLDENS=1.
#[test]
fn spline_nurbs_goldens() {
    // 1. the spline: MoveTo(0,0), SplineStart (second control
    //    point) with knots 0/0.3/1 degree 3, knots at 0.6/1.
    let mut spline = shape_at(1, (2.0, 2.0), 4.0, 2.0, 0.0);
    push_geo(
        &mut spline,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(
                2,
                "SplineStart",
                &[
                    ("X", 1.0),
                    ("Y", 1.5),
                    ("A", 0.3),
                    ("B", 0.0),
                    ("C", 1.0),
                    ("D", 3.0),
                ],
            ),
            geo_row(3, "SplineKnot", &[("X", 2.5), ("Y", 0.4), ("A", 0.6)]),
            geo_row(4, "SplineKnot", &[("X", 4.0), ("Y", 2.0), ("A", 1.0)]),
        ],
    );
    // 2. NURBS uniform: M(0,0) → NURBSTo(4,2), interior points
    //    (1.2, 1.6) and (2.8, 0.2), weights 1.
    let mut nurbs_u = shape_at(2, (7.0, 2.0), 4.0, 2.0, 0.0);
    push_geo(
        &mut nurbs_u,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            nurbs_row(2, 4.0, 2.0, "NURBS(1, 3, 0, 0, 1.2,1.6,0,1, 2.8,0.2,0,1)"),
        ],
    );
    // 3. NURBS rational: the same geometry with the middle
    //    control weighted 2 — the belly pulls toward that point.
    let mut nurbs_r = shape_at(3, (12.0, 2.0), 4.0, 2.0, 0.0);
    push_geo(
        &mut nurbs_r,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            nurbs_row(2, 4.0, 2.0, "NURBS(1, 3, 0, 0, 1.2,1.6,0,2, 2.8,0.2,0,1)"),
        ],
    );
    let (doc, page) = page_fixture(vec![spline, nurbs_u, nurbs_r]);
    let svg = render_page(&doc, &page).unwrap();

    let regenerate = std::env::var("UPDATE_SPLINE_GOLDENS").is_ok();
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/spline_nurbs.svg");
    if regenerate {
        let _ = std::fs::create_dir_all(golden_path.parent().unwrap());
        std::fs::write(&golden_path, &svg).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path)
        .expect("the spline/NURBS golden exists (regenerate with UPDATE_SPLINE_GOLDENS=1)");
    // Git's autocrlf normalizes text files to CRLF on Windows
    // checkouts; the renderer emits LF. Compare newline-normalized.
    let norm = |s: &str| s.replace('\r', "");
    assert_eq!(
        norm(&svg).trim(),
        norm(&golden).trim(),
            "the spline/NURBS golden drifted — if the change is intended, \
         regenerate with UPDATE_SPLINE_GOLDENS=1 and review the diff"
    );
    // Each curve emits a multi-point polyline well above the
    // chord: count `L` segments (16 per knot span, >8 per curve).
    let segments = svg.match_indices("L ").count();
    assert!(
        segments > 24,
        "three flattened curves produce many segments: {segments}"
    );
}

/// #188: SplineStart + SplineKnot rows render the EVALUATED
/// B-spline — a multi-segment path that leaves the control-point
/// chord — not the v0.2 single chord.
#[test]
fn spline_rows_render_the_evaluated_curve() {
    let mut s = shape_at(1, (2.0, 2.0), 4.0, 2.0, 0.0);
    // P0=(0,0); P1=SplineStart (1,1); P2/P3=SplineKnots; cubic.
    push_geo(
        &mut s,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(
                2,
                "SplineStart",
                &[
                    ("X", 1.0),
                    ("Y", 1.5),
                    ("A", 0.4),
                    ("B", 0.0),
                    ("C", 1.0),
                    ("D", 3.0),
                ],
            ),
            geo_row(3, "SplineKnot", &[("X", 2.0), ("Y", 0.5), ("A", 0.6)]),
            geo_row(4, "SplineKnot", &[("X", 4.0), ("Y", 2.0), ("A", 1.0)]),
        ],
    );
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    let path = svg.match_indices("L ").count();
    assert!(
        path > 8,
        "the spline flattens to many segments, not a chord: {} L segments",
        path
    );
    // The ink leaves the P0→P3 chord (the S bulge through P1/P2).
    // The chord from (0,0) to (4,2): at x=2 the chord y is 1; the
    // spline's belly passes well below (through ~(2, 0.9±)) — assert
    // some emitted coordinate off the chord line instead of exact.
    let off = svg
        .match_indices("L ")
        .map(|(i, _)| &svg[i + 2..i + 30])
        .filter_map(|seg| {
            let mut it = seg.split_whitespace();
            let x: f64 = it.next()?.parse().ok()?;
            let y: f64 = it.next()?.parse().ok()?;
            Some((x, y))
        })
        .find(|(x, y)| (0.5 * x + 2.0 - y).abs() > 0.05);
    assert!(off.is_some(), "some flattened point leaves the chord");
}

/// #188: the corpus NURBSTo row (E = NURBS(...) with the row-cell
/// frame) renders off its chord — the v0.2 single chord is gone.
#[test]
fn nurbs_to_row_renders_from_the_e_formula() {
    let mut s = shape_at(1, (2.0, 2.0), 4.0, 2.0, 0.0);
    let e = "NURBS(1, 3, 0, 0, 0.35,0.9,0,1, 0.65,0.1,0,1)";
    push_geo(
        &mut s,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(
                2,
                "NURBSTo",
                &[
                    ("X", 4.0),
                    ("Y", 2.0),
                    ("A", 0.0),
                    ("B", 1.0),
                    ("C", 0.0),
                    ("D", 1.0),
                ],
            ),
        ],
    );
    // The E cell with its formula text.
    {
        let sec = s
            .sheet
            .children
            .iter_mut()
            .find_map(|c| match c {
                SheetChild::Section(sec) if sec.name == "Geometry" => Some(sec),
                _ => None,
            })
            .unwrap();
        let r2 = sec.rows.iter_mut().find(|r| r.ix == 2).unwrap();
        r2.cells.push((
            "E".to_string(),
            Cell {
                v: Some(e.to_string()),
                f: Some(e.to_string()),
                ..Cell::default()
            },
        ));
    }
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    let segments = svg.match_indices("L ").count();
    assert!(
        segments > 8,
        "the NURBS flattens to many segments: {segments}"
    );
    // Interior points (0.35·4=1.4, 0.9·2=1.8) and (2.6, 0.2) pull
    // the curve off the (0,0)→(4,2) chord; some emitted point must
    // sit off it (chord: y = 0.5x).
    let off = svg
        .match_indices("L ")
        .map(|(i, _)| &svg[i + 2..i + 30])
        .filter_map(|seg| {
            let mut it = seg.split_whitespace();
            let x: f64 = it.next()?.parse().ok()?;
            let y: f64 = it.next()?.parse().ok()?;
            Some((x, y))
        })
        .find(|(x, y)| (0.5 * x + 2.0 - y).abs() > 0.05);
    assert!(off.is_some(), "some flattened point leaves the chord");
}

#[test]
fn layer_svg_scopes_the_viewbox_and_skips_hidden() {
    let (doc, page) = page_fixture(vec![shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0)]);
    let bounds = (1.0, 2.0, 3.0, 4.0);
    let svg = render_shape_layer(&doc, &page, 1, &[], bounds).unwrap();
    assert!(
        svg.contains(r#"viewBox="1.00000 2.00000 3.00000 4.00000""#),
        "viewBox must be the given bounds: {svg}"
    );
    assert!(svg.contains("<g transform"), "the shape is drawn");
    // A hidden root emits the (empty) svg only — same skip the
    // page renderer applies to hidden members.
    let mut hidden_shape = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    hidden_shape.sheet.set_cell("LayerMember", Cell::value("2"));
    let (doc, page) = page_fixture(vec![hidden_shape]);
    let svg = render_shape_layer(&doc, &page, 1, &[2], bounds).unwrap();
    assert!(
        !svg.contains("<g transform"),
        "hidden root must not draw: {svg}"
    );
}

/// #236: the culled patch serialization omits every subtree node
/// whose ink box misses the (padded) cull rect and keeps the ones
/// it covers — the serialize+parse saving is real, and a
/// regression to full serialization would re-include the far
/// shape's ink. The group node itself always stays (its subtree
/// box spans both leaves).
#[test]
fn culled_serialization_omits_shapes_outside_the_cull_rect() {
    // A group with two disjoint leaves: red left, green right.
    // Real Geometry rows, so the leaves actually paint.
    let rect_rows = |w: f64, h: f64| {
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "LineTo", &[("X", w), ("Y", 0.0)]),
            geo_row(3, "LineTo", &[("X", w), ("Y", h)]),
            geo_row(4, "LineTo", &[("X", 0.0), ("Y", h)]),
            geo_row(5, "LineTo", &[("X", 0.0), ("Y", 0.0)]),
        ]
    };
    let mut left = shape_at(2, (0.5, 0.5), 0.6, 0.4, 0.0);
    left.sheet.set_cell("FillForegnd", Cell::value("#ff2200"));
    push_geo(&mut left, rect_rows(0.6, 0.4));
    let mut right = shape_at(3, (2.5, 0.5), 0.6, 0.4, 0.0);
    right.sheet.set_cell("FillForegnd", Cell::value("#22cc00"));
    push_geo(&mut right, rect_rows(0.6, 0.4));
    let mut group = shape_at(1, (3.0, 5.0), 3.0, 1.0, 0.0);
    group.sub_shapes = vec![left, right];
    let (doc, page) = page_fixture(vec![group]);
    let bounds = shape_ink_bounds(&doc, &page, 1).unwrap();
    // The cull rect covers the LEFT leaf's x range only (the
    // leaves share their y range, so x does the separating).
    let cull = (bounds.0, bounds.1, 1.4, bounds.3);
    let (_, culled) =
        render_shape_layer_opts_culled(&doc, &page, 1, &[], bounds, None, None, cull, None)
            .unwrap();
    assert!(
        culled.contains("#ff2200"),
        "the covered leaf stays in the patch: {culled}"
    );
    assert!(
        !culled.contains("#22cc00"),
        "the far leaf is omitted from the patch serialization: {culled}"
    );
    // The full serialization keeps both.
    let (_, full) =
        render_shape_layer_opts(&doc, &page, 1, &[], bounds, None, None, None).unwrap();
    assert!(full.contains("#ff2200") && full.contains("#22cc00"));
}

/// 1-D shapes (BeginX/EndX) fill their CLOSED sections — the
/// block-arrow masters (Visio 2021 witness, t04) — while an open
/// path keeps ignoring a decorative FillPattern.
#[test]
fn one_d_shape_fills_closed_geometry_only() {
    let render = |closed: bool| {
        let mut s = shape_at(1, (2.0, 2.0), 3.0, 1.0, 0.0);
        for (n, v) in [
            ("BeginX", "0.5"),
            ("BeginY", "2"),
            ("EndX", "3.5"),
            ("EndY", "2"),
        ] {
            s.sheet.set_cell(n, Cell::value(v));
        }
        s.sheet.set_cell("FillForegnd", Cell::value("#ed7d31"));
        s.sheet.set_cell("FillPattern", Cell::value("1"));
        let mut rows = vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.25)]),
            geo_row(2, "LineTo", &[("X", 2.5), ("Y", 0.25)]),
            geo_row(3, "LineTo", &[("X", 3.0), ("Y", 0.5)]),
            geo_row(4, "LineTo", &[("X", 2.5), ("Y", 0.75)]),
            geo_row(5, "LineTo", &[("X", 0.0), ("Y", 0.75)]),
        ];
        if closed {
            rows.push(geo_row(6, "LineTo", &[("X", 0.0), ("Y", 0.25)]));
        }
        push_geo(&mut s, rows);
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    assert!(render(true).contains(r##"fill="#ed7d31""##));
    assert!(!render(false).contains("#ed7d31"));
}

/// A closed curve followed by a second closed outline in the same
/// section still fills: the curve row ends without a space, so the
/// next `MoveTo` arrives glued to its last number (Greptile on PR
/// #328).
#[test]
fn one_d_shape_fills_a_closed_curve_before_another_outline() {
    let mut s = shape_at(1, (2.0, 2.0), 3.0, 1.0, 0.0);
    for (n, v) in [
        ("BeginX", "0.5"),
        ("BeginY", "2"),
        ("EndX", "3.5"),
        ("EndY", "2"),
    ] {
        s.sheet.set_cell(n, Cell::value(v));
    }
    s.sheet.set_cell("FillForegnd", Cell::value("#ed7d31"));
    s.sheet.set_cell("FillPattern", Cell::value("1"));
    push_geo(
        &mut s,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.5)]),
            geo_row(
                2,
                "QuadBezTo",
                &[("X", 1.0), ("Y", 0.5), ("A", 0.5), ("B", 1.0)],
            ),
            geo_row(
                3,
                "QuadBezTo",
                &[("X", 0.0), ("Y", 0.5), ("A", 0.5), ("B", 0.0)],
            ),
            geo_row(4, "MoveTo", &[("X", 2.0), ("Y", 0.25)]),
            geo_row(5, "LineTo", &[("X", 3.0), ("Y", 0.25)]),
            geo_row(6, "LineTo", &[("X", 3.0), ("Y", 0.75)]),
            geo_row(7, "LineTo", &[("X", 2.0), ("Y", 0.25)]),
        ],
    );
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(svg.contains(r##"fill="#ed7d31""##), "{svg}");
}

/// Closure is per subpath (Greptile on PR #328): two closed
/// outlines with different starts both fill, an open subpath next
/// to a closed one stays out of the fill, and a closed curve made
/// of a single row fills too.
#[test]
fn closed_subpaths_are_judged_one_by_one() {
    let two = "M 0 0 L 1 0 L 1 1 L 0 0 M 5 5 L 6 5 L 6 6 L 5 5 ";
    assert_eq!(closed_subpaths(two).split_whitespace().count(), 24);
    let mixed = "M 0 0 L 1 0 L 1 1 M 5 5 L 6 5 L 6 6 L 5 5 ";
    let f = closed_subpaths(mixed);
    assert!(f.starts_with("M 5 5") && !f.contains("M 0 0"), "{f}");
    let curve = "M 0 0 C 1 0 0 1 0 0 ";
    assert_eq!(closed_subpaths(curve).trim(), curve.trim());
    assert_eq!(closed_subpaths("M 0 0 L 1 1 Z").trim(), "M 0 0 L 1 1 Z");
    assert!(closed_subpaths("M 0 0 L 1 0").is_empty());
    assert!(closed_subpaths("M 0 0").is_empty());
    // Curve rows end without a space, so the next `M` arrives
    // glued to the last number; both outlines still close.
    let glued = "M 0 0 C 1 0 0 1 0 0M 5 5 L 6 5 L 6 6 L 5 5 ";
    let f = closed_subpaths(glued);
    assert!(f.contains("M 0 0") && f.contains("M 5 5"), "{f}");
    let open_after_curve = "M 0 0 C 1 0 0 1 0 0M 5 5 L 0 0";
    let f = closed_subpaths(open_after_curve);
    assert!(f.contains("M 0 0") && !f.contains("M 5 5"), "{f}");
}

/// The geometry frame keeps the sign of a negative instance size:
/// master width 2, instance width −4 → scale −2 → frame 2, so a
/// fractional vertex at 0.5 lands at −2 after `emit_pt`'s scale,
/// not at 1e12 (Greptile on PR #329).
#[test]
fn frame_dim_keeps_negative_scales() {
    assert!((frame_dim(-4.0, -2.0) - 2.0).abs() < 1e-12);
    assert!((0.5 * frame_dim(-4.0, -2.0) * -2.0 - -2.0).abs() < 1e-12);
    assert!((frame_dim(3.0, 1.5) - 2.0).abs() < 1e-12);
    assert_eq!(frame_dim(3.0, 0.0), 3.0, "a zero scale keeps the dim");
}

/// An instance PolylineTo row without its own A cell takes the
/// master's POLYLINE() text; absolute vertices scale master →
/// instance like X/Y (CodeRabbit on PR #329).
#[test]
fn polyline_to_inherits_the_master_polyline_text() {
    let poly_row = |x: f64, a: Option<&str>| {
        let mut r = geo_row(2, "PolylineTo", &[("X", x), ("Y", 0.0)]);
        if let Some(a) = a {
            r.cells.push((
                "A".into(),
                Cell {
                    v: Some(a.into()),
                    ..Cell::default()
                },
            ));
        }
        r
    };
    let mut master = shape_at(1, (1.0, 0.5), 2.0, 1.0, 0.0);
    push_geo(
        &mut master,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            poly_row(2.0, Some("POLYLINE(1, 1, 0,1, 2,1)")),
        ],
    );
    let mut inst = shape_at(1, (4.0, 5.0), 4.0, 1.0, 0.0);
    inst.master = Some(7);
    push_geo(
        &mut inst,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            poly_row(4.0, None),
        ],
    );
    let (mut doc, page) = page_fixture(vec![inst]);
    doc.masters.push(vsdx::model::Master {
        id: 7,
        shapes: vec![master],
        ..vsdx::model::Master::default()
    });
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        svg.contains("L 0.00000 -1.00000 L 4.00000 -1.00000 L 4.00000 -0.00000"),
        "the master's vertices, stretched to the instance width: {svg}"
    );
}

/// An instance that turns the master's PolylineTo row into an ArcTo
/// keeps its own bend; the master's POLYLINE() text is not pushed
/// into it (Greptile on PR #329).
#[test]
fn an_instance_arc_keeps_its_bend_over_a_master_polyline() {
    let arc = geo_row(2, "ArcTo", &[("X", 4.0), ("Y", 0.0), ("A", 0.5)]);
    let (doc, page) = polyline_master_and_instance(arc, "POLYLINE(1, 1, 0,1, 2,1)");
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        !svg.contains("L 0.00000 -1.00000 L 4.00000 -1.00000"),
        "no master vertices on the arc: {svg}"
    );
    assert!(svg.contains(" A "), "the arc keeps its bend: {svg}");
}

/// A last vertex on the endpoint leaves no zero-length segment, so
/// the end tangent and the arrowheads survive (CodeRabbit on PR
/// #329).
#[test]
fn polyline_to_skips_a_vertex_on_its_endpoint() {
    // One copy of the endpoint, and several trailing copies
    // (Greptile on PR #329).
    for text in [
        "POLYLINE(1, 1, 1,0, 1,1)",
        "POLYLINE(1, 1, 1,0, 1,1, 1,1, 1,1)",
    ] {
        let mut s = shape_at(1, (2.0, 2.0), 1.0, 1.0, 0.0);
        let mut poly = geo_row(2, "PolylineTo", &[("X", 1.0), ("Y", 1.0)]);
        poly.cells.push((
            "A".into(),
            Cell {
                v: Some(text.into()),
                ..Cell::default()
            },
        ));
        push_geo(
            &mut s,
            vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), poly],
        );
        s.sheet.set_cell("EndArrow", Cell::value("2"));
        let (doc, page) = page_fixture(vec![s]);
        let svg = render_page(&doc, &page).unwrap();
        assert!(
            svg.contains("dc-ah"),
            "{text}: the arrowhead survives: {svg}"
        );
        assert!(
            !svg.contains("L 1.00000 -1.00000 L 1.00000 -1.00000"),
            "{text}: no repeated endpoint: {svg}"
        );
    }
}

/// PolylineTo walks its POLYLINE() vertices (Visio 2021 witness,
/// t05): fractions of Width/Height for type 0, absolute local
/// coordinates for type 1, then the row's X/Y endpoint — the
/// stencils' router glyphs used to collapse to a single chord.
#[test]
fn polyline_to_draws_its_vertices() {
    let mut s = shape_at(1, (2.0, 2.0), 2.0, 1.0, 0.0);
    let mut poly = geo_row(2, "PolylineTo", &[("X", 0.0), ("Y", 0.0)]);
    poly.cells.push((
        "A".into(),
        Cell {
            v: Some("POLYLINE(0, 0, 1,0, 1,1)".into()),
            ..Cell::default()
        },
    ));
    push_geo(
        &mut s,
        vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), poly],
    );
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    // (1,0)·(2,1) → (2,0) and (1,1)·(2,1) → (2,1) (y flipped).
    assert!(
        svg.contains("L 2.00000 -0.00000 L 2.00000 -1.00000 L 0.00000 -0.00000"),
        "{svg}"
    );
}

/// Fill/line transparency (Visio 2021 witness, t01): the *Trans
/// cells are fractions; 0.5 paints at half alpha, 1 paints
/// nothing at all (the stencils' invisible helper boxes), and
/// opaque paint carries no opacity attribute.
#[test]
fn fill_and_line_transparency() {
    let render = |cells: &[(&str, &str)]| {
        let mut s = rect_shape(1, (2.0, 2.0));
        s.sheet.set_cell("FillForegnd", Cell::value("#ff0000"));
        s.sheet.set_cell("LineColor", Cell::value("#00ff00"));
        for (n, v) in cells {
            s.sheet.set_cell(n, Cell::value(*v));
        }
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    let opaque = render(&[]);
    assert!(opaque.contains("#ff0000") && !opaque.contains("opacity="));
    let half = render(&[("FillForegndTrans", "0.5"), ("LineColorTrans", "0.25")]);
    assert!(
        half.contains(r##"fill="#ff0000" fill-opacity="0.500""##),
        "{half}"
    );
    assert!(half.contains(r#"stroke-opacity="0.750""#), "{half}");
    let gone = render(&[("FillForegndTrans", "1"), ("LineColorTrans", "1")]);
    assert!(
        !gone.contains("#ff0000") && !gone.contains("#00ff00"),
        "100 % transparent paint is dropped: {gone}"
    );
    // A hatch pattern's dominant background color fades by
    // FillBkgndTrans, not FillForegndTrans.
    let bk = render(&[
        ("FillPattern", "2"),
        ("FillBkgnd", "#0000ff"),
        ("FillBkgndTrans", "0.5"),
    ]);
    assert!(bk.contains(r#"fill-opacity="0.500""#), "{bk}");
}

/// LineColorTrans also applies to the arrowheads: a half
/// transparent line fades them, a fully transparent one hides them
/// with the shaft (Greptile on PR #327).
#[test]
fn line_transparency_fades_and_hides_arrowheads() {
    let render = |trans: &str| {
        let mut s = shape_at(1, (2.0, 2.0), 1.0, 0.0, 0.0);
        push_geo(
            &mut s,
            vec![
                geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                geo_row(2, "LineTo", &[("X", 1.0), ("Y", 0.0)]),
            ],
        );
        s.sheet.set_cell("EndArrow", Cell::value("2"));
        s.sheet.set_cell("LineColor", Cell::value("#00ff00"));
        s.sheet.set_cell("LineColorTrans", Cell::value(trans));
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    let opaque = render("0");
    assert!(
        opaque.contains("dc-ah"),
        "an opaque line draws its head: {opaque}"
    );
    let half = render("0.5");
    assert!(
        half.contains("dc-ah")
            && half.contains(r#"<g fill-opacity="0.500" stroke-opacity="0.500">"#),
        "a half transparent line fades its head: {half}"
    );
    let gone = render("1");
    assert!(
        !gone.contains("dc-ah") && !gone.contains("#00ff00"),
        "a fully transparent line hides its head: {gone}"
    );
}

/// Visio 2013+ gradients (Visio 2021 witnesses t08–t10): the
/// FillGradient stop rows drive the paint; Dir 0 runs along the
/// angle (0° left → right), Dir 1–7 are radial from a box anchor,
/// Dir 8–13 rectangular tiles clipped to the outline, and
/// LineGradientEnabled paints the stroke.
#[test]
fn gradient_fill_directions_and_line_gradient() {
    let stops = |name: &str| {
        let mut sec = Section {
            name: name.into(),
            ..Section::default()
        };
        for (ix, (c, p)) in [("#ff0000", "0"), ("#0000ff", "1")].into_iter().enumerate() {
            sec.rows.push(Row {
                ix: ix as i64,
                cells: vec![
                    ("GradientStopColor".into(), Cell::value(c)),
                    ("GradientStopPosition".into(), Cell::value(p)),
                ],
                ..Row::default()
            });
        }
        SheetChild::Section(sec)
    };
    let render = |cells: &[(&str, &str)], line: bool| {
        let mut s = rect_shape(1, (2.0, 2.0));
        s.sheet
            .children
            .push(stops(if line { "LineGradient" } else { "FillGradient" }));
        for (n, v) in cells {
            s.sheet.set_cell(n, Cell::value(*v));
        }
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    let lin = render(
        &[("FillGradientEnabled", "1"), ("FillGradientAngle", "0")],
        false,
    );
    assert!(
        lin.contains(r#"x1="0.00000" y1="-0.50000" x2="1.00000" y2="-0.50000""#),
        "0° runs left → right across the box: {lin}"
    );
    assert!(lin.contains(r##"stop-color="#ff0000""##) && lin.contains("url(#dc-grad"));
    let rad = render(
        &[("FillGradientEnabled", "1"), ("FillGradientDir", "3")],
        false,
    );
    assert!(
        rad.contains(r#"<radialGradient id="#) && rad.contains(r#"cx="0.50000" cy="-0.50000""#),
        "{rad}"
    );
    let rect = render(
        &[("FillGradientEnabled", "1"), ("FillGradientDir", "10")],
        false,
    );
    assert!(rect.contains("<clipPath") && rect.matches("<linearGradient").count() == 4);
    // Disabled: the plain FillForegnd paint, no gradient.
    assert!(!render(&[("FillGradientEnabled", "0")], false).contains("Gradient id"));
    // A right-to-left connector (negative Width) keeps the
    // left → right sense of a 0° gradient.
    let mut neg = rect_shape(1, (2.0, 2.0));
    neg.sheet.set_cell("Width", Cell::value("-1"));
    neg.sheet.set_cell("FillGradientEnabled", Cell::value("1"));
    neg.sheet.set_cell("FillGradientAngle", Cell::value("0"));
    neg.sheet.children.push(stops("FillGradient"));
    let (doc, page) = page_fixture(vec![neg]);
    let neg = render_page(&doc, &page).unwrap();
    assert!(
        neg.contains(r#"x1="-1.00000" y1="-0.50000" x2="0.00000""#),
        "{neg}"
    );
    let stroke = render(&[("LineGradientEnabled", "1")], true);
    assert!(stroke.contains(r#"stroke="url(#dc-grad"#), "{stroke}");
}

/// Visio eases each gradient segment with smoothstep and mixes in
/// linear light (Visio 2021 witness: its own PNG export of linear and
/// radial ramps fits this to 0.01 RMS). Black → white: the halfway
/// sub-stop is the linear-light midpoint #bcbcbc, not sRGB #808080,
/// and the quarter stop sits low on the eased curve.
#[test]
fn gradient_stops_ease_in_linear_light() {
    let svg = stop_elems(&[("#000000".into(), 1.0, 0.0), ("#ffffff".into(), 1.0, 1.0)]);
    assert_eq!(svg.matches("<stop ").count(), EASE_STEPS + 1, "{svg}");
    assert!(
        svg.contains(r##"<stop offset="0.5000" stop-color="#bcbcbc"/>"##),
        "{svg}"
    );
    // u = 0.25 → smoothstep 0.15625 → linear 0.156 → sRGB #6e6e6e.
    assert!(
        svg.contains(r##"<stop offset="0.2500" stop-color="#6e6e6e"/>"##),
        "{svg}"
    );
    // Non-hex colors keep a plain two-stop ramp.
    let named = stop_elems(&[("red".into(), 1.0, 0.0), ("blue".into(), 1.0, 1.0)]);
    assert_eq!(named.matches("<stop ").count(), 2, "{named}");
}

/// Legacy fill patterns (Visio 2021 witness t12): 2–7 draw 1/96 in
/// lines at a 1/12 in period in FillForegnd over FillBkgnd, clipped
/// to the outline; dither patterns mix the two colors.
#[test]
fn hatch_patterns_draw_lines_over_the_background() {
    let render = |pat: &str| {
        let mut s = rect_shape(1, (2.0, 2.0));
        s.sheet.set_cell("FillForegnd", Cell::value("#0000ff"));
        s.sheet.set_cell("FillBkgnd", Cell::value("#ffff00"));
        s.sheet.set_cell("FillPattern", Cell::value(pat));
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    let horiz = render("6");
    assert!(
        horiz.contains("<clipPath")
            && horiz.contains(r##"stroke="#0000ff" stroke-width="0.01042""##),
        "{horiz}"
    );
    assert!(horiz.contains(r##"fill="#ffff00""##));
    let dither = render("9");
    assert!(dither.contains(r##"fill="#808080""##), "{dither}");
}

/// An Ellipse row joins the shape's even-odd fill (Visio 2021
/// witness, t07): a rectangle section plus an ellipse section is a
/// square with a round hole — one fill path carrying both contours,
/// no separately filled <ellipse> on top.
#[test]
fn ellipse_row_joins_the_even_odd_fill() {
    let mut s = rect_shape(1, (2.0, 2.0));
    s.sheet.set_cell("FillForegnd", Cell::value("#ff0000"));
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 1,
        ..Section::default()
    };
    sec.rows.push(geo_row(
        1,
        "Ellipse",
        &[
            ("X", 0.5),
            ("Y", 0.5),
            ("A", 0.8),
            ("B", 0.5),
            ("C", 0.5),
            ("D", 0.8),
        ],
    ));
    s.sheet.children.push(SheetChild::Section(sec));
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(!svg.contains("<ellipse"), "{svg}");
    let fill = svg
        .lines()
        .find(|l| l.contains(r#"fill-rule="evenodd""#))
        .expect("one even-odd fill path");
    assert_eq!(
        fill.matches('M').count(),
        2,
        "rect + ellipse contours: {fill}"
    );
    assert_eq!(fill.matches(" A ").count(), 2, "two half arcs: {fill}");
}

/// Rounding (Visio 2021 witness, t06): every corner of a straight
/// walk becomes a tangent arc of the cell's radius, clamped to half
/// the shorter leg; curved paths keep their authored shape.
#[test]
fn rounding_rounds_polyline_corners() {
    let sq = "M 0 0 L 2 0 L 2 -1 L 0 -1 L 0 0 ";
    let r = round_corners(sq, 0.25).expect("straight walk rounds");
    assert_eq!(r.matches(" A 0.25000 0.25000").count(), 4, "{r}");
    assert!(
        r.starts_with("M 0.25000 0.00000") && r.ends_with("Z "),
        "{r}"
    );
    // A radius larger than half the short leg clamps to it.
    let big = round_corners(sq, 5.0).unwrap();
    assert!(big.contains(" A 0.50000 0.50000"), "{big}");
    // An open walk keeps its endpoints and rounds the interior.
    let open = round_corners("M 0 0 L 1 0 L 1 -1 ", 0.25).unwrap();
    assert!(open.starts_with("M 0.00000 0.00000") && open.ends_with("L 1.00000 -1.00000 "));
    assert_eq!(open.matches(" A ").count(), 1);
    assert!(round_corners("M 0 0 C 1 1 2 2 3 3 ", 0.25).is_none());
    // A pill drawn as a flat end plus a shallow arc end (the network
    // LLD's BVI labels): Rounding above half the height rounds both
    // ends fully — the arc's bulge is shallower than the radius, so
    // it rounds through as its chord.
    // Radii too small for the chord scale up uniformly (SVG): this
    // arc is half of a 0.49 × 0.05 ellipse, far deeper than the
    // radius, so it stays (CodeRabbit on PR #354).
    let wide = "M 0 0 L 0.77570 0 A 0.39349 0.03937 0 0 1 0.77570 -0.09843 L 0 -0.09843 L 0 0 ";
    let w = round_corners(wide, 0.15748).expect("the straight corners round");
    assert!(
        w.contains("A 0.39349 0.03937 0 0 1 0.77570 -0.09843"),
        "{w}"
    );
    assert_eq!(w.matches(" A 0.04921 0.04921").count(), 2, "{w}");
    // As the renderer writes it: the arc row ends without a space,
    // so the next `L` arrives glued to its last number.
    let glued = "M 0.00000 -0.00000 L 0.77570 -0.00000 A 0.08228 0.05895 -90.0000 0 0 0.77570 -0.09843L 0.00000 -0.09843 L 0.00000 -0.00000 Z";
    let g = round_corners(glued, 0.15748).expect("the glued pill rounds");
    assert_eq!(g.matches(" A 0.04921 0.04921").count(), 4, "{g}");
    // A deep arc stays a curve: its corners keep their shape and
    // only the straight corners round.
    let deep = "M 0 0 L 2 0 A 1 1 0 0 1 2 -2 L 0 -2 L 0 0 ";
    let d = round_corners(deep, 0.25).expect("straight corners round");
    assert!(
        d.contains("A 1.00000 1.00000 0 0 1 2.00000 -2.00000"),
        "{d}"
    );
    assert_eq!(d.matches(" A 0.25000 0.25000").count(), 2, "{d}");
    // A shallow arc whose ends round no corner keeps its shape:
    // only the right angle before it rounds (CodeRabbit on PR #354).
    let tail = round_corners("M 1 0 L 1 -2 L 2 -2 A 1 0.25 0 0 1 3 -2 ", 0.1)
        .expect("the right angle rounds");
    assert!(
        tail.ends_with("A 1.00000 0.25000 0 0 1 3.00000 -2.00000 "),
        "{tail}"
    );
    assert_eq!(tail.matches(" A 0.10000 0.10000").count(), 1, "{tail}");
    // Through the renderer: the cell reaches the stroked outline.
    let mut s = rect_shape(1, (2.0, 2.0));
    s.sheet.set_cell("Rounding", Cell::value("0.125"));
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert_eq!(
        svg.matches(" A 0.12500 0.12500").count(),
        8,
        "fill + stroke"
    );
}

/// A spline samples into `L` segments but is a curve: a positive
/// `Rounding` leaves it as drawn (Greptile on PR #344).
#[test]
fn rounding_leaves_sampled_splines_alone() {
    let render = |rounding: Option<&str>| {
        let mut s = shape_at(1, (2.0, 2.0), 4.0, 2.0, 0.0);
        push_geo(
            &mut s,
            vec![
                geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                geo_row(
                    2,
                    "SplineStart",
                    &[
                        ("X", 1.0),
                        ("Y", 1.5),
                        ("A", 0.3),
                        ("B", 0.0),
                        ("C", 1.0),
                        ("D", 3.0),
                    ],
                ),
                geo_row(3, "SplineKnot", &[("X", 2.5), ("Y", 0.4), ("A", 0.6)]),
                geo_row(4, "SplineKnot", &[("X", 4.0), ("Y", 2.0), ("A", 1.0)]),
            ],
        );
        if let Some(r) = rounding {
            s.sheet.set_cell("Rounding", Cell::value(r));
        }
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    assert_eq!(render(Some("0.125")), render(None));
}

/// A formula-only `Rounding` on an instance of a resized master
/// evaluates with the INSTANCE's size: `Width/10` on a 4 in wide
/// instance is 0.4, not the 1 in master's 0.1 (Greptile on PR #344).
#[test]
fn rounding_formulas_use_the_instance_size() {
    // A formula-only cell on the instance, and the master's cell with
    // the master's cache `V='0.1'` (Greptile on PR #344, twice): both
    // evaluate with the instance's size.
    for on_master in [false, true] {
        let mut master = rect_shape(1, (1.0, 1.0));
        master.sheet.set_cell("Width", Cell::value("1"));
        master.sheet.set_cell("Height", Cell::value("1"));
        let mut inst = shape_at(1, (4.0, 5.0), 4.0, 4.0, 0.0);
        inst.master = Some(7);
        let cell = Cell {
            v: on_master.then(|| "0.1".to_string()),
            f: Some("Width/10".into()),
            ..Cell::default()
        };
        if on_master {
            master.sheet.set_cell("Rounding", cell);
        } else {
            inst.sheet.set_cell("Rounding", cell);
        }
        let (mut doc, page) = page_fixture(vec![inst]);
        doc.masters.push(vsdx::model::Master {
            id: 7,
            shapes: vec![master],
            ..vsdx::model::Master::default()
        });
        let svg = render_page(&doc, &page).unwrap();
        assert!(
            svg.contains(" A 0.40000 0.40000"),
            "on_master={on_master}: {svg}"
        );
        assert!(
            !svg.contains(" A 0.10000 0.10000"),
            "on_master={on_master}: {svg}"
        );
    }
}

/// A deleted spline row draws nothing and does not block the
/// rounding of the straight walk that remains (Greptile on PR #344).
#[test]
fn deleted_curve_rows_do_not_block_rounding() {
    let mut s = rect_shape(1, (2.0, 2.0));
    s.sheet.set_cell("Rounding", Cell::value("0.125"));
    let mut gone = geo_row(6, "SplineKnot", &[("X", 0.5), ("Y", 0.5), ("A", 1.0)]);
    gone.attrs.push(("Del".into(), "1".into()));
    if let Some(SheetChild::Section(sec)) = s
        .sheet
        .children
        .iter_mut()
        .find(|c| matches!(c, SheetChild::Section(sec) if sec.name == "Geometry"))
    {
        sec.rows.push(gone);
    }
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(svg.contains(" A 0.12500 0.12500"), "{svg}");
}

/// A master size formula the instance cannot evaluate keeps the
/// master's cached value, whether the instance omits the whole row
/// or only the cell (Greptile on PR #343).
#[test]
fn unevaluable_master_formulas_stay_unscaled_on_both_paths() {
    let render = |omit_row: bool| {
        let mut x = geo_row(2, "LineTo", &[("Y", 0.0)]);
        x.cells.push((
            "X".into(),
            Cell {
                v: Some("1.8".into()),
                f: Some("Width-User.Missing".into()),
                ..Cell::default()
            },
        ));
        let mut master = shape_at(1, (1.0, 0.5), 2.0, 1.0, 0.0);
        push_geo(
            &mut master,
            vec![
                geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                x,
                geo_row(3, "LineTo", &[("X", 2.0), ("Y", 1.0)]),
            ],
        );
        let mut inst = shape_at(1, (4.0, 5.0), 4.0, 2.0, 0.0);
        inst.master = Some(7);
        let mut rows = vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)])];
        if !omit_row {
            rows.push(geo_row(2, "LineTo", &[("Y", 0.0)]));
        }
        rows.push(geo_row(3, "LineTo", &[("X", 4.0), ("Y", 2.0)]));
        push_geo(&mut inst, rows);
        let (mut doc, page) = page_fixture(vec![inst]);
        doc.masters.push(vsdx::model::Master {
            id: 7,
            shapes: vec![master],
            ..vsdx::model::Master::default()
        });
        render_page(&doc, &page).unwrap()
    };
    for omit_row in [true, false] {
        let svg = render(omit_row);
        assert!(
            svg.contains("L 1.80000 ") && !svg.contains("L 3.60000 "),
            "omit_row={omit_row}: {svg}"
        );
    }
}

/// Inherited master cells in local geometry rows (the network LLD's
/// stretched U-Turn arrows): only a whole-identifier Width/Height
/// reference scales with the instance.
#[test]
fn own_size_references_are_whole_identifiers() {
    assert!(refs_own_size("Width*0.5"));
    assert!(refs_own_size("Height-User.BarWidth"));
    assert!(refs_own_size("MIN(Width,Height)"));
    assert!(!refs_own_size("User.BarWidth"));
    assert!(!refs_own_size("Sheet.5!Width*1"));
    assert!(!refs_own_size("TxtHeight*0.5"));
    assert!(!refs_own_size("Controls.Row_1"));
}

/// A master's `LineCap` formula evaluates with the instance's size,
/// not the master's cached value (Greptile on PR #342): `Width`
/// cached as 1 (flat) on a 1 in master is 2 (extended) on a 2 in
/// instance.
#[test]
fn line_cap_formulas_use_the_instance() {
    let mut master = rect_shape(1, (1.0, 1.0));
    master.sheet.set_cell("Width", Cell::value("1"));
    master.sheet.set_cell("Height", Cell::value("1"));
    master.sheet.set_cell(
        "LineCap",
        Cell {
            v: Some("1".into()),
            f: Some("Width".into()),
            ..Cell::default()
        },
    );
    let mut inst = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    inst.master = Some(7);
    let (mut doc, page) = page_fixture(vec![inst]);
    doc.masters.push(vsdx::model::Master {
        id: 7,
        shapes: vec![master],
        ..vsdx::model::Master::default()
    });
    let svg = render_page(&doc, &page).unwrap();
    assert!(svg.contains(r#"stroke-linecap="square""#), "{svg}");
    assert!(!svg.contains(r#"stroke-linecap="butt""#), "{svg}");
}

/// LineCap (Visio 2021 witness, b04): 0 round ends and corners
/// (the default), 1 flat, 2 square-extended; a walk that returns to
/// its start closes with `Z` so the start corner joins too.
#[test]
fn line_cap_and_closed_corner() {
    assert!(cap_attr(0, false).contains(r#"stroke-linecap="round""#));
    assert!(cap_attr(1, false).contains(r#"stroke-linecap="butt""#));
    assert!(cap_attr(2, false).contains(r#"stroke-linecap="square""#));
    assert!(
        !cap_attr(1, true).contains("linecap"),
        "dashes keep #235's caps"
    );
    // The join still follows LineCap when dashed (CodeRabbit on PR
    // #342).
    assert!(cap_attr(1, true).contains(r#"stroke-linejoin="miter""#));
    assert!(cap_attr(2, true).contains(r#"stroke-linejoin="miter""#));
    assert!(cap_attr(0, true).contains(r#"stroke-linejoin="round""#));
    assert!(close_returning_path("M 0 0 L 1 0 L 1 1 L 0 0").ends_with(" Z"));
    assert_eq!(
        close_returning_path("M 0 0 L 1 0 L 1 1"),
        "M 0 0 L 1 0 L 1 1"
    );
    let mut s = rect_shape(1, (2.0, 2.0));
    s.sheet.set_cell("LineCap", Cell::value("2"));
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(svg.contains(r#"Z" fill="none""#), "{svg}");
    assert!(svg.contains(r#"stroke-linecap="square""#), "{svg}");
}

/// #199 + #185: the gallery matches the 600 dpi Visio print (the
/// authoritative oracle in Analysis/arrow-gallery-visio2016.md) —
/// the open-head set 14–22 strokes instead of filling, the
/// dimension set composes ticks with rings/dots, 12 is the single
/// wide V while 43 is the double chevron, the rear arcs of
/// 5/6/8/18 are circular, and the bare ends 46–50 have no head.
#[test]
fn arrow_gallery_matches_the_visio_print() {
    // The open heads 14–22: stroked paths, never filled.
    for kind in [14, 15, 16, 17, 18, 19, 20, 21, 22] {
        let Some(parts) = arrow_spec(kind) else {
            panic!("arrow {kind} should exist");
        };
        assert!(
            parts.iter().all(|p| !p.filled),
            "arrow {kind} is an open head"
        );
    }
    // 22 is the stroked diamond (4.5 × 2.25 units).
    let parts = arrow_spec(22).unwrap();
    assert!(
        parts[0].d.contains("L-2.25,-1.125 L-4.5,0"),
        "diamond profile"
    );
    // #239: 38's diamond STROKES (the 600 dpi print's interior runs
    // a gap — the earlier "filled" read was the low-res gallery's);
    // its dot stays filled, like 34's ring stays open.
    let d38 = arrow_spec(38).unwrap();
    assert!(d38[0].circle.is_some() && d38[0].filled, "38's dot fills");
    assert!(
        !d38[1].filled && d38[1].circle.is_none(),
        "38's diamond strokes open"
    );
    // 12 is ONE wide open V; 43 is the double chevron (the print
    // correction — the old table had the two swapped up).
    let v12 = arrow_spec(12).unwrap();
    assert_eq!(v12[0].d, "M-3,-1 L0,0 L-3,1", "12 is the single wide V");
    let v43 = arrow_spec(43).unwrap();
    assert_eq!(v43[0].d.matches('M').count(), 2, "43 is the double chevron");
    // The 5/6/8/18 rears are circular arcs (exact cubic handles);
    // 5/8 concave, 6/18 convex.
    for kind in [5, 8, 6, 18] {
        let parts = arrow_spec(kind).unwrap();
        assert!(
            parts.iter().all(|p| p.d.contains('C')),
            "arrow {kind} carries the circular rear arc"
        );
    }
    let concave = arrow_spec(5).unwrap()[0].d;
    let convex = arrow_spec(6).unwrap()[0].d;
    assert!(concave.contains("C-1.68524"), "5 bows toward the tip");
    assert!(convex.contains("C-2.31476"), "6 bows away from the tip");
    // 39/40: double triangles — filled vs stroked.
    let filled = arrow_spec(39).unwrap();
    assert!(filled.iter().all(|p| p.filled));
    assert_eq!(filled[0].d.matches('M').count(), 2, "two heads");
    let open = arrow_spec(40).unwrap();
    assert!(open.iter().all(|p| !p.filled));
    // 20/41: rings; 42: a filled dot.
    for kind in [20, 41] {
        let parts = arrow_spec(kind).unwrap();
        assert!(parts.iter().all(|p| p.circle.is_some() && !p.filled));
    }
    let dot_parts = arrow_spec(42).unwrap();
    assert!(dot_parts[0].circle.is_some() && dot_parts[0].filled);
    // 25/26: two / three tick strokes in one part.
    let t2 = arrow_spec(25).unwrap();
    assert_eq!(t2[0].d.matches('M').count(), 2);
    let t3 = arrow_spec(26).unwrap();
    assert_eq!(t3[0].d.matches('M').count(), 3);
    // 35: a filled-dot part plus a tick part (composite).
    let comp = arrow_spec(35).unwrap();
    assert_eq!(comp.len(), 2);
    assert!(comp[0].filled && comp[0].circle.is_some());
    assert!(!comp[1].filled && comp[1].circle.is_none());
    // 9 leans "/" through the tip; 23 leans "\" behind it (the
    // print's two slanted ticks oppose).
    assert_eq!(arrow_spec(9).unwrap()[0].d, "M1,-1 L-1,1");
    assert_eq!(arrow_spec(23).unwrap()[0].d, "M-2.9,-1 L-0.9,1");
    // 46–50: bare ends.
    for kind in [46, 47, 48, 49, 50] {
        assert!(arrow_spec(kind).is_none(), "arrow {kind} is bare");
    }
}

/// #199 + #185 smoke: a line with EndArrow renders the head from
/// the verified table at its measured size — the open diamond 22
/// strokes with its shaft trimmed to the rear, the classic
/// triangle 4 fills with the shaft under its fill, the zero-weight
/// line uses the C[size] + 0.75 pt unit, and the bare end 46
/// carries no head.
#[test]
fn arrow_markers_render_from_the_verified_table() {
    let fixture = |kind: i64| -> String {
        let mut shape = Shape {
            id: 1,
            ..Shape::default()
        };
        shape
            .sheet
            .set_cell("EndArrow", Cell::value(kind.to_string()));
        shape.sheet.children.push(SheetChild::Section(Section {
            name: "Geometry".into(),
            rows: vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 0,
                    cells: vec![
                        ("X".into(), Cell::value("0")),
                        ("Y".into(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 1,
                    cells: vec![
                        ("X".into(), Cell::value("2")),
                        ("Y".into(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
            ],
            ..Section::default()
        }));
        let mut page = Page {
            id: 1,
            name: "Page-1".into(),
            ..Page::default()
        };
        page.shapes.push(shape);
        let mut doc = Document::default();
        doc.pages.push(page);
        render_page(&doc, &doc.pages[0]).expect("render")
    };
    // The unit at the default 0.75 pt pen, size 2: 0.75/72 +
    // 2.52/72 = 0.045417 in.
    let unit = 0.75 / 72.0 + 2.52 / 72.0;
    // #239: the diamonds trim TWO PENS short of their rear vertex
    // (the ink-tangency rule), so 22's setback is 4.5·unit − 2·pen.
    let pen = 0.75 / 72.0;
    // The open diamond 22 strokes, and its setback trims the shaft
    // to 2 − (4.5·unit − 2·pen).
    let open22 = fixture(22);
    assert!(open22.contains("dc-ah"), "head attached: {open22}");
    assert!(open22.contains("M0,0 L-2.25,-1.125 L-4.5,0 L-2.25,1.125 Z"));
    assert!(open22.contains("fill=\"none\" stroke=\"#000000\""));
    let cut = 2.0 - (4.5 * unit - 2.0 * pen);
    assert!(
        open22.contains(&format!("{cut:.5}")),
        "the shaft trims two pens short of the diamond's rear: {open22}"
    );
    // 34 (ring + open diamond) trims the same rule under its 6-unit
    // rear; 38's diamond STROKES (the #239 fixture pass: the print's
    // interior is hollow) and trims identically.
    let comp34 = fixture(34);
    let cut34 = 2.0 - (6.0 * unit - 2.0 * pen);
    assert!(
        comp34.contains(&format!("{cut34:.5}")),
        "34 trims two pens short of its rear: {comp34}"
    );
    let comp38 = fixture(38);
    assert!(
        comp38.contains("M-1.5,0 L-3.75,-1.125 L-6,0 L-3.75,1.125 Z\" fill=\"none\""),
        "38's diamond strokes open (the print's interior is hollow): {comp38}"
    );
    assert!(
        comp38.contains(&format!("{cut34:.5}")),
        "38 trims with the diamond rule: {comp38}"
    );
    // The classic triangle 4 fills; its 2-unit setback hides under
    // the fill (trim to 2 − 2·unit).
    let classic4 = fixture(4);
    assert!(classic4.contains("dc-ah"), "head attached: {classic4}");
    assert!(classic4.contains("<path d=\"M0,0 L-2,-1 L-2,1 Z\" fill=\"#000000\"/>"));
    assert!(classic4.contains(&format!("{:.5}", 2.0 - 2.0 * unit)));
    // The bare end 46 carries no head at all.
    let bare46 = fixture(46);
    assert!(!bare46.contains("dc-ah"), "bare ends carry no head");
}

/// #185: the shaft trim — a straight line shortens from each end
/// by the physical setbacks, closed loops and multi-subpath data
/// stay untouched, a cut that would consume the path keeps the
/// original, a quadratic splits exactly onto the original curve,
/// and an arc sub-span lands on the cut angle.
#[test]
fn trim_path_cuts_open_contours_only_and_exactly() {
    // Straight line: symmetric and one-sided trims.
    assert_eq!(
        trim_path("M 0 0 L 1 0 ", 0.1, 0.1),
        "M 0.10000 0.00000 L 0.90000 0.00000"
    );
    assert_eq!(
        trim_path("M 0 0 L 1 0 ", 0.0, 0.25),
        "M 0.00000 0.00000 L 0.75000 0.00000"
    );
    assert_eq!(
        trim_path("M 0 0 L 1 0 ", 0.25, 0.0),
        "M 0.25000 0.00000 L 1.00000 0.00000"
    );
    // A closed loop never trims.
    let closed = "M 0 0 L 1 0 L 1 1 L 0 0";
    assert_eq!(trim_path(closed, 0.1, 0.1), closed);
    // Multi-subpath data never trims.
    let multi = "M 0 0 L 1 0 M 2 0 L 3 0";
    assert_eq!(trim_path(multi, 0.1, 0.1), multi);
    // A cut that would consume the whole path keeps the original.
    assert_eq!(trim_path("M 0 0 L 1 0 ", 0.6, 0.6), "M 0 0 L 1 0 ");
    // Malformed data comes back untouched.
    assert_eq!(trim_path("garbage", 1.0, 1.0), "garbage");
    // A quadratic splits exactly: the trimmed sub-curve's endpoint
    // is the original at the cut parameter, and its own middle is
    // the original at half the kept span.
    let q = "M 0 0 Q 1 2 2 0 ";
    let samples = segment_samples('Q', &[1.0, 2.0, 2.0, 0.0], (0.0, 0.0));
    let total = polyline_length(&samples);
    let t_cut = sample_time_at_length(&samples, total - 0.5);
    let trimmed = trim_path(q, 0.0, 0.5);
    let expected_end = quad_at((0.0, 0.0), (1.0, 2.0), (2.0, 0.0), t_cut);
    assert!(
        trimmed.contains(&format!("{:.5} {:.5}", expected_end.0, expected_end.1)),
        "the cut lands on the curve: {trimmed}"
    );
    let q_part = trimmed.split("Q ").nth(1).expect("sub-quadratic");
    let nums: Vec<f64> = q_part
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    let got = quad_at((0.0, 0.0), (nums[0], nums[1]), (nums[2], nums[3]), 0.5);
    let want = quad_at((0.0, 0.0), (1.0, 2.0), (2.0, 0.0), t_cut * 0.5);
    assert!(
        (got.0 - want.0).abs() < 1e-4 && (got.1 - want.1).abs() < 1e-4,
        "the sub-quadratic tracks the original: {got:?} vs {want:?}"
    );
    // An arc sub-span: trimming a semicircle by a quarter of its
    // length lands the endpoint at the 270° point (1, -1), small
    // arc, sweep preserved.
    let a = "M 0 0 A 1 1 0 0 1 2 0 ";
    let trimmed = trim_path(a, 0.0, std::f64::consts::PI / 2.0);
    let a_part = trimmed.split("A ").nth(1).expect("sub-arc");
    let nums: Vec<f64> = a_part
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    assert!(
        (nums[5] - 1.0).abs() < 5e-3 && (nums[6] + 1.0).abs() < 5e-3,
        "the arc cut lands at the quarter point: {trimmed}"
    );
    assert_eq!(nums[3], 0.0, "the quarter arc is not large");
    assert_eq!(nums[4], 1.0, "the sweep survives the split");
}

// ------------------------------------------------------------------
// Wave-4 text stack (GitHub #232 #230 #229 #231 #234 #235).

/// The inline/master Geometry merge pairs SECTIONS by IX and keeps
/// inherited Rel* coordinates as fractions (CodeRabbit on PR #381): an
/// inline copy that keeps only Geometry IX=1 takes IX=1's section
/// cells (not IX=0's NoShow), and its RelLineTo's inherited X stays
/// 0.75 of the width for the painter to scale.
#[test]
fn merged_geometry_pairs_sections_by_ix_and_keeps_rel_fractions() {
    let section = |ix: i64, rows: Vec<Row>, cells: Vec<(String, Cell)>| {
        SheetChild::Section(Section {
            name: "Geometry".into(),
            ix,
            rows,
            cells,
            ..Section::default()
        })
    };
    let mut master = shape_at(1, (0.5, 0.5), 1.0, 1.0, 0.0);
    master.sheet.children.push(section(
        0,
        vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)])],
        vec![("NoShow".into(), Cell::value("1"))],
    ));
    master.sheet.children.push(section(
        1,
        vec![
            geo_row(1, "RelMoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "RelLineTo", &[("X", 0.75), ("Y", 0.5)]),
        ],
        vec![],
    ));
    let mut inst = shape_at(1, (4.0, 5.0), 4.0, 1.0, 0.0);
    inst.master = Some(7);
    inst.sheet.children.push(section(
        1,
        vec![
            geo_row(1, "RelMoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "RelLineTo", &[("Y", 0.25)]),
        ],
        vec![],
    ));
    let (mut doc, _page) = page_fixture(vec![inst.clone()]);
    doc.masters.push(vsdx::model::Master {
        id: 7,
        shapes: vec![master],
        ..vsdx::model::Master::default()
    });
    let chain = [&inst];
    let env = shape_env(&doc, &inst, &chain);
    let merged = crate::svg::shape_layer::merged_own_geometry(&doc, &inst, &chain, 4.0, 1.0, &env);
    assert_eq!(merged.len(), 1);
    assert!(
        merged[0].cell("NoShow").is_none(),
        "IX=1 has no NoShow; IX=0's must not leak in"
    );
    let x = merged[0].rows[1]
        .cell("X")
        .and_then(|c| c.numeric())
        .expect("the inherited X");
    assert!((x - 0.75).abs() < 1e-9, "a fraction, unscaled: {x}");
}
