//! Tests for text resolution, wrapping, run styling, and emission.
use super::*;

/// #73: Char.Color accepts both spellings with fill/line-parity
/// semantics — a `#rrggbb` hex string verbatim, a legacy palette
/// index through the [MS-VSDX] table — so a palette write and a
/// legacy file render identically.
#[test]
fn char_color_accepts_hex_and_palette_index() {
    let doc = Document::default();
    let emit = |color: &str| {
        let mut s = Shape::default();
        s.sheet.text = Some("colored".into());
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.set_cell("TxtWidth", Cell::value("2.0"));
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Character".into(),
            rows: vec![Row {
                cells: vec![("Color".to_string(), Cell::value(color))],
                ..Row::default()
            }],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        out
    };
    let hex = emit("#ED7D31");
    assert!(
        hex.contains(r##"fill="#ED7D31""##),
        "hex Char.Color renders verbatim: {hex}"
    );
    // Palette index 4 is #0000FF in the [MS-VSDX] table; 2 is
    // #FF0000 — the legacy spelling resolves through the same
    // mapping as the fill/line path.
    assert!(
        emit("4").contains(r##"fill="#0000FF""##),
        "an index Char.Color resolves through the palette table"
    );
    assert!(emit("2").contains(r##"fill="#FF0000""##), "index 2 is red");
    // No Color cell: the built-in default is black.
    let mut s = Shape::default();
    s.sheet.text = Some("plain".into());
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("2.0"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(
        out.contains(r##"fill="#000000""##),
        "default text is black: {out}"
    );
}

#[test]
fn font_family_chains_follow_the_alias_table() {
    // Document family first, then the metric-compatible substitute,
    // then Carlito, then the CSS generic (GitHub #17's example).
    assert_eq!(
        font_family_attr(Some("Arial")),
        " font-family=\"Arial, 'Liberation Sans', Carlito, sans-serif\""
    );
    assert_eq!(
        font_family_attr(Some("Calibri")),
        " font-family=\"Calibri, Carlito, sans-serif\"",
        "Calibri's substitute IS Carlito — no duplicate entry"
    );
    assert_eq!(
        font_family_attr(Some("Times New Roman")),
        " font-family=\"'Times New Roman', 'Liberation Serif', Carlito, serif\""
    );
    assert_eq!(
        font_family_attr(Some("Courier New")),
        " font-family=\"'Courier New', 'Liberation Mono', Carlito, monospace\""
    );
    assert_eq!(
        font_family_attr(Some("Cambria")),
        " font-family=\"Cambria, Caladea, Carlito, serif\""
    );
    // Unknown families: kept as-is, Carlito + sans-serif tail.
    assert_eq!(
        font_family_attr(Some("Wingdings")),
        " font-family=\"Wingdings, Carlito, sans-serif\""
    );
    // Lookup is case-insensitive and trims; no Font cell (or an
    // empty one) falls back to Visio's default family, Calibri.
    assert_eq!(
        font_family_attr(Some("  ARIAL ")),
        " font-family=\"ARIAL, 'Liberation Sans', Carlito, sans-serif\""
    );
    assert_eq!(
        font_family_attr(None),
        " font-family=\"Calibri, Carlito, sans-serif\"",
        "Visio's default family is Calibri"
    );
    assert_eq!(
        font_family_attr(Some("")),
        " font-family=\"Calibri, Carlito, sans-serif\""
    );
}

#[test]
fn char_font_reads_the_first_character_row() {
    let doc = Document::default();
    let s = shape_with_character(Some("Arial"));
    let chain = [&s];
    assert_eq!(char_font(&doc, &s, &chain).as_deref(), Some("Arial"));
    // Numeric (legacy binary index) values name no family.
    let s = shape_with_character(Some("4"));
    let chain = [&s];
    assert_eq!(char_font(&doc, &s, &chain), None);
    // No Font cell / no section at all.
    let s = shape_with_character(None);
    let chain = [&s];
    assert_eq!(char_font(&doc, &s, &chain), None);
    let s = Shape::default();
    let chain = [&s];
    assert_eq!(char_font(&doc, &s, &chain), None);
}

#[test]
fn char_font_inherits_from_the_master_sheet() {
    // The instance's own sheet carries no sections, so the
    // effective sheet is its master's (the Size/Color path).
    let mut root = shape_with_character(Some("Segoe UI"));
    root.id = 1;
    let doc = Document {
        masters: vec![vsdx::Master {
            id: 7,
            shapes: vec![root],
            ..Default::default()
        }],
        ..Document::default()
    };
    let inst = Shape {
        id: 2,
        master: Some(7),
        ..Shape::default()
    };
    assert_eq!(char_font(&doc, &inst, &[]).as_deref(), Some("Segoe UI"));
    // An own Character row wins over the master's.
    let mut inst = Shape {
        id: 3,
        master: Some(7),
        ..Shape::default()
    };
    inst.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![char_row(Some("Cambria"))],
        ..Section::default()
    }));
    assert_eq!(char_font(&doc, &inst, &[]).as_deref(), Some("Cambria"));
}

#[test]
fn emitted_text_carries_the_font_family_chain() {
    let doc = Document::default();
    let mut s = shape_with_character(Some("Arial"));
    s.sheet.text = Some("hello".into());
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(
        out.contains(r#"font-family="Arial, 'Liberation Sans', Carlito, sans-serif""#),
        "emitted text element: {out}"
    );
    assert!(out.contains("hello"), "emitted: {out}");
}

/// #124 item 2: a text block whose on-screen em height falls under
/// TEXT_GREEK_PX emits gray bars (no glyph nodes for usvg to turn
/// into outlines) and records its size in the greek profile; above
/// the threshold — or with no ppi at all (export/print/thumbnail
/// paths pass None) — glyphs emit exactly as before.
#[test]
fn text_greeks_below_the_pixel_threshold() {
    let doc = Document::default();
    let tiny = |size: &str| {
        // sizes floor at 0.02 in in resolve_text_block
        let mut s = Shape::default();
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Character".into(),
            rows: vec![Row {
                cells: vec![("Size".to_string(), Cell::value(size))],
                ..Row::default()
            }],
            ..Section::default()
        }));
        s.sheet.text = Some("tiny text".into());
        s
    };
    // 0.02 in em (the resolver floor) at 96 ppi = 1.92 px: greeked to bars.
    let s = tiny("0.02");
    let chain = [&s];
    let ctx = GreekCtx::new(96.0);
    let mut out = String::new();
    emit_text(
        &doc,
        &s,
        &chain,
        2.0,
        1.0,
        1.0,
        1.0,
        false,
        Some(&ctx),
        None,
        &mut out,
    );
    assert!(
        out.contains(r##"fill="#7f7f7f""##),
        "greek bars emitted: {out}"
    );
    assert!(!out.contains("<text"), "no glyph node when greeked: {out}");
    assert!(!out.contains("<tspan"), "no tspan when greeked: {out}");
    assert_eq!(ctx.sizes.into_inner(), vec![0.02]);
    assert_eq!(greeked_count(&[0.02], 96.0), 1);

    // 0.13889 in em (the 10 pt default) at 96 ppi ≈ 13.3 px: glyphs.
    let s = tiny("0.13889");
    let chain = [&s];
    let ctx = GreekCtx::new(96.0);
    let mut out = String::new();
    emit_text(
        &doc,
        &s,
        &chain,
        2.0,
        1.0,
        1.0,
        1.0,
        false,
        Some(&ctx),
        None,
        &mut out,
    );
    assert!(out.contains("tiny text"), "glyph path unchanged: {out}");
    assert!(
        !out.contains("#7f7f7f"),
        "no bars above the threshold: {out}"
    );
    assert_eq!(
        greeked_count(&[0.13889], 96.0),
        0,
        "13 px text does not greek"
    );

    // No greek context (export/print/thumbnails): glyphs regardless
    // of size — greeking never enters those paths.
    let s = tiny("0.02");
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(out.contains("tiny text"), "export never greeks: {out}");
}

#[test]
fn rotated_text_block_pivots_at_txtpin() {
    let Some((t, pin)) = text_wrapper_transform(1.0, 1.0, 30.0) else {
        panic!("rotation must emit a transform wrapper");
    };
    let m = compose_tf(&t);
    let [a, b, c, d, e, f] = m.0;
    let (x, y) = (pin.0 * a + pin.1 * c + e, pin.0 * b + pin.1 * d + f);
    assert!(
        (x - pin.0).abs() < 1e-6 && (y - pin.1).abs() < 1e-6,
        "anchor ({x},{y}) must stay at the TxtPin {pin:?} — transform {t}"
    );
}

#[test]
fn flipped_text_block_pivots_at_txtpin() {
    // FlipX only and both axes: the counter-scale keeps the anchor
    // fixed at the pin in each case.
    for (sx, sy) in [(-1.0, 1.0), (-1.0, -1.0)] {
        let Some((t, pin)) = text_wrapper_transform(sx, sy, 0.0) else {
            panic!("flip must emit a transform wrapper");
        };
        let m = compose_tf(&t);
        let [a, b, c, d, e, f] = m.0;
        let (x, y) = (pin.0 * a + pin.1 * c + e, pin.0 * b + pin.1 * d + f);
        assert!(
            (x - pin.0).abs() < 1e-6 && (y - pin.1).abs() < 1e-6,
            "anchor ({x},{y}) must stay at the TxtPin {pin:?} — transform {t}"
        );
    }
}

#[test]
fn unflipped_unrotated_text_has_no_wrapper() {
    assert!(text_wrapper_transform(1.0, 1.0, 0.0).is_none());
}

/// Local CodeRabbit round 3 + Greptile round 7: paragraph cells
/// inherit through the TEXT-STYLE stylesheet cascade like
/// character cells do (`char_cell`'s walk) — a styled shape with
/// no local or master Paragraph row still picks up the style's
/// IndLeft/Bullet, a local cell still wins, a local row carrying
/// only another cell does not shadow the style (per-cell
/// inheritance), and INHERITED cells (F="Inh") never satisfy a
/// level: their cached V is stale against the live parent, so the
/// parent's current value wins (resolve_cell's rule).
#[test]
fn paragraph_cells_inherit_the_text_style_cascade() {
    // Parent style 8 carries the live values; child style 7
    // caches a STALE IndLeft under F="Inh" and links to 8.
    let parent = {
        let mut sh = Sheet::default();
        sh.children.push(SheetChild::Section(Section {
            name: "Paragraph".into(),
            rows: vec![Row {
                cells: vec![
                    ("IndLeft".to_string(), Cell::value("0.25")),
                    ("Bullet".to_string(), Cell::value("2")),
                ],
                ..Row::default()
            }],
            ..Section::default()
        }));
        vsdx::model::StyleSheet {
            id: 8,
            name_u: None,
            attrs: Vec::new(),
            sheet: sh,
        }
    };
    let child = {
        let mut sh = Sheet::default();
        sh.children.push(SheetChild::Section(Section {
            name: "Paragraph".into(),
            rows: vec![Row {
                cells: vec![(
                    "IndLeft".to_string(),
                    Cell {
                        v: Some("0.5".into()),
                        f: Some("Inh".into()),
                        ..Cell::default()
                    },
                )],
                ..Row::default()
            }],
            ..Section::default()
        }));
        vsdx::model::StyleSheet {
            id: 7,
            name_u: None,
            attrs: vec![("TextStyle".to_string(), "8".to_string())],
            sheet: sh,
        }
    };
    let mut doc = Document::default();
    doc.stylesheets.push(parent);
    doc.stylesheets.push(child);
    let base = || {
        let mut s = Shape::default();
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.text = Some("styled".into());
        s.text_style = Some(7);
        s
    };
    // Bare styled shape: the PARENT's live value wins through the
    // child's stale Inh cache.
    let s = base();
    let tb = resolve_text_block(&doc, &s, &[&s], 2.0, 1.0);
    assert!(
        (tb.ind_left - 0.25).abs() < 1e-12,
        "the parent's live IndLeft wins over the child's Inh cache: {}",
        tb.ind_left
    );
    assert!(tb.bullet.is_some(), "the style's Bullet arms the block");
    // A stale Inh cache on the SHAPE itself never wins either.
    let mut stale = base();
    stale.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![(
                "IndLeft".to_string(),
                Cell {
                    v: Some("9".into()),
                    f: Some("Inh".into()),
                    ..Cell::default()
                },
            )],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let tb = resolve_text_block(&doc, &stale, &[&stale], 2.0, 1.0);
    assert!(
        (tb.ind_left - 0.25).abs() < 1e-12,
        "a shape-level Inh cache falls through to the style: {}",
        tb.ind_left
    );
    // A local cell WINS over the style.
    let mut local = base();
    local.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![("IndLeft".to_string(), Cell::value("0.5"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let tb = resolve_text_block(&doc, &local, &[&local], 2.0, 1.0);
    assert!(
        (tb.ind_left - 0.5).abs() < 1e-12,
        "the local IndLeft wins: {}",
        tb.ind_left
    );
    // A local row carrying ONLY HorzAlign leaves the style's
    // IndLeft standing (per-cell inheritance, PR #284 round 2).
    let mut shadow = base();
    shadow.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![("HorzAlign".to_string(), Cell::value("0"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let tb = resolve_text_block(&doc, &shadow, &[&shadow], 2.0, 1.0);
    assert!(
        (tb.ind_left - 0.25).abs() < 1e-12,
        "a HorzAlign-only local row never shadows the style: {}",
        tb.ind_left
    );
}

/// The Text Block Format cell is `VerticalAlign` with 0 top,
/// 1 middle, 2 bottom (Visio 2021 witness: three boxes with 0/1/2
/// put the text top/middle/bottom). It maps onto the block's
/// 1/2/3 encoding; out-of-range values clamp.
#[test]
fn vertical_align_cell_maps_top_middle_bottom() {
    let va = |v: Option<&str>| {
        let mut s = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
        if let Some(v) = v {
            s.sheet.set_cell("VerticalAlign", Cell::value(v));
        }
        let (doc, page) = page_fixture(vec![s]);
        text_block(&doc, &page, 1).unwrap().valign
    };
    assert_eq!(va(Some("0")), 1, "0 = top");
    assert_eq!(va(Some("1")), 2, "1 = middle");
    assert_eq!(va(Some("2")), 3, "2 = bottom");
    assert_eq!(va(None), 2, "absent = middle");
    assert_eq!(va(Some("7")), 3, "clamped");
}

/// Text Block Format cells come from the TEXT style. A shape with
/// a different line style must still pick up the text style's
/// `VerticalAlign` and margins (Greptile/CodeRabbit on PR #325).
#[test]
fn text_block_cells_resolve_through_the_text_style() {
    let style = |id: i64, cells: &[(&str, &str)]| {
        let mut sh = Sheet::default();
        for (n, v) in cells {
            sh.set_cell(n, Cell::value(*v));
        }
        vsdx::model::StyleSheet {
            id,
            name_u: None,
            attrs: Vec::new(),
            sheet: sh,
        }
    };
    let mut doc = Document::default();
    doc.stylesheets.push(style(3, &[("LineWeight", "0.02")]));
    doc.stylesheets
        .push(style(4, &[("VerticalAlign", "0"), ("TopMargin", "0.2")]));
    let mut s = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    s.sheet.text = Some("styled".into());
    s.line_style = Some(3);
    s.fill_style = Some(3);
    s.text_style = Some(4);
    let tb = resolve_text_block(&doc, &s, &[&s], 2.0, 1.0);
    assert_eq!(tb.valign, 1, "the text style's VerticalAlign=0 is top");
    assert!(
        (tb.insets.2 - 0.2).abs() < 1e-12,
        "the text style's TopMargin: {}",
        tb.insets.2
    );
}

/// Character runs (Visio 2021 witness c03): `<cp IX='n'/>` markers
/// pick Character rows; a struck run gets a line through it, a red
/// bold run its own positioned chunk, an underlined run a line under
/// it. A block whose rows match the first row renders unchanged.
#[test]
fn character_runs_style_their_chunks() {
    let mut s = rect_shape(1, (2.0, 2.0));
    s.sheet.set_cell("Width", Cell::value("3"));
    s.sheet.text = Some("normal struck red".into());
    s.sheet.text_raw =
        Some("<cp IX='0'/>normal <cp IX='1'/>struck<cp IX='0'/> <cp IX='2'/>red".into());
    let mut sec = Section {
        name: "Character".into(),
        ..Section::default()
    };
    let row = |ix: i64, cells: &[(&str, &str)]| Row {
        ix,
        cells: cells
            .iter()
            .map(|(n, v)| (n.to_string(), Cell::value(*v)))
            .collect(),
        ..Row::default()
    };
    sec.rows.push(row(0, &[]));
    sec.rows.push(row(1, &[("Strikethru", "1")]));
    sec.rows
        .push(row(2, &[("Color", "#ff0000"), ("Style", "1")]));
    s.sheet.children.push(SheetChild::Section(sec));
    let (doc, page) = page_fixture(vec![s]);
    let svg = render_page(&doc, &page).unwrap();
    assert!(
        svg.contains(r##"fill="#ff0000" font-weight="bold">red</tspan>"##),
        "{svg}"
    );
    assert!(svg.contains(">struck</tspan>"), "{svg}");
    assert_eq!(
        svg.matches("<line ").count(),
        1,
        "one strike-through line: {svg}"
    );
}

/// A run set in a smaller size measures at that size when the line
/// wraps (the network LLD's P2MP labels: 10 pt + 8 pt fit the
/// 3.58 in block on one line in Visio; measured all at 10 pt they
/// wrapped).
#[test]
fn smaller_runs_measure_at_their_own_size_when_wrapping() {
    let render = |tail: &str| {
        let mut s = rect_shape(1, (4.0, 2.0));
        for (n, v) in [
            ("Width", "3.5828"),
            ("LeftMargin", "0"),
            ("RightMargin", "0"),
        ] {
            s.sheet.set_cell(n, Cell::value(v));
        }
        s.sheet.text =
            Some("P2MP:vl2170:IPv4: 88.213.189.16/29 IPv6:2a00:d80:1420:1170::/64".into());
        s.sheet.text_raw = Some(
            "<cp IX='0'/>P2MP:vl2170:IPv4: 88.213.189.16/29 IPv6:<cp IX='1'/>2a00:d80:1420:1170::/64"
                .into(),
        );
        let mut sec = Section {
            name: "Character".into(),
            ..Section::default()
        };
        let row = |ix: i64, size: &str| Row {
            ix,
            cells: vec![
                ("Font".to_string(), Cell::value("Calibri")),
                ("Size".to_string(), Cell::value(size)),
            ],
            ..Row::default()
        };
        sec.rows.push(row(0, "0.1388888888888889"));
        sec.rows.push(row(1, tail));
        s.sheet.children.push(SheetChild::Section(sec));
        let (doc, page) = page_fixture(vec![s]);
        render_page(&doc, &page).unwrap()
    };
    // Distinct baselines: a run's own chunk shares its line's y.
    let lines = |svg: &str| {
        let mut ys: Vec<&str> = svg
            .split(" y=\"")
            .skip(1)
            .filter_map(|t| t.split('"').next())
            .collect();
        ys.sort_unstable();
        ys.dedup();
        ys.len()
    };
    // The 8 pt tail fits on the first line; set at 10 pt it wraps.
    let mixed = render("0.1111111111111111");
    let uniform = render("0.1388888888888889");
    assert!(
        lines(&uniform) > lines(&mixed),
        "mixed: {mixed}\nuniform: {uniform}"
    );
}

/// The wrap measures a paragraph set wholly in another size at that
/// size, a bold run with the bold face, and survives a narrow CJK
/// token whose split leaves nothing behind (Greptile on PR #350).
#[test]
fn wrap_runs_measure_uniform_sizes_bold_faces_and_empty_rests() {
    let face = crate::textmetrics::resolve(Some("Calibri"), false, false);
    let size = 0.1389;
    let text = "aaaa bbbb cccc";
    let n = text.chars().count();
    let w = face.width_in_runs(&[], text, size);
    let wrap = |usable: f64, runs: Option<&WrapRuns>| {
        wrap_block_text_scaled(&face, text, size, usable, &face, size, &[], 0.0, 0.0, runs)
            .len()
    };
    assert_eq!(wrap(w * 1.05, None), 1);
    let big = WrapRuns {
        factor: vec![2.0; n],
        face: vec![None; n],
        faces: Vec::new(),
    };
    assert!(wrap(w * 1.05, Some(&big)) > 1, "a 2x paragraph wraps");
    let bold_face = crate::textmetrics::resolve(Some("Calibri"), true, false);
    let bold_w = bold_face.width_in_runs(&[], text, size);
    assert!(bold_w > w * 1.01, "the bold face is wider");
    let bold = WrapRuns {
        factor: vec![1.0; n],
        face: vec![Some(0); n],
        faces: vec![bold_face],
    };
    assert!(wrap(w * 1.005, Some(&bold)) > 1, "a bold run wraps bold");
    let cjk = WrapRuns {
        factor: vec![1.0, 0.5],
        face: vec![None, None],
        faces: Vec::new(),
    };
    let lines = wrap_block_text_scaled(
        &face,
        "图，",
        size,
        0.01,
        &face,
        size,
        &[],
        0.0,
        0.0,
        Some(&cjk),
    );
    assert!(!lines.is_empty());
}

/// Mixed-size lines place and space by their runs (Greptile on PR
/// #350): a right-aligned line with a smaller tail ends at the right
/// edge, a 30 pt run pushes the next line down, a superscript run
/// draws at its reduced size, and a bulleted line keeps its runs.
#[test]
fn mixed_size_runs_place_space_and_script() {
    let tail = |z: &str| {
        emit_runs(
            "<cp IX='0'/>right aligned <cp IX='1'/>tail",
            &[(0, &[]), (1, &[("Size", z), ("Color", "#ff0000")])],
            &[],
            &[("HorzAlign", "2")],
        )
    };
    let x0 = |svg: &str| tspan_xy(svg)[0].0;
    assert!(
        x0(&tail("0.1111")) > x0(&tail("0.1389")) + 1e-3,
        "a smaller tail starts further right"
    );
    let tall = |z: &str| {
        emit_runs(
            "<cp IX='0'/>first\n<cp IX='1'/>big<cp IX='2'/>\nthird",
            &[
                (0, &[]),
                (1, &[("Size", z), ("Color", "#ff0000")]),
                (2, &[]),
            ],
            &[],
            &[],
        )
    };
    let gap = |svg: &str| {
        let ys: Vec<f64> = tspan_xy(svg).iter().map(|p| p.1).collect();
        ys[ys.len() - 1] - ys[0]
    };
    assert!(
        gap(&tall("0.4167")) > gap(&tall("0.1389")) + 0.2,
        "a 30 pt line takes 30 pt of height"
    );
    let sup = emit_runs(
        "<cp IX='0'/>H<cp IX='1'/>2",
        &[(0, &[("Pos", "1")]), (1, &[("Pos", "1"), ("Size", "0.2")])],
        &[],
        &[],
    );
    assert!(!sup.contains(r#"font-size="0.2000""#), "{sup}");
    let bullet = emit_runs(
        "<cp IX='0'/>bulleted <cp IX='1'/>red",
        &[(0, &[]), (1, &[("Color", "#ff0000")])],
        &[],
        &[("Bullet", "1")],
    );
    assert!(bullet.contains(r##"fill="#ff0000""##), "{bullet}");
}

/// A run whose Color is `THEMEVAL()` in a theme-less drawing takes the
/// No Style black, not the first row's color (the network LLD: address
/// lines under a green heading, black in Visio 2021). An `Inh` run still
/// paints as the block.
#[test]
fn themed_run_color_takes_no_style_black() {
    let svg = emit_runs(
        "<cp IX='0'/>green heading\n<cp IX='1'/>10.0.0.1/32",
        &[(0, &[("Color", "#008000")]), (1, &[("Color", "Themed")])],
        &[],
        &[],
    );
    assert!(
        svg.contains(r##"fill="#000000">10.0.0.1/32</tspan>"##),
        "{svg}"
    );
    assert!(!svg.contains(r##"fill="#008000">10.0.0.1"##), "{svg}");
}

/// Lines set wholly in a smaller run sit closer together: Visio 2021
/// spaces a line by its largest run, also below the block size (the
/// network LLD: 6 pt address lines in an 8 pt box).
#[test]
fn smaller_run_lines_space_at_their_own_size() {
    let lines = |z: &str| {
        emit_runs(
            "<cp IX='0'/>head\n<cp IX='1'/>one\ntwo\nthree",
            &[(0, &[("Size", "0.1111")]), (1, &[("Size", z)])],
            &[],
            &[],
        )
    };
    let span = |svg: &str| {
        let ys: Vec<f64> = tspan_xy(svg).iter().map(|p| p.1).collect();
        ys[ys.len() - 1] - ys[1]
    };
    let small = span(&lines("0.0833"));
    let same = span(&lines("0.1111"));
    // Two 6 pt slots against two 8 pt slots (1.2 em each).
    assert!(
        (small - same * 0.0833 / 0.1111).abs() < 0.01,
        "small {small} same {same}"
    );
}

/// A document-controlled `IX` does not size an allocation (Greptile
/// on PR #350): only referenced rows get a style.
#[test]
fn huge_character_ix_values_stay_small() {
    let svg = emit_runs(
        "<cp IX='0'/>plain <cp IX='1000000000'/>red",
        &[(0, &[]), (1_000_000_000, &[("Color", "#ff0000")])],
        &[],
        &[],
    );
    assert!(svg.contains(r##"fill="#ff0000""##), "{svg}");
}

/// Stock monospace faces fall back to the vendored monospace
/// Liberation Mono, never the proportional Carlito.
#[test]
fn monospace_stock_fonts_stay_monospace() {
    for f in ["Lucida Console", "Consolas", "Courier New"] {
        let chain = family_chain(Some(f));
        assert_eq!(chain[1], "Liberation Mono", "{f}: {chain:?}");
        assert_eq!(chain.last().map(String::as_str), Some("monospace"));
    }
    // Not a stock Visio font: it lays out as Calibri like any
    // unknown family (Greptile on PR #332 found its old mapping
    // unreachable).
    let chain = family_chain(Some("Lucida Sans Typewriter"));
    assert_eq!(chain[0], "Calibri", "{chain:?}");
}

/// GitHub #22: the shared TextBlock resolution must agree with the
/// defaults `emit_text` has always used — absent `Txt*` cells pin
/// the block at the shape's center, size it to the shape, and take
/// the Visio built-ins (10 pt, black, centered, 0.1/0.0555 in
/// insets).
#[test]
fn text_block_defaults_match_emit_text() {
    let (doc, page) = page_fixture(vec![shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0)]);
    let tb = text_block(&doc, &page, 1).unwrap();
    assert_eq!(tb.pin, (1.0, 0.5), "pin defaults to the shape's center");
    assert_eq!(tb.size, (2.0, 1.0), "size defaults to Width/Height");
    assert_eq!(tb.loc_pin, (1.0, 0.5));
    assert_eq!(tb.angle, 0.0);
    assert!((tb.font_size - 0.13888).abs() < 1e-9, "10 pt default");
    assert_eq!(tb.color, "#000000");
    assert!(!tb.bold && !tb.italic);
    assert_eq!(tb.halign, 1, "Visio default HorzAlign = center");
    assert_eq!(tb.valign, 2, "middle");
    assert_eq!(tb.insets, (0.1, 0.1, 0.0555, 0.0555));
    assert!((tb.line_height - tb.font_size * 1.2).abs() < 1e-12);
    assert_eq!(tb.font, None);
}

/// Explicit `Txt*` cells and Character/Paragraph styling resolve
/// through the chain (GitHub #22 — the editor overlay and the
/// renderer read the same values).
#[test]
fn text_block_reads_txt_cells_and_style() {
    let mut sh = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    for (n, v) in [
        ("TxtPinX", 1.5),
        ("TxtPinY", 0.25),
        ("TxtWidth", 1.0),
        ("TxtHeight", 0.5),
        ("TxtLocPinX", 0.5),
        ("TxtLocPinY", 0.25),
    ] {
        sh.sheet.set_cell(n, Cell::value(format!("{v}")));
    }
    sh.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![Row {
            cells: vec![
                ("Font".into(), Cell::value("Arial")),
                ("Size".into(), Cell::value("0.27778")), // 20 pt
                ("Color".into(), Cell::value("#FF0000")),
                ("Style".into(), Cell::value("1")), // bold
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    sh.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![("HorzAlign".into(), Cell::value("2"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let (doc, page) = page_fixture(vec![sh]);
    let tb = text_block(&doc, &page, 1).unwrap();
    assert_eq!(tb.pin, (1.5, 0.25));
    assert_eq!(tb.size, (1.0, 0.5));
    assert_eq!(tb.loc_pin, (0.5, 0.25));
    assert_eq!(tb.font.as_deref(), Some("Arial"));
    assert!((tb.font_size - 0.27778).abs() < 1e-12);
    assert_eq!(tb.color, "#FF0000");
    assert!(tb.bold && !tb.italic);
    assert_eq!(tb.halign, 2);
}

/// The block's PAGE rect (the editor overlay's footprint): a
/// default block on an unrotated top-level shape is the shape's
/// own rect; an explicit smaller block lands pin-anchored
/// (GitHub #22).
#[test]
fn text_block_page_rect_maps_local_block() {
    let (doc, page) = page_fixture(vec![shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0)]);
    let (x, y, w, h) = text_block_page_rect(&doc, &page, 1).unwrap();
    // Shape rect: pin (4,5) center → (3, 4.5)..(5, 5.5), y-up.
    assert!(
        (x - 3.0).abs() < 1e-9 && (y - 4.5).abs() < 1e-9,
        "x={x} y={y}"
    );
    assert!((w - 2.0).abs() < 1e-9 && (h - 1.0).abs() < 1e-9);

    // Off-center pin: TxtPin (1.5, 0.25) with a centered 1×0.5
    // block → shape-local (1.0, 0.0)..(2.0, 0.5) → page
    // (4.0, 4.5)..(5.0, 5.0) (pin − LocPin = (3.0, 4.5) offset).
    let mut sh = shape_at(2, (4.0, 5.0), 2.0, 1.0, 0.0);
    for (n, v) in [
        ("TxtPinX", 1.5),
        ("TxtPinY", 0.25),
        ("TxtWidth", 1.0),
        ("TxtHeight", 0.5),
    ] {
        sh.sheet.set_cell(n, Cell::value(format!("{v}")));
    }
    let (doc, page) = page_fixture(vec![sh]);
    let (x, y, w, h) = text_block_page_rect(&doc, &page, 2).unwrap();
    assert!(
        (x - 4.0).abs() < 1e-9 && (y - 4.5).abs() < 1e-9,
        "x={x} y={y}"
    );
    assert!((w - 1.0).abs() < 1e-9 && (h - 0.5).abs() < 1e-9);
}

/// #21: measured wrap keeps the authored line "Is implanted or"
/// on one line — real per-glyph advances at 8 pt measure ≈0.70 in
/// against the 0.8159 in block, where the 0.5 em guess capped the
/// line at 14 characters and split it. Since #144 the third line's
/// leading space survives too.
#[test]
fn measured_wrap_keeps_the_utilisation_1065_lines() {
    let doc = Document::default();
    let s = shape_1065();
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    let lines = text_lines(&out);
    assert_eq!(
        lines,
        vec!["If allograft", "Is implanted or", " opened"],
        "the three authored lines survive character-exact: {out}"
    );
}

/// #144: lines are character-exact — a paragraph's leading space
/// run survives (first paragraph included: only the trailing end
/// of the block is trimmed), interior runs stay, a whitespace-only
/// paragraph emits its spaces, an empty paragraph stays an empty
/// line, and the emitted `<text>` carries xml:space="preserve" so
/// the rasterizer keeps the spacing.
#[test]
fn wrap_is_character_exact_on_spaces() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("2.0"));
    {
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        assert!(out.is_empty(), "no text, no text element: {out}");
    }

    // First-paragraph leading run, interior run, whitespace-only
    // paragraph, empty paragraph. The final paragraph's trailing
    // space is trimmed with the block's end (corpus texts end with
    // CRLF; that trailing break must not grow the line count).
    s.sheet.text = Some("  opened\r\na  b\n   \n\nend ".into());
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert_eq!(
        text_lines(&out),
        vec!["  opened", "a  b", "   ", "", "end"],
        "every authored space survives into its line: {out}"
    );
    assert!(
        out.contains(r#"<text text-anchor="middle" xml:space="preserve""#),
        "the text element carries xml:space=preserve: {out}"
    );
}

/// #257 text-direction: the paragraph base derivation — explicit
/// U+202A/U+202B overrides decide before the first strong
/// character; neutrals and numbers never decide.
#[test]
fn paragraph_base_follows_marks_then_first_strong() {
    assert!(!paragraph_base_rtl("hello world"), "latin → LTR");
    assert!(
        paragraph_base_rtl("\u{0633}\u{0644}\u{0627}"),
        "arabic → RTL"
    );
    assert!(paragraph_base_rtl("\u{202B}hello"), "RLE override → RTL");
    assert!(
        !paragraph_base_rtl("\u{202A}\u{0633}\u{0644}\u{0627}"),
        "LRE overrides arabic"
    );
    assert!(
        !paragraph_base_rtl("123 (x)"),
        "numbers+neutrals → LTR default"
    );
    assert!(!paragraph_base_rtl("   "), "all-neutral → LTR default");
}

/// PR #269 review: RTL paragraphs measure with the RUN-PAIRED
/// faces (see textmetrics::split_runs) — in this test binary the
/// vendored fallbacks are deliberately unregistered (deterministic
/// pins), so every paragraph measures as ONE run through the same
/// single-face path as before: the wrap's output for pure-Latin
/// and pure-Persian paragraphs is unchanged. (#282 grew the
/// fallback to the Vazirmatn→Noto chain; the assertion covers the
/// whole chain at once.)
#[test]
fn rtl_wrap_stays_single_face_without_a_registered_fallback() {
    let latin = crate::textmetrics::resolve(None, false, false);
    let fallback = crate::textmetrics::resolve(Some("Vazirmatn"), false, false);
    let noto = crate::textmetrics::resolve(Some("Noto Sans SC"), false, false);
    let alef = '\u{0627}';
    assert!(
        !fallback.covers(alef),
        "unregistered vazirmatn covers nothing"
    );
    let chain: Vec<&crate::textmetrics::ResolvedFace> = vec![&fallback, &noto];
    let fa = "\u{0633}\u{0644}\u{0627}\u{0645}";
    for para in [format!("\u{202B}{fa}"), format!("\u{202B}abc {fa}")] {
        let lines = wrap_block_text(
            &latin, &para, 0.13889, 2.0, &latin, 0.13889, &chain, 0.0, 0.0,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].rtl, "the base is rtl");
        // No covering fallback → one run → width equals the plain
        // single-face measurement the wrap always used.
        let w = lines[0].text.clone();
        let expect = latin.width_in(&w, 0.13889);
        let got = latin.width_in_runs(&chain, &w, 0.13889);
        assert!(
            (expect - got).abs() < 1e-9,
            "single-run text keeps byte-identical widths: {expect} vs {got}"
        );
    }
}

/// #257 text-direction (PR #269 review): the Right-to-Left Text
/// toggle round-trips — on marks every paragraph, off strips the
/// marks AND pins still-RTL paragraphs with an explicit LRE so
/// "off" is really LTR (the trim-then-ask direction test in the
/// old dc-ui toggle could never see its own mark).
#[test]
fn rtl_toggle_round_trips_and_pins_natural_rtl_ltr() {
    // Latin: on marks every paragraph (CRLF normalizes), off
    // restores the original bytes.
    let on = toggle_rtl_marks("ab\r\ncd", true);
    assert_eq!(on, "\u{202B}ab\n\u{202B}cd");
    assert_eq!(toggle_rtl_marks(&on, false), "ab\ncd");
    // Naturally-RTL text: off strips the RLE and pins an LRE —
    // without the pin the first-strong scan would keep it RTL and
    // the toggle would be a visual no-op.
    let fa = "\u{0633}\u{0644}\u{0627}\u{0645}";
    let marked = format!("\u{202B}{fa}");
    assert_eq!(toggle_rtl_marks(&marked, false), format!("\u{202A}{fa}"));
    // Toggling back on replaces the pin with the RLE.
    assert_eq!(
        toggle_rtl_marks(&format!("\u{202A}{fa}"), true),
        format!("\u{202B}{fa}")
    );
    // An empty paragraph never gains a pin when switching off.
    assert_eq!(toggle_rtl_marks("\u{202B}", false), "");
    // An EMPTY shape pre-sets its direction: the bare mark is the
    // whole text and the first typed paragraph inherits the base.
    assert_eq!(toggle_rtl_marks("", true), "\u{202B}");
    assert_eq!(toggle_rtl_marks("", false), "");
    // The trailing terminator's empty paragraph takes NO mark —
    // a marked empty tail is content to the wrapper and would add
    // a TEXTHEIGHT line to "hello\r\n" (#148 gold, PR #269 review).
    assert_eq!(toggle_rtl_marks("hello\r\n", true), "\u{202B}hello\n");
    assert_eq!(
        toggle_rtl_marks("\u{202B}hello\n", false),
        "hello\n",
        "off restores the plain trailing break"
    );
}

/// #257 text-direction: a paragraph whose base is RTL wraps with
/// the complex-script pair and emits the isolate-wrapped line at
/// its visual right edge (start alignment), with direction=ltr
/// pinned on the element.
#[test]
fn rtl_paragraph_wraps_and_emits_isolated_at_visual_right() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("3.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("3.0"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![("HorzAlign".into(), Cell::value("0"))], // start = reading order
            ..Row::default()
        }],
        ..Section::default()
    }));
    s.sheet.text = Some(
        "\u{0633}\u{0644}\u{0627}\u{0645} \u{0639}\u{0644}\u{06CC}\u{06A9}\u{0645}".into(),
    );
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 3.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    // The isolate pair wraps the line content.
    assert!(
        out.contains('\u{2067}'),
        "the RTL line carries an RLI: {out}"
    );
    assert!(
        out.contains('\u{2069}'),
        "the RTL line carries a PDI: {out}"
    );
    // direction=ltr pins the element base for attribute-honoring viewers.
    assert!(
        out.contains(r#"direction="ltr""#),
        "element base pinned: {out}"
    );
    // Start alignment under an RTL base puts the line's visual
    // left edge at (right edge - shaped width), NOT at left_x —
    // the tspan x must sit right of the block's left inset edge.
    let x: f64 = out
        .split(r#"<tspan x=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a tspan x");
    // Block pin (1.5,0.5) centered: left edge = tx - lpx + ins_l
    // = 1.5 - 1.5 + 0.1 = 0.1; the RTL line's visual left must be
    // strictly inside the block (right_x - lw > 0.1).
    assert!(x > 0.1, "RTL start line sits at its visual right: x={x}");
    // A second shape with the SAME text but end alignment (2)
    // anchors the line at the block's LEFT edge instead.
    if let Some(SheetChild::Section(sec)) = s.sheet.children.last_mut() {
        sec.rows[0].cells[0] = ("HorzAlign".into(), Cell::value("2"));
    }
    let chain = [&s];
    let mut out2 = String::new();
    emit_text(
        &doc, &s, &chain, 3.0, 1.0, 1.0, 1.0, false, None, None, &mut out2,
    );
    let x2: f64 = out2
        .split(r#"<tspan x=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a tspan x");
    assert!(
        (x2 - 0.1).abs() < 1e-6,
        "RTL end line anchors at the visual left inset edge: x2={x2}"
    );
    // PR #269 review: the end-aligned line carries its OWN start
    // anchor (the inherited element "end" would push the glyphs
    // left of the block), and the tspan's font-family is the
    // complete attribute — the old double wrapper nested the
    // quotes. Both are pinned by parsing the emission through the
    // same usvg the rasterizer runs.
    assert!(
        out2.contains(r#" text-anchor="start""#),
        "the end-aligned RTL line anchors itself start: {out2}"
    );
    let full = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10">{out2}</svg>"#
    );
    // usvg's roxmltree stage rejects malformed attributes — the
    // nested-quote font-family this fixes failed here.
    resvg::usvg::Tree::from_str(&full, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("RTL emission parses as well-formed SVG: {e}\n{full}"));
}

/// #257 text-direction (PR #269 review): RTL lines under a larger
/// ComplexScriptSize take proportionally taller slots — with
/// default 1.2 em spacing and 2 em glyphs, the flat line height
/// overlapped adjacent lines.
#[test]
fn larger_complex_script_lines_take_taller_slots() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("3.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("3.0"));
    s.sheet.set_cell("TxtHeight", Cell::value("1.2"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![("ComplexScriptSize".into(), Cell::value("-200"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    // Two RTL paragraphs (the explicit mark decides the base).
    let fa = "\u{0633}\u{0644}\u{0627}\u{0645}";
    s.sheet.text = Some(format!("{fa}\r\n{fa}"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 3.0, 1.2, 1.5, 0.6, false, None, None, &mut out,
    );
    let ys: Vec<f64> = out
        .split(r#" y=""#)
        .skip(1)
        .filter_map(|rest| rest.split('"').next().and_then(|v| v.parse().ok()))
        .collect();
    assert_eq!(ys.len(), 2, "two rtl lines: {out}");
    let (tb, _) = {
        let chain = [&s];
        let tb = resolve_text_block(&doc, &s, &chain, 3.0, 1.2);
        (tb, chain)
    };
    let slot = line_slot_height(&tb, true);
    let naive = tb.line_height;
    assert!(
        slot > naive * 1.5,
        "the double-size cs glyphs take a taller slot: {slot} vs {naive}"
    );
    assert!(
        (ys[1] - ys[0] - slot).abs() < 2e-3,
        "the second line advances by the taller slot: {} vs {slot}",
        ys[1] - ys[0]
    );
    // TEXTHEIGHT agrees with the painted slots (the refreshed
    // cache feeds TxtHeight the same sum).
    let m = crate::textfunc::SheetTextMeasure::new(&doc, &s, &[&s], format!("{fa}\r\n{fa}"));
    let h = {
        use vsdx::formula::TextMeasure as _;
        m.text_height(&format!("{fa}\r\n{fa}"), 3.0).unwrap()
    };
    let ins_tb = tb.insets.2 + tb.insets.3;
    assert!(
        (h - (2.0 * slot + ins_tb)).abs() < 1e-3,
        "TEXTHEIGHT sums the slots (+ insets): {h} vs {}",
        2.0 * slot + ins_tb
    );
}

/// PR #269 review: a SMALLER Latin line followed by a larger
/// complex-script line — the larger line's baseline must rise by
/// its own ascent, not advance by the Latin slot (which overlapped
/// the Latin glyphs).
#[test]
fn mixed_latin_then_larger_cs_lines_keep_their_ascents() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("3.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("3.0"));
    s.sheet.set_cell("TxtHeight", Cell::value("1.4"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![("ComplexScriptSize".into(), Cell::value("-200"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let fa = "\u{0633}\u{0644}\u{0627}\u{0645}";
    s.sheet.text = Some(format!("one\r\n\u{202B}{fa}"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 3.0, 1.4, 1.5, 0.7, false, None, None, &mut out,
    );
    let ys: Vec<f64> = out
        .split(r#" y=""#)
        .skip(1)
        .filter_map(|r| r.split('"').next().and_then(|v| v.parse().ok()))
        .collect();
    assert_eq!(ys.len(), 2, "one latin + one rtl line: {out}");
    let tb = resolve_text_block(&doc, &s, &chain, 3.0, 1.4);
    let size = tb.font_size;
    let delta = ys[1] - ys[0];
    // Latin slot (1.2 em) + (cs ascent − latin ascent) = 2.0 em —
    // the old flat advance was the Latin slot's 1.2 em.
    assert!(
        (delta - 2.0 * size).abs() < 2e-3,
        "the larger line rises by its own ascent: {delta} vs {}",
        2.0 * size
    );
}

/// #257 text-direction: LTR-only blocks keep the pre-#257 byte
/// form (no direction attribute, no isolates) — golden stability.
#[test]
fn ltr_blocks_emit_without_direction_marks() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("2.0"));
    s.sheet.text = Some("plain latin".into());
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(!out.contains("direction="), "no direction attr: {out}");
    assert!(!out.contains('\u{2067}'), "no isolates: {out}");
}

/// #257 text-direction: ComplexScriptFont/Size resolve through the
/// first Character row with the spec's zero/absent semantics —
/// a CS face rides the RTL tspan's font-family, and its size swaps
/// the glyph size when it differs.
#[test]
fn complex_script_cells_feed_the_rtl_line() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("3.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("3.0"));
    {
        let chain = [&s];
        let tb = resolve_text_block(&doc, &s, &chain, 3.0, 1.0);
        assert_eq!(tb.cs_font, None, "absent cell → None");
        assert_eq!(tb.cs_size, None, "absent size → None (Size fallback)");
    }
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![
                ("ComplexScriptFont".into(), Cell::value("Vazirmatn")),
                ("ComplexScriptSize".into(), Cell::value("0.2")),
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let (font, size) = {
        let chain = [&s];
        let tb = resolve_text_block(&doc, &s, &chain, 3.0, 1.0);
        (tb.cs_font, tb.cs_size)
    };
    assert_eq!(font.as_deref(), Some("Vazirmatn"));
    assert_eq!(size, Some(0.2));
    // The negative-size semantics: <0 = |v|·Size/100.
    let size0 = {
        let chain = [&s];
        resolve_text_block(&doc, &s, &chain, 3.0, 1.0).font_size
    };
    if let Some(SheetChild::Section(sec)) = s.sheet.children.last_mut() {
        sec.rows[0].cells[1] = ("ComplexScriptSize".into(), Cell::value("-150"));
    }
    let size = {
        let chain = [&s];
        resolve_text_block(&doc, &s, &chain, 3.0, 1.0).cs_size
    };
    assert_eq!(size, Some(1.5 * size0), "negative → |v|·Size/100");
}

/// #144 review: when a block is too narrow for `"a "` and `" b"`,
/// the wrap breaks inside the `"a  b"` space run — the surviving
/// space is a line of its own, not an empty line and not a dropped
/// character.
#[test]
fn narrow_wrap_breaking_in_a_space_run_keeps_the_space() {
    let doc = Document::default();
    let size = 0.13889; // the text-block default (10 pt)
    let face = crate::textmetrics::resolve(None, false, false);
    let usable = face.width_in("a", size) + 0.4 * face.width_in(" ", size);
    let mut s = Shape::default();
    s.sheet.text = Some("a  b".into());
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet
        .set_cell("TxtWidth", Cell::value(format!("{usable:.6}")));
    s.sheet.set_cell("LeftMargin", Cell::value("0"));
    s.sheet.set_cell("RightMargin", Cell::value("0"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert_eq!(
        text_lines(&out),
        vec!["a", " ", "b"],
        "the surviving space is its own line: {out}"
    );
}

/// #21: a run that genuinely overflows still wraps — measurement
/// replaces the guess, it does not disable wrapping.
#[test]
fn measured_wrap_splits_genuinely_overflowing_runs() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.text = Some("considerable label text beyond the block".into());
    s.sheet.set_cell("Width", Cell::value("0.8"));
    s.sheet.set_cell("Height", Cell::value("1.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("0.8"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    let lines = text_lines(&out);
    assert!(lines.len() > 1, "must wrap: {lines:?}");
    // The greedy invariant: every line except the last is full —
    // adding the next word would overflow.
    let usable = 0.8 - 0.1 - 0.1;
    let face = crate::textmetrics::resolve(None, false, false);
    for pair in lines.windows(2) {
        let joined = format!("{} {}", pair[0], pair[1]);
        assert!(
            face.width_in(&joined, 0.13889) > usable,
            "line {:?} wrapped too early (usable {usable}): {lines:?}",
            pair[0]
        );
    }
}

/// #21: a single word wider than the block hard-splits at the
/// longest fitting prefix (never a zero-length line, never a
/// stuck loop).
#[test]
fn measured_wrap_hard_splits_single_long_words() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.text = Some("aaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    s.sheet.set_cell("Width", Cell::value("0.5"));
    s.sheet.set_cell("TxtWidth", Cell::value("0.5"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    let lines = text_lines(&out);
    assert!(
        lines.len() > 1,
        "a 26-em-ish run cannot fit 0.3 in: {lines:?}"
    );
    assert!(
        lines.iter().all(|l| !l.is_empty()),
        "hard splits keep every line non-empty: {lines:?}"
    );
    let joined: String = lines.concat();
    assert_eq!(joined, "aaaaaaaaaaaaaaaaaaaaaaaaaa", "no characters lost");
}

/// The emergency hard-split's boundary rules run to a FIXPOINT:
/// glue (U+00A0) may move the split past the position the
/// forbidden-head pass already cleared, which used to strand a
/// closer at the head of the rest — `图⍽，！色` must never open a
/// line with `，` or `！` (CodeRabbit/Greptile round 5).
#[test]
fn emergency_split_never_opens_a_line_with_a_closer() {
    let face = crate::textmetrics::resolve(None, false, false);
    // Narrower than any glyph: every token takes the emergency
    // path.
    let lines = wrap_block_text(
        &face,
        "图\u{00A0}，！色",
        0.13889,
        0.05,
        &face,
        0.13889,
        &[],
        0.0,
        0.0,
    );
    let joined: String = lines.iter().map(|l| l.text.clone()).collect();
    assert_eq!(joined, "图\u{00A0}，！色", "no characters lost");
    assert!(lines.len() >= 2, "the emergency splitter fired");
    for l in &lines {
        let first = l.text.chars().next().expect("lines are non-empty");
        assert!(
            !crate::textmetrics::line_head_forbidden(first),
            "no line opens with a forbidden closer: {:?}",
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>()
        );
    }
}

/// The emergency splitter keeps a combining mark with its base:
/// `カ` + U+3099 never splits between them, without consuming
/// the following `ク` (CodeRabbit round 5).
#[test]
fn emergency_split_keeps_combining_marks_with_their_base() {
    let face = crate::textmetrics::resolve(None, false, false);
    let lines = wrap_block_text(
        &face,
        "カ\u{3099}ク",
        0.13889,
        0.05,
        &face,
        0.13889,
        &[],
        0.0,
        0.0,
    );
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["カ\u{3099}", "ク"],
        "the dakuten stays on its base; the break before ク survives"
    );
}

/// #282 ja (CodeRabbit round 2): the emergency splitter never
/// leaves a STANDALONE sound mark (ﾞ ﾟ — Lm class, glued only by
/// the kinsoku head rule) to open a line of over-long halfwidth
/// kana.
#[test]
fn emergency_split_never_opens_a_line_with_a_sound_mark() {
    let face = crate::textmetrics::resolve(None, false, false);
    let lines = wrap_block_text(
        &face,
        "ｱｲｶ\u{FF9E}ｸｻ",
        0.13889,
        0.05,
        &face,
        0.13889,
        &[],
        0.0,
        0.0,
    );
    let joined: String = lines.iter().map(|l| l.text.clone()).collect();
    assert_eq!(joined, "ｱｲｶ\u{FF9E}ｸｻ", "no characters lost");
    assert!(lines.len() >= 2, "the emergency splitter fired");
    for l in &lines {
        let first = l.text.chars().next().expect("lines are non-empty");
        assert!(
            !crate::textmetrics::line_head_forbidden(first),
            "no line opens with a forbidden starter: {:?}",
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>()
        );
    }
}

/// CRLF and lone CR each break exactly one line (carried over
/// from the guess era — the normalizer is unchanged).
#[test]
fn crlf_and_lone_cr_each_break_one_line() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.text = Some("one\r\ntwo\rthree".into());
    s.sheet.set_cell("Width", Cell::value("8.0"));
    s.sheet.set_cell("TxtWidth", Cell::value("8.0"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 8.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert_eq!(
        text_lines(&out),
        vec!["one", "two", "three"],
        "no empty paragraph between CRLF pairs"
    );
}

/// #22 review: the in-place editor anchors its caret on the
/// renderer's own first-baseline formula — pin the numbers so the
/// helper and `emit_text` can never drift (block 1 in tall, 10 pt
/// text, default insets, two wrapped lines of 0.1667 in each).
#[test]
fn text_first_baseline_matches_the_valign_bands() {
    let mut tb = TextBlock::default();
    tb.size.1 = 1.0;
    tb.font_size = 0.13889;
    tb.insets = (0.1, 0.1, 0.0555, 0.0555);
    let total = 2.0 * 0.16667;
    let ascent = 0.13889 * 0.8;
    tb.valign = 1; // top
    assert!((text_first_baseline(&tb, total, tb.font_size) - (0.0555 + ascent)).abs() < 1e-9);
    tb.valign = 2; // middle (default)
    assert!(
        (text_first_baseline(&tb, total, tb.font_size) - ((1.0 - total) / 2.0 + ascent)).abs()
            < 1e-9
    );
    tb.valign = 3; // bottom
    assert!(
        (text_first_baseline(&tb, total, tb.font_size) - (1.0 - 0.0555 - total + ascent)).abs()
            < 1e-9
    );
    // More wrapped lines raise the first line in the middle band.
    let two = text_first_baseline(&tb, total, tb.font_size);
    let four = text_first_baseline(&tb, total * 2.0, tb.font_size);
    assert!(four < two, "taller text starts its first line higher");
    // PR #269 review: a first line of larger complex-script glyphs
    // ascends from a higher baseline, and its slot (not the flat
    // line height) is what the next line advances by.
    tb.line_height = 0.16667;
    tb.cs_size = Some(tb.font_size * 2.0);
    let eff = effective_cs_size(&tb);
    assert_eq!(eff, tb.font_size * 2.0);
    assert_eq!(line_slot_height(&tb, true), tb.line_height * 2.0);
    assert_eq!(
        line_slot_height(&tb, false),
        tb.line_height,
        "ltr keeps the latin slot"
    );
    // A SMALLER complex-script size never shrinks the slot.
    tb.cs_size = Some(tb.font_size * 0.5);
    assert_eq!(line_slot_height(&tb, true), tb.line_height);
}

/// #232 stock-font rule: a family outside Visio's stock set
/// resolves the CALIBRI chain outright (named first, Carlito
/// substituted — Visio's own SVG renames unknown families
/// Calibri), while stock families keep their own chain and our
/// infrastructure faces stay themselves.
#[test]
fn non_stock_fonts_resolve_the_calibri_chain() {
    for name in [
        "Roboto",
        "Helvetica",
        "Futura",
        "Fira Mono",
        "Certainly Not Installed",
    ] {
        let chain = family_chain(Some(name));
        assert_eq!(
            chain,
            vec![
                "Calibri".to_string(),
                "Carlito".to_string(),
                "sans-serif".to_string()
            ],
            "{name} lays out in Calibri (#232): {chain:?}"
        );
    }
    // Stock families keep their own chain (Arial still aliases to
    // its metric twin).
    assert_eq!(
        family_chain(Some("Arial"))[..2],
        ["Arial".to_string(), "Liberation Sans".to_string()]
    );
    assert_eq!(family_chain(Some("Calibri"))[0], "Calibri");
    // The app infrastructure faces stay named (#232 exemption) —
    // the RTL fallback must never regress to Calibri.
    assert_eq!(family_chain(Some("Vazirmatn"))[0], "Vazirmatn");
    assert_eq!(
        family_chain(Some("Liberation Serif"))[0],
        "Liberation Serif"
    );
    // No Font cell: Visio's default family is Calibri anyway.
    assert_eq!(family_chain(None)[0], "Calibri");
}

/// #230: Character.Pos 1/2 draws at the measured reduced em and
/// shifts the baseline (Carlito inherits Calibri's 0.667 /
/// +0.3 / −0.14 numbers); an unwitnessed family keeps the
/// nominal draw. The LINE SLOTS stay nominal — TEXTHEIGHT
/// retains the nominal size.
#[test]
fn character_pos_scripts_the_block() {
    let emit_pos = |pos: i32, font: Option<&str>| {
        let doc = Document::default();
        let mut s = Shape::default();
        s.sheet.text = Some("H2O".into());
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Character".into(),
            rows: vec![Row {
                cells: font
                    .map(|f| {
                        vec![
                            ("Pos".to_string(), Cell::value(pos.to_string())),
                            ("Font".to_string(), Cell::value(f)),
                        ]
                    })
                    .unwrap_or_else(|| vec![("Pos".to_string(), Cell::value(pos.to_string()))]),
                ..Row::default()
            }],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        out
    };
    // Baseline: no Pos cell → nominal draw, no shift.
    let plain = emit_pos(0, None);
    assert!(
        plain.contains(r#"font-size="0.1389""#),
        "nominal size: {plain}"
    );
    // Superscript on the default (Calibri) family: 2/3 em draw,
    // baseline raised 0.3 em from the nominal baseline.
    let sup = emit_pos(1, None);
    assert!(
        sup.contains(r#"font-size="0.0926""#),
        "superscript draws at 0.667 × 10 pt: {sup}"
    );
    let sup_y: f64 = sup
        .split(r#" y=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a y coordinate in the emitted text");
    let plain_y: f64 = plain
        .split(r#" y=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a y coordinate in the emitted text");
    assert!(
        (plain_y - sup_y - 0.3 * 0.13888).abs() < 5e-4,
        "superscript raises the baseline 0.3 em: {plain_y} vs {sup_y}"
    );
    // Subscript lowers by 0.14 em (both measured families).
    let sub = emit_pos(2, None);
    let sub_y: f64 = sub
        .split(r#" y=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a y coordinate");
    assert!(
        (sub_y - plain_y - 0.14 * 0.13888).abs() < 5e-4,
        "subscript lowers the baseline 0.14 em: {sub_y} vs {plain_y}"
    );
    // Arial's witnessed numbers differ (1331/2048, 0.477 em
    // raise) — Liberation Sans inherits them.
    let sup_arial = emit_pos(1, Some("Arial"));
    let arial_y: f64 = sup_arial
        .split(r#" y=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a y coordinate");
    assert!(
        (plain_y - arial_y - 0.477 * 0.13888).abs() < 5e-4,
        "Arial superscripts raise 0.477 em: {arial_y} vs {plain_y}"
    );
    // An unwitnessed stock family keeps the nominal draw.
    let times = emit_pos(1, Some("Times New Roman"));
    assert!(
        times.contains(r#"font-size="0.1389""#),
        "Times New Roman has no witnessed Pos metrics: {times}"
    );
}

/// #229: TextBkgnd paints ONE rectangle spanning the laid-out
/// lines — above geometry, below text — through the +1 palette
/// encoding (1–24 → palette 0–23), the 0/255 no-paint rule, the
/// fractional transparency (V=0.5 = 50 %), and the corpus's theme-default formula
/// (falls to its cached V like every THEMEVAL we don't model).
#[test]
fn text_bkgnd_paints_the_laid_out_extent() {
    let emit_bkgnd = |bkgnd: &str, trans: Option<&str>| {
        let doc = Document::default();
        let mut s = Shape::default();
        s.sheet.text = Some("boxed".into());
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.set_cell("TextBkgnd", Cell::value(bkgnd));
        if let Some(tr) = trans {
            s.sheet.set_cell("TextBkgndTrans", Cell::value(tr));
        }
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        out
    };
    // 0 and 255 paint nothing.
    assert!(!emit_bkgnd("0", None).contains("<rect"));
    assert!(!emit_bkgnd("255", None).contains("<rect"));
    // 1 → palette entry 0 (black); 5 → palette entry 4 (blue).
    let black = emit_bkgnd("1", None);
    assert!(
        black.contains(r##"fill="#000000""##) && black.contains("<rect"),
        "TextBkgnd 1 is palette entry 0: {black}"
    );
    assert!(
        emit_bkgnd("5", None).contains(r##"fill="#0000ff""##),
        "TextBkgnd 5 is palette entry 4"
    );
    // 50 % transparency renders as 128/255 ≈ 0.502 alpha.
    let half = emit_bkgnd("1", Some("0.5"));
    assert!(
        half.contains(r#"fill-opacity="0.502""#),
        "50 % trans → 128/255 alpha: {half}"
    );
    // The corpus's theme-default formula: THEMEGUARD(
    // THEMEVAL("BackgroundColor")+1) with cached V=#ffffff — the
    // un-modeled THEMEVAL falls to the cached V (white).
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.text = Some("boxed".into());
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell(
        "TextBkgnd",
        Cell {
            v: Some("#ffffff".into()),
            f: Some(r#"THEMEGUARD(THEMEVAL("BackgroundColor")+1)"#.into()),
            ..Cell::default()
        },
    );
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(
        out.contains(r##"fill="#ffffff""##),
        "the theme-default label paints its cached white: {out}"
    );
    // The rect sits BEFORE the <text> (below text, above the
    // geometry emitted earlier).
    let rect_at = out.find("<rect").expect("a rect");
    let text_at = out.find("<text").expect("a text");
    assert!(rect_at < text_at, "background below text: {out}");
    // Geometry (PR review wave 4, center-edge fix PR #284): the
    // rect spans the LINES — its y is the FIRST LINE SLOT's top
    // (middle-aligned here: top_edge −1 + (th − total_h)/2 =
    // −0.5833), and its x is the centered line's LEFT edge:
    // band [0.1, 1.9] centers at 1.0, so the text (0.342 wide)
    // starts at 0.829 — the background sits BEHIND the text, not
    // at the left margin.
    let rect = &out[rect_at..text_at];
    assert!(
        rect.contains(r#"x="0.8290""#),
        "rect behind the centered text: {rect}"
    );
    assert!(
        rect.contains(r#"y="-0.5833""#),
        "rect at the first line slot: {rect}"
    );
    let rect_w: f64 = rect
        .split(r#"width=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a rect width");
    assert!(
        (rect_w - 0.3420).abs() < 2e-3,
        "the rect spans the widest laid-out line: {rect_w}"
    );
    // An all-RTL block (explicit RLE) paints its background too —
    // the span joins the union from the RTL branch (PR review).
    let mut s = Shape::default();
    s.sheet.text = Some("\u{202B}\u{0633}\u{0644}\u{0627}\u{0645}".into());
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("TextBkgnd", Cell::value("1"));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(
        out.contains(r##"fill="#000000""##) && out.contains("<rect"),
        "an all-RTL block paints its text background: {out}"
    );
}

/// #231: a bulleted paragraph's text starts at the computed tab
/// stop (bullet glyph as a path, then the text left-anchored at
/// the stop); TextPosAfterBullet only applies when it lies past
/// the bullet's start (the measured Visio-2021 rule), and a
/// custom bullet string draws as text.
#[test]
fn bullets_place_text_at_the_tab_stop() {
    let emit_bullet = |bullet: &str, extra: &[(&str, &str)]| {
        let doc = Document::default();
        let mut s = Shape::default();
        s.sheet.text = Some("bulleted".into());
        s.sheet.set_cell("Width", Cell::value("4.0"));
        // Zero L/R margins: the text-area left edge sits at 0, so
        // the tab-stop offsets below assert in TEXT-AREA
        // coordinates exactly as the #231 witness reports them.
        s.sheet.set_cell("LeftMargin", Cell::value("0"));
        s.sheet.set_cell("RightMargin", Cell::value("0"));
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Paragraph".into(),
            rows: vec![Row {
                cells: {
                    let mut cells = vec![
                        ("Bullet".to_string(), Cell::value(bullet)),
                        ("HorzAlign".to_string(), Cell::value("0")),
                    ];
                    for (n, v) in extra {
                        cells.push((n.to_string(), Cell::value(*v)));
                    }
                    cells
                },
                ..Row::default()
            }],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 4.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        out
    };
    // Built-in style 1 draws a PATH dot (never a text glyph), at
    // the line start (bullet offset 0 with zero margins).
    let out = emit_bullet("1", &[]);
    assert!(
        out.contains(r#"<path transform="translate(0.0000,"#),
        "path bullet: {out}"
    );
    // The path is a SIBLING of <text>, not a child (PR #284
    // review: SVG's text content model has no place for shapes —
    // usvg drops a <path> inside <text>, and the bullet would
    // reserve its gap but paint nothing).
    let path_at = out.find("<path").expect("a path");
    let text_open_at = out.find("<text").expect("a text");
    let text_close_at = out.rfind("</text>").expect("the text close");
    assert!(
        path_at < text_open_at || path_at > text_close_at,
        "the bullet path must live outside the text element: {out}"
    );
    // The text starts AT the tab stop — left-anchored past the
    // bullet (DefaultTabStop 0.5 default).
    let tx: f64 = out
        .split(r#"<tspan x=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a tspan x");
    assert!(
        (tx - 0.5).abs() < 1e-3,
        "after-bullet text sits on the 0.5 tab stop: {tx}"
    );
    // The measured rule (the #231 witness verbatim): IndFirst
    // 0.25 with TextPosAfterBullet 0.1 — TPAB does not lie past
    // the bullet's start (0.25), so it is rejected and the next
    // tab stop (0.5) wins; Visio does not add TPAB to IndFirst
    // (that would read 0.35). TextPosAfterBullet 0.6 past the
    // bullet's start wins outright.
    let early: f64 = emit_bullet("1", &[("IndFirst", "0.25"), ("TextPosAfterBullet", "0.1")])
        .split(r#"<tspan x=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a tspan x");
    assert!(
        (early - 0.5).abs() < 1e-3,
        "0.1 < the stop → tab wins: {early}"
    );
    let late: f64 = emit_bullet("1", &[("TextPosAfterBullet", "0.6")])
        .split(r#"<tspan x=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .and_then(|v| v.parse().ok())
        .expect("a tspan x");
    assert!(
        (late - 0.6).abs() < 1e-3,
        "0.6 past the stop → wins: {late}"
    );
    // A custom bullet string stays TEXT in its own face.
    let custom = emit_bullet("–", &[]);
    assert!(
        custom.contains(">&ndash;</tspan>") || custom.contains(">\u{2013}</tspan>"),
        "custom bullets stay text: {custom}"
    );
    // Bullet 0: no bullet machinery at all.
    assert!(!emit_bullet("0", &[]).contains("<path"));

    // The centered paragraph's custom bullet: the element anchor
    // is middle, the bullet tspan must override to start.
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.text = Some("bulleted".into());
    s.sheet.set_cell("Width", Cell::value("4.0"));
    s.sheet.set_cell("LeftMargin", Cell::value("0"));
    s.sheet.set_cell("RightMargin", Cell::value("0"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![
                ("Bullet".to_string(), Cell::value("–")),
                // NO HorzAlign cell → the default center.
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 4.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    // The element anchor is middle; only the bullet tspan pins
    // "start" (the main text tspan's override carries no
    // font-size, so this substring matches the bullet alone).
    assert!(
        out.contains(r#"text-anchor="start" font-size"#),
        "the centered custom bullet pins its own start anchor: {out}"
    );
}

/// Final local CodeRabbit pass: numeric Bullet values that fail
/// the style filter are UNSUPPORTED STYLES, not custom text —
/// "-1"/"1.5" arm nothing (the old filter conflated them with
/// non-numeric strings and painted the literal as a glyph).
#[test]
fn unsupported_numeric_bullets_stay_off() {
    let doc = Document::default();
    let emit = |bullet: &str| {
        let mut s = Shape::default();
        s.sheet.text = Some("bulleted".into());
        s.sheet.set_cell("Width", Cell::value("4.0"));
        s.sheet.set_cell("LeftMargin", Cell::value("0"));
        s.sheet.set_cell("RightMargin", Cell::value("0"));
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Paragraph".into(),
            rows: vec![Row {
                cells: vec![
                    ("Bullet".to_string(), Cell::value(bullet)),
                    ("HorzAlign".to_string(), Cell::value("0")),
                ],
                ..Row::default()
            }],
            ..Section::default()
        }));
        let chain = [&s];
        let mut out = String::new();
        emit_text(
            &doc, &s, &chain, 4.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
        );
        out
    };
    for v in ["-1", "1.5", "256", "0.0"] {
        let out = emit(v);
        assert!(
            !out.contains(r#"<path transform="translate("#),
            "{v} draws no round-set path bullet: {out}"
        );
        // …and no tab stop is reserved — the text starts at the
        // margin like an unbULLETED block.
        let tx: f64 = out
            .split(r#"<tspan x=""#)
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .and_then(|x| x.parse().ok())
            .unwrap_or(-1.0);
        assert!((tx).abs() < 1e-3, "{v} reserves no bullet tab stop: x={tx}");
    }
    // A genuinely non-numeric string still arms the custom bullet.
    let out = emit("\u{2022}");
    assert!(
        out.contains(r#"<path transform="translate("#) || out.contains('\u{2022}'),
        "a custom string bullet still draws"
    );
}

/// #235 terminal-NBSP hang: a trailing no-break space never
/// wraps its word — the overflow check measures the line without
// the NBSP's advance, while the NBSP stays in the line content.
#[test]
fn terminal_nbsp_hangs_past_the_margin() {
    let latin = crate::textmetrics::resolve(None, false, false);
    let fallback = crate::textmetrics::resolve(Some("Vazirmatn"), false, false);
    // Grow a word until word+NBSP just exceeds a 0.5 in line
    // while the word alone still fits: the hang rule must keep
    // that word whole (the NBSP paints past the margin), where
    // the plain measurement would hard-split it.
    let mut word = String::from("m");
    while latin.width_in(&format!("{word}\u{a0}"), 0.13889) <= 0.5 {
        word.push('n');
    }
    assert!(
        latin.width_in(&word, 0.13889) <= 0.5,
        "fixture: the word alone fits the margin"
    );
    let chain: Vec<&crate::textmetrics::ResolvedFace> = vec![&fallback];
    let solo = wrap_block_text(
        &latin,
        &format!("{word}\u{a0}"),
        0.13889,
        0.5,
        &latin,
        0.13889,
        &chain,
        0.0,
        0.0,
    );
    assert_eq!(
        solo.len(),
        1,
        "word+NBSP never hard-splits on the hanging advance: {:?}",
        solo.iter().map(|l| &l.text).collect::<Vec<_>>()
    );
    assert_eq!(
        solo[0].text,
        format!("{word}\u{a0}"),
        "the NBSP stays content"
    );
    let lines = wrap_block_text(
        &latin,
        &format!("{word}\u{a0} next"),
        0.13889,
        0.5,
        &latin,
        0.13889,
        &chain,
        0.0,
        0.0,
    );
    assert_eq!(
        lines[0].text,
        format!("{word}\u{a0}"),
        "the NBSP binds its word onto the first line (#235)"
    );
    // And a plain space there DOES wrap.
    let lines_plain = wrap_block_text(
        &latin,
        &format!("{word} next"),
        0.13889,
        0.5,
        &latin,
        0.13889,
        &chain,
        0.0,
        0.0,
    );
    assert_eq!(
        lines_plain[0].text, word,
        "the control: a plain trailing space wraps"
    );
}

/// #282 zh-Hans: CJK paragraphs wrap between ideographs under the
/// kinsoku rules — no spaces needed — and a closing punct never
/// STARTS a line. The measurement face is irrelevant to the break
/// STRUCTURE (Carlito's missing-glyph advances are uniform here);
/// the tokenizer owns the opportunities, so the unregistered test
/// world pins them fine.
#[test]
fn cjk_paragraph_wraps_between_ideographs_with_kinsoku() {
    let latin = crate::textmetrics::resolve(None, false, false);
    let fallback = crate::textmetrics::resolve(Some("Vazirmatn"), false, false);
    let chain: Vec<&crate::textmetrics::ResolvedFace> = vec![&fallback];
    let size = 0.13889;
    let w1 = latin.width_in("图", size);
    assert!(w1 > 0.0);
    // A width fitting two ideographs: the paragraph breaks every
    // two glyphs — between ideographs, never inside one.
    let lines = wrap_block_text(
        &latin,
        "图表形状",
        size,
        w1 * 2.5,
        &latin,
        size,
        &chain,
        0.0,
        0.0,
    );
    assert_eq!(
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["图表", "形状"],
        "ideograph boundaries are the break opportunities"
    );
    // Kinsoku across widths: the sentence enders stay glued to
    // the token they close and never open a line.
    for usable in [w1 * 1.2, w1 * 1.8, w1 * 2.5] {
        let lines = wrap_block_text(
            &latin,
            "画图，上色。",
            size,
            usable,
            &latin,
            size,
            &chain,
            0.0,
            0.0,
        );
        for l in &lines {
            assert!(
                !l.text.starts_with(['，', '。']),
                "a closing punct never starts a line: {:?}",
                l.text
            );
        }
    }
    // The Latin path is untouched: the same engine, pure spaces.
    let lines = wrap_block_text(
        &latin,
        "ab cd",
        size,
        latin.width_in("ab", size) * 1.2,
        &latin,
        size,
        &chain,
        0.0,
        0.0,
    );
    assert_eq!(
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["ab", "cd"],
        "Latin keeps the space-driven breaks"
    );
}

/// #234: the composed width quantizes the size then rounds every
/// glyph advance onto the same grid — for a single glyph that is
/// exactly one grid-rounded product, and the terminal-run space
/// + slack make TEXTWIDTH strictly wider than the bare advance.
#[test]
fn composed_width_rounds_on_the_size_grid() {
    let f = crate::textmetrics::resolve(None, false, false);
    let size = 0.13889;
    let q = crate::textmetrics::quantize18(size);
    let adv_em = f.width_in("x", 1.0);
    let expect = (q * adv_em * 262_144.0).round() / 262_144.0;
    let got = f.composed_width_in("x", size);
    assert!(
        (got - expect).abs() < 1e-12,
        "one glyph: {got} vs grid-rounded {expect}"
    );
    // The space advance itself lands on the grid.
    let sp = f.space_advance_in(size);
    assert!(
        (sp * 262_144.0 - (sp * 262_144.0).round()).abs() < 1e-9,
        "the terminal space is a grid step"
    );
}

// ---- #267 field substitution ----

/// The corpus's common shape: a master-root Field row referenced
/// by a self-closing marker on an instanced shape, with the
/// User row the Value formula reads living on the SAME sheet.
#[test]
fn field_marker_renders_the_substituted_value() {
    let mut doc = Document::default();
    let mut m = vsdx::Master {
        id: 2,
        name_u: Some("Card".into()),
        ..vsdx::Master::default()
    };
    let mut root = Shape {
        id: 1,
        ..Shape::default()
    };
    root.sheet.set_cell("Width", Cell::value("2.0"));
    root.sheet.set_cell("Height", Cell::value("1.0"));
    root.sheet.children.push(SheetChild::Section(Section {
        name: "User".into(),
        rows: vec![Row {
            n: Some("Label".into()),
            cells: vec![("Value".to_string(), Cell::value("Hello"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    // The Field row, in the corpus's verbatim cell shape.
    root.sheet.children.push(SheetChild::Section(Section {
        name: "Field".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![
                (
                    "Value".to_string(),
                    Cell {
                        v: Some(String::new()),
                        u: Some("STR".into()),
                        f: Some("User.Label".into()),
                        ..Cell::default()
                    },
                ),
                (
                    "Format".to_string(),
                    Cell {
                        v: Some("esc(0)".into()),
                        u: Some("STR".into()),
                        f: Some("FIELDPICTURE(0)".into()),
                        ..Cell::default()
                    },
                ),
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    root.sheet.text_raw = Some("<fld IX='0'/>".into());
    root.sheet.text = Some(String::new());
    m.shapes.push(root);
    doc.masters.push(m);

    // An instance of the master carrying the marker's text.
    let mut inst = Shape {
        id: 5,
        master: Some(2),
        ..Shape::default()
    };
    inst.sheet.set_cell("Width", Cell::value("2.0"));
    inst.sheet.set_cell("Height", Cell::value("1.0"));
    inst.sheet.text_raw = Some("due <fld IX='0'/>".into());
    inst.sheet.text = Some("due ".into());
    let page_sheet = |w: &str, h: &str| {
        let mut sheet = Sheet::default();
        sheet.set_cell("PageWidth", Cell::value(w));
        sheet.set_cell("PageHeight", Cell::value(h));
        sheet
    };
    let mut page = vsdx::Page {
        id: 0,
        name: "Page-1".into(),
        ..vsdx::Page::default()
    };
    page.children.push(IndexChild::Sheet {
        tag: "PageSheet".into(),
        sheet: page_sheet("8.5", "11.0"),
    });
    page.shapes.push(inst);
    doc.pages.push(page);

    let svg = render_page(&doc, &doc.pages[0]).unwrap();
    assert!(
        svg.contains("due Hello"),
        "the marker substitutes the evaluated field: {svg}"
    );
}

/// Unresolvable fields fall back marker content → cached V →
/// empty, never erroring the render.
#[test]
fn field_marker_falls_back_to_the_cached_display() {
    let doc = Document::default();
    let mut s = Shape::default();
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("Height", Cell::value("1.0"));
    // A Value formula that cannot evaluate (unknown cell), no
    // cached V: the marker's own content serves.
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Field".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![(
                "Value".to_string(),
                Cell {
                    u: Some("STR".into()),
                    f: Some("User.Missing".into()),
                    ..Cell::default()
                },
            )],
            ..Row::default()
        }],
        ..Section::default()
    }));
    s.sheet.text_raw = Some("value: <fld IX='0'>4/17</fld>".into());
    s.sheet.text = Some("value: 4/17".into());
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    assert!(
        out.contains("value: 4/17"),
        "unresolvable field serves its cached display: {out}"
    );
}

/// Page facts thread through the walk: PAGENUMBER()/PAGENAME()
/// evaluate live.
#[test]
fn field_page_number_renders_live() {
    let mut doc = Document::default();
    let mut s = Shape {
        id: 1,
        ..Shape::default()
    };
    s.sheet.set_cell("Width", Cell::value("2.0"));
    s.sheet.set_cell("Height", Cell::value("1.0"));
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Field".into(),
        rows: vec![Row {
            ix: 0,
            cells: vec![(
                "Value".to_string(),
                Cell {
                    u: Some("STR".into()),
                    f: Some("PAGENUMBER()".into()),
                    ..Cell::default()
                },
            )],
            ..Row::default()
        }],
        ..Section::default()
    }));
    s.sheet.text_raw = Some("<fld IX='0'/>".into());
    s.sheet.text = Some(String::new());
    let mut sheet = Sheet::default();
    sheet.set_cell("PageWidth", Cell::value("8.5"));
    sheet.set_cell("PageHeight", Cell::value("11.0"));
    let mut page = vsdx::Page {
        id: 0,
        name: "Flowchart".into(),
        ..vsdx::Page::default()
    };
    page.children.push(IndexChild::Sheet {
        tag: "PageSheet".into(),
        sheet,
    });
    page.shapes.push(s);
    doc.pages.push(page);
    // A second page so the first is page 1 of 2 — PAGENUMBER is
    // the 1-based position.
    doc.pages.push(vsdx::Page {
        id: 1,
        name: "Page-2".into(),
        ..vsdx::Page::default()
    });

    let svg = render_page(&doc, &doc.pages[0]).unwrap();
    assert!(
        svg.contains(">1<"),
        "PAGENUMBER renders the 1-based position: {svg}"
    );
}

/// #378 / CodeRabbit on PR #381: an instance without its own `<Text>`
/// shows its master's — but a multi-root master's instance draws every
/// root (with its text) through the mirroring, so its first root's
/// label must not also land in the instance's text block.
#[test]
fn multi_root_master_label_renders_once() {
    let mut doc = Document::default();
    let root = |id: i64, px: &str, text: Option<&str>| {
        let mut r = Shape {
            id,
            ..Shape::default()
        };
        for (n, v) in [
            ("Width", "1.0"),
            ("Height", "0.5"),
            ("PinX", px),
            ("PinY", "0.25"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.25"),
            ("TxtWidth", "1.0"),
            ("TxtHeight", "0.5"),
        ] {
            r.sheet.set_cell(n, Cell::value(v));
        }
        r.sheet.text = text.map(str::to_string);
        r
    };
    doc.masters.push(vsdx::Master {
        id: 3,
        shapes: vec![root(1, "0.5", Some("FIRSTROOT")), root(2, "1.5", None)],
        ..vsdx::Master::default()
    });
    let mut inst = Shape {
        id: 9,
        master: Some(3),
        ..Shape::default()
    };
    for (n, v) in [
        ("Width", "2.0"),
        ("Height", "0.5"),
        ("PinX", "2.0"),
        ("PinY", "2.0"),
    ] {
        inst.sheet.set_cell(n, Cell::value(v));
    }
    let mut page = vsdx::Page {
        id: 0,
        name: "Page-1".into(),
        ..vsdx::Page::default()
    };
    let mut ps = Sheet::default();
    ps.set_cell("PageWidth", Cell::value("8.5"));
    ps.set_cell("PageHeight", Cell::value("11.0"));
    page.children.push(IndexChild::Sheet {
        tag: "PageSheet".into(),
        sheet: ps,
    });
    page.shapes.push(inst);
    doc.pages.push(page);
    let svg = render_page(&doc, &doc.pages[0]).unwrap();
    assert_eq!(svg.matches("FIRSTROOT").count(), 1, "{svg}");
}
