//! #324 phase 2: Visio's own jump-rule fixtures, end to end —
//! `b01-linejumps.vsdx` (page `LineJumpCode` 1/2/3 across three
//! pages) and `c02-jumpcodes.vsdx` (connector `ConLineJumpCode` 0–4
//! plus the inert `ConFixedCode` 0–6 row), both authored AND saved
//! by Visio 2021 (mk-git, #322); the `.visio.pdf` beside each in the
//! issue is Visio's own export of the same file.
//!
//! The stored hop arcs in the files ARE the expected answer: our
//! crossing set must pick the same jumpers Visio stored, computed on
//! the STRAIGHTENED routes — the fixtures' stylesheet 0 carries
//! `LineToLineX` 0.0984252 × `LineJumpFactorX` 0.66667 / 2 = the
//! stored |A| 0.0328084 exactly, so every stored arc strips and the
//! hops recompute at the true crossings. The connectors carry no
//! local `ObjType`; the marker lives on their master — so these
//! files also pin the chain-resolved jump-candidacy gate.
//!
//! The fixtures are COMMITTED beside the test (owner instruction:
//! tests live inside `diagramcraft/`, never against the outer
//! `Sample/` tree — it is not pushed and its tests would run
//! nowhere else). A missing fixture fails the parse, not a skip.

const B01: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/b01-linejumps.vsdx"
);
const C02: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/c02-jumpcodes.vsdx"
);

const D04: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/d04-jumps-panel.vsdx"
);

fn open(path: &str) -> vsdx::Document {
    vsdx::parse::parse(std::path::Path::new(path))
        .unwrap_or_else(|e| panic!("fixture {path} must parse: {e}"))
}

/// The page's hop jumpers, sorted — the set to compare with Visio's
/// stored arcs.
fn jumpers(doc: &vsdx::Document, page: &vsdx::Page) -> Vec<i64> {
    let mut v: Vec<i64> = dc::geom::page_jumps(doc, page, &[])
        .iter()
        .map(|j| j.jumper)
        .collect();
    v.sort_unstable();
    v
}

/// b01: page 1 (the absent-cell default) hops the flatter line of
/// each pair — the horizontals 2 and 6, the shallow 18° line 8 — and
/// leaves the mirrored X (the equally steep 4/5) untouched; page 2
/// hops the verticals 3, 7, 9; page 3 hops the later line of every
/// pair, the X included (5).
#[test]
fn b01_page_codes_pick_visio_stored_jumpers() {
    let doc = open(B01);
    assert_eq!(
        jumpers(&doc, &doc.pages[0]),
        vec![2, 6, 8],
        "page 1 (absent cell = Visio's default)"
    );
    assert_eq!(jumpers(&doc, &doc.pages[1]), vec![3, 7, 9], "page 2");
    assert_eq!(
        jumpers(&doc, &doc.pages[2]),
        vec![3, 5, 7, 9],
        "page 3 — the later line, the mirrored X included"
    );
}

/// c02 (page 1): the horizontal of each pair hops — every
/// `ConFixedCode` 0–6 on the top row leaves that unchanged (the
/// cell is inert) — except where the horizontal's own
/// `ConLineJumpCode` says otherwise: 1 never (26), 4 neither (35),
/// and 3 hands the hop to the vertical (33 hops over 32).
#[test]
fn c02_connector_codes_pick_visio_stored_jumpers() {
    let doc = open(C02);
    assert_eq!(
        jumpers(&doc, &doc.pages[0]),
        vec![2, 5, 8, 11, 14, 17, 20, 23, 29, 33],
        "the ConFixedCode row hops throughout; ConLineJumpCode 1/4 rows do not"
    );
}

/// The svg ink gate rides the SAME chain-resolved candidacy as the
/// crossing set: b01's connectors carry `ObjType` only on their
/// master, and shape 8's stored hop sits on a DIAGONAL run — a full
/// page render must straighten every stored arc (no `A` path command
/// anywhere; the drawn hop arcs are sampled polylines) while the
/// jump group still draws. A scoped render is no witness here: it
/// passes a zero strip radius by design. (CodeRabbit round on phase
/// 2 asked whether the ink gate accepts master-carried candidacy:
/// `resolve_cell` follows the instance's own master before any
/// ancestors, so it does — this pins it.)
#[test]
fn b01_ink_straightens_master_objtype_stored_hops() {
    let doc = open(B01);
    let svg = dc::svg::render_page(&doc, &doc.pages[0]).unwrap();
    assert!(
        !svg.contains(" A "),
        "every stored arc straightens in the ink, on master-carried candidacy"
    );
    assert!(
        svg.contains(r#"data-dc="line-jumps""#),
        "the page still draws its computed hops"
    );
}

/// d04 (Visio-authored, #322): the 60° link carries Visio's stored
/// hop ON ITS OWN DIAGONAL RUN — with the classifier's
/// axis-aligned-chord condition gone (#324 phase 2/3) the arc strips
/// from the walk and the recomputed hop lands at the true crossing
/// with the same jumper (the 60° link is flatter than the 84° one);
/// the mirrored X (equally steep) draws nothing; and a hop over the
/// gray chassis panel reads the PANEL's fill as its backdrop — the
/// canvas's gap color, where paper white used to dash (#322).
#[test]
fn d04_diagonal_stored_hop_strips_and_the_backdrop_is_the_panel() {
    let doc = open(D04);
    let page = &doc.pages[0];
    assert_eq!(dc::geom::page_jump_rules(&doc, page).page_code, 1);
    // Visio's own stored arcs sit on shapes 3 and 5 — the crossing
    // set picks the same jumpers.
    let js = dc::geom::page_jumps(&doc, page, &[]);
    let mut got: Vec<i64> = js.iter().map(|j| j.jumper).collect();
    got.sort_unstable();
    assert_eq!(got, vec![3, 5], "{js:?}");
    // The 60°×84° hop sits at the geometric crossing of the two
    // links' endpoints, running along the 60° line.
    let j5 = js.iter().find(|j| j.jumper == 5).unwrap();
    assert!(
        (j5.at.0 - 4.119_06).abs() < 1e-3 && (j5.at.1 - 2.045_55).abs() < 1e-3,
        "the hop sits at the 60°×84° crossing (the endpoints' intersection,          to the frame walk's rounding): {:?}",
        j5.at
    );
    assert!(
        (j5.dir.0 - 0.5).abs() < 1e-6,
        "the hop runs along the 60° link: {:?}",
        j5.dir
    );
    // The stored arc on the diagonal run STRIPPED: every endpoint of
    // the strip-aware walk lies on the straight Begin→End line — the
    // control walk (no strip radii) keeps the arc's bulging samples.
    let s5 = vsdx::model::find_shape_by_id(&page.shapes, 5).unwrap();
    let root = s5
        .master
        .and_then(|mid| doc.master(mid))
        .and_then(|m| m.shapes.first())
        .unwrap();
    let chain = vec![s5, root];
    let g = |n: &str| {
        doc.resolve_cell(root, &[root], n)
            .and_then(|c| c.numeric())
            .unwrap_or(1.0)
    };
    let frame = dc::geom::hit_frame(&doc, &chain, s5, (g("Width"), g("Height")));
    let begin = (3.4, 0.8);
    let end = (4.554_700_5, 2.8);
    let off_line = |p: (f64, f64)| {
        ((end.0 - begin.0) * (p.1 - begin.1) - (end.1 - begin.1) * (p.0 - begin.0)).abs()
            / (end.0 - begin.0).hypot(end.1 - begin.1)
    };
    let segs = dc::geom::geometry_hit_segments(
        s5,
        Some(root),
        &frame,
        Some(dc::geom::jump_radii_in(&doc, page)),
    );
    assert!(
        !segs.is_empty()
            && segs
                .iter()
                .flat_map(|(a, b)| [*a, *b])
                .all(|p| off_line(p) < 1e-6),
        "the diagonal stored arc strips to the straight run"
    );
    let raw = dc::geom::geometry_hit_segments(s5, Some(root), &frame, None);
    assert!(
        raw.iter()
            .flat_map(|(a, b)| [*a, *b])
            .any(|p| off_line(p) > 0.01),
        "the control walk keeps the arc's bulge"
    );
    // The hop over the panel reads the PANEL's fill as its backdrop
    // (never paper white); below a page point with nothing filled,
    // white.
    let j3 = js.iter().find(|j| j.jumper == 3).unwrap();
    assert_eq!(
        dc::svg::backdrop_color_at(&doc, page, j3.at, 3, &[]),
        (0xbf, 0xbf, 0xbf),
        "the gap over the gray panel paints the panel's fill"
    );
    assert_eq!(
        dc::svg::backdrop_color_at(&doc, page, (7.8, 4.6), 8, &[]),
        (0xff, 0xff, 0xff),
        "off the panel the backdrop is the page's white"
    );
}
