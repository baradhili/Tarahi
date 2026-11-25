use super::*;
use vsdx::model::{IndexChild, Page, Section, SheetChild};

// Shared test fixtures and builders used across the svg test modules.
/// Synthetic Role-callout-shaped master (issue #15 specimen, values
/// recomputed — no Microsoft content): a root group whose children
/// mix proportional XForms (`Sheet.5!Width*1`) with GUARDed absolute
/// ones (`GUARD(4MM)`), driven by User cells whose formulas reference
/// the group Height.
fn role_master_fixture() -> (Shape, Shape) {
    let set = |sheet: &mut Sheet, n: &str, v: f64, f: Option<&str>| {
        sheet.set_cell(
            n,
            Cell {
                v: Some(format!("{v}")),
                f: f.map(str::to_string),
                ..Cell::default()
            },
        );
    };
    // Root: master canvas 1.1811 × 0.7874.
    let mut root = Shape {
        id: 5,
        ..Shape::default()
    };
    set(&mut root.sheet, "Width", 1.181102362204724, None);
    set(&mut root.sheet, "Height", 0.7874015748031495, None);
    let urow = |n: &str, v: f64, f: Option<&str>| Row {
        n: Some(n.into()),
        cells: vec![(
            "Value".into(),
            Cell {
                v: Some(format!("{v}")),
                f: f.map(str::to_string),
                ..Cell::default()
            },
        )],
        ..Row::default()
    };
    root.sheet.children.push(SheetChild::Section(Section {
        name: "User".into(),
        rows: vec![
            urow("ShapeLayout", 0.0, None),
            urow("InfoPanelHeight", 0.2362204724409449, None),
            urow("IconX", 0.1181102362204724, Some("User.InfoPanelHeight/2")),
            urow(
                "IconY",
                0.6692913385826771,
                Some("IF(OR(User.ShapeLayout<>1), Height-User.InfoPanelHeight/2, User.InfoPanelHeight/2)"),
            ),
        ],
        ..Section::default()
    }));
    // Container child: fully proportional.
    let mut container = Shape {
        id: 6,
        ..Shape::default()
    };
    set(
        &mut container.sheet,
        "Width",
        1.181102362204721,
        Some("Sheet.5!Width*1"),
    );
    set(
        &mut container.sheet,
        "Height",
        0.7874015748031491,
        Some("Sheet.5!Height*1"),
    );
    set(
        &mut container.sheet,
        "PinX",
        0.5905511811023657,
        Some("Sheet.5!Width*0.5"),
    );
    set(
        &mut container.sheet,
        "PinY",
        0.3937007874015748,
        Some("Sheet.5!Height*0.5"),
    );
    set(
        &mut container.sheet,
        "LocPinX",
        0.5905511811023605,
        Some("Width*0.5"),
    );
    set(
        &mut container.sheet,
        "LocPinY",
        0.3937007874015746,
        Some("Height*0.5"),
    );
    // Icon child: absolute size, pinned via User cells.
    let mut icon = Shape {
        id: 8,
        ..Shape::default()
    };
    set(
        &mut icon.sheet,
        "Width",
        0.1574803149606299,
        Some("GUARD(4MM)"),
    );
    set(
        &mut icon.sheet,
        "Height",
        0.1692913385826771,
        Some("GUARD(4.3MM)"),
    );
    set(
        &mut icon.sheet,
        "PinX",
        0.1181102362204724,
        Some("GUARD(Sheet.5!User.IconX)"),
    );
    set(
        &mut icon.sheet,
        "PinY",
        0.6692913385826771,
        Some("GUARD(Sheet.5!User.IconY)"),
    );
    set(
        &mut icon.sheet,
        "LocPinX",
        0.07874015748031496,
        Some("Width*0.5"),
    );
    set(
        &mut icon.sheet,
        "LocPinY",
        0.08464566929133857,
        Some("Height*0.5"),
    );
    // Literal child (no formulas): uniform-scale fallback.
    let mut plain = Shape {
        id: 16,
        ..Shape::default()
    };
    set(&mut plain.sheet, "Width", 0.5, None);
    set(&mut plain.sheet, "Height", 0.25, None);
    set(&mut plain.sheet, "PinX", 0.5, None);
    set(&mut plain.sheet, "PinY", 0.25, None);
    root.sub_shapes = vec![container, icon, plain];

    // Instance: same master stretched tall (2.25 × 7.02 aspect change).
    let mut inst = Shape {
        id: 65,
        master: Some(7),
        ..Shape::default()
    };
    set(&mut inst.sheet, "Width", 2.657480296178819, None);
    set(&mut inst.sheet, "Height", 5.528301723437821, None);
    (root, inst)
}

/// Parse `matrix(a,b,c,d,e,f)` / `scale(sx,sy)` into (a..f).
fn parse_tf(t: &str) -> [f64; 6] {
    let body = t
        .trim_start_matches("matrix(")
        .trim_start_matches("scale(")
        .trim_end_matches(')');
    let nums: Vec<f64> = body.split(',').map(|n| n.parse().unwrap()).collect();
    if t.starts_with("scale(") {
        [nums[0], 0.0, 0.0, nums[1], 0.0, 0.0]
    } else {
        nums.try_into().unwrap()
    }
}

/// A Character row with Font/Size/Color cells for char_font tests.
fn char_row(font: Option<&str>) -> Row {
    let mut cells = Vec::new();
    if let Some(f) = font {
        cells.push((
            "Font".to_string(),
            Cell {
                v: Some(f.to_string()),
                ..Cell::default()
            },
        ));
    }
    Row {
        cells,
        ..Row::default()
    }
}

fn shape_with_character(font: Option<&str>) -> Shape {
    let mut s = Shape::default();
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![char_row(font)],
        ..Section::default()
    }));
    s
}

/// A geometry row for the wave-3 shape fixtures.
fn w3_row(ix: i64, t: &str, cells: &[(&str, &str)]) -> Row {
    Row {
        ix,
        t: Some(t.to_string()),
        cells: cells
            .iter()
            .map(|(n, v)| (n.to_string(), Cell::value(*v)))
            .collect(),
        ..Row::default()
    }
}

/// Compose an SVG transform list (`translate(a,b) scale(c,d)
/// rotate(e) …`) into one matrix. SVG applies leftmost-last to the
/// point, so composition is left-to-right matrix multiplication.
fn compose_tf(list: &str) -> Mat {
    let mut total = Mat::identity();
    for cmd in list.split(')').filter(|s| !s.trim().is_empty()) {
        let body = cmd.trim().split('(').nth(1).unwrap_or("");
        let n: Vec<f64> = body.split(',').map(|v| v.trim().parse().unwrap()).collect();
        let m = if cmd.trim().starts_with("translate") {
            Mat::translate(n[0], n[1])
        } else if cmd.trim().starts_with("scale") {
            Mat::scale(n[0], n[1])
        } else if cmd.trim().starts_with("rotate") {
            Mat::rotation(n[0].to_radians())
        } else {
            panic!("unhandled transform command: {cmd}");
        };
        total = total.mul(&m);
    }
    total
}

/// A text block pinned off-origin, for the TxtPin-pivot tests.
fn shape_with_pinned_text(txt_angle_deg: f64) -> Shape {
    let mut s = Shape::default();
    let set = |s: &mut Shape, n: &str, v: f64| {
        s.sheet.set_cell(
            n,
            Cell {
                v: Some(format!("{v}")),
                ..Cell::default()
            },
        );
    };
    set(&mut s, "TxtPinX", 1.5);
    set(&mut s, "TxtPinY", 0.75);
    set(&mut s, "TxtWidth", 1.0);
    set(&mut s, "TxtHeight", 0.5);
    set(&mut s, "TxtAngle", txt_angle_deg.to_radians());
    s.sheet.text = Some("label".into());
    s
}

fn text_wrapper_transform(
    sx: f64,
    sy: f64,
    txt_angle_deg: f64,
) -> Option<(String, (f64, f64))> {
    let doc = Document::default();
    let s = shape_with_pinned_text(txt_angle_deg);
    let chain = [&s];
    let mut out = String::new();
    emit_text(
        &doc, &s, &chain, 2.0, 1.0, sx, sy, false, None, None, &mut out,
    );
    let t = out
        .split("transform=\"")
        .nth(1)?
        .split('"')
        .next()
        .map(str::to_owned)?;
    Some((t, (1.5, -0.75))) // (list, TxtPin in the emitted y-down frame)
}

fn xform_shape(cells: &[(&str, f64)]) -> Shape {
    let mut s = Shape::default();
    for (n, v) in cells {
        s.sheet.set_cell(
            n,
            Cell {
                v: Some(format!("{v}")),
                ..Cell::default()
            },
        );
    }
    s
}

/// A member at parent-local Pin (1,1), 1×1: parent-local rect
/// (0.5, 0.5, 1, 1).
fn unit_member() -> Shape {
    let mut m = xform_shape(&[]);
    m.id = 10;
    m
}

/// #203: a NURBSTo row's E cell (formula text) — the interior
/// points/knots/weights ride in `E`, the endpoint in X/Y.
fn nurbs_row(ix: i64, x: f64, y: f64, e_text: &str) -> Row {
    Row {
        t: Some("NURBSTo".into()),
        ix,
        cells: vec![
            ("X".to_string(), Cell::value(format!("{x}"))),
            ("Y".to_string(), Cell::value(format!("{y}"))),
            ("A".to_string(), Cell::value("0")),
            ("B".to_string(), Cell::value("1")),
            ("C".to_string(), Cell::value("0")),
            ("D".to_string(), Cell::value("1")),
            (
                "E".to_string(),
                vsdx::Cell {
                    v: Some(e_text.to_string()),
                    f: Some(e_text.to_string()),
                    ..Cell::default()
                },
            ),
        ],
        ..Row::default()
    }
}

fn page_fixture(shapes: Vec<Shape>) -> (Document, Page) {
    let mut sheet = Sheet::default();
    sheet.set_cell("PageWidth", Cell::value("8.5"));
    sheet.set_cell("PageHeight", Cell::value("11"));
    let page = Page {
        id: 0,
        name: "T".into(),
        children: vec![IndexChild::Sheet {
            tag: "PageSheet".into(),
            sheet,
        }],
        shapes,
        ..Page::default()
    };
    (
        Document {
            pages: vec![page.clone()],
            ..Document::default()
        },
        page,
    )
}

fn shape_at(id: i64, pin: (f64, f64), w: f64, h: f64, angle_deg: f64) -> Shape {
    let mut sh = Shape {
        id,
        ..Shape::default()
    };
    for (n, v) in [
        ("PinX", pin.0),
        ("PinY", pin.1),
        ("Width", w),
        ("Height", h),
        ("LocPinX", w / 2.0),
        ("LocPinY", h / 2.0),
        ("Angle", angle_deg),
    ] {
        sh.sheet.set_cell(n, Cell::value(format!("{v}")));
    }
    sh
}

/// A geometry row built from (type, [(cell, value)]).
fn geo_row(ix: i64, t: &str, cells: &[(&str, f64)]) -> Row {
    Row {
        t: Some(t.into()),
        ix,
        cells: cells
            .iter()
            .map(|(n, v)| (n.to_string(), Cell::value(format!("{v}"))))
            .collect(),
        ..Row::default()
    }
}

fn push_geo(s: &mut Shape, rows: Vec<Row>) {
    let mut sec = Section {
        name: "Geometry".into(),
        ix: 0,
        ..Section::default()
    };
    sec.rows = rows;
    s.sheet.children.push(SheetChild::Section(sec));
}

const PAD: f64 = 0.08;

/// A master PolylineTo row and an instance row at the same IX.
fn polyline_master_and_instance(
    inst_row: vsdx::Row,
    master_text: &str,
) -> (Document, vsdx::Page) {
    let mut a = geo_row(2, "PolylineTo", &[("X", 2.0), ("Y", 0.0)]);
    a.cells.push((
        "A".into(),
        Cell {
            v: Some(master_text.into()),
            ..Cell::default()
        },
    ));
    let mut master = shape_at(1, (1.0, 0.5), 2.0, 1.0, 0.0);
    push_geo(
        &mut master,
        vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), a],
    );
    let mut inst = shape_at(1, (4.0, 5.0), 4.0, 1.0, 0.0);
    inst.master = Some(7);
    push_geo(
        &mut inst,
        vec![geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), inst_row],
    );
    let (mut doc, page) = page_fixture(vec![inst]);
    doc.masters.push(vsdx::model::Master {
        id: 7,
        shapes: vec![master],
        ..vsdx::model::Master::default()
    });
    (doc, page)
}

/// A 1×1 rectangle with closed geometry, for paint tests.
fn rect_shape(id: i64, pin: (f64, f64)) -> Shape {
    let mut s = shape_at(id, pin, 1.0, 1.0, 0.0);
    push_geo(
        &mut s,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "LineTo", &[("X", 1.0), ("Y", 0.0)]),
            geo_row(3, "LineTo", &[("X", 1.0), ("Y", 1.0)]),
            geo_row(4, "LineTo", &[("X", 0.0), ("Y", 1.0)]),
            geo_row(5, "LineTo", &[("X", 0.0), ("Y", 0.0)]),
        ],
    );
    s
}

/// One text block with character runs through `emit_text`: `raw`
/// carries the `<cp IX='n'/>` markers, `rows` the Character rows by
/// IX, `cells` extra sheet cells, `para` extra Paragraph cells.
fn emit_runs(
    raw: &str,
    rows: &[(i64, &[(&str, &str)])],
    cells: &[(&str, &str)],
    para: &[(&str, &str)],
) -> String {
    let doc = Document::default();
    let mut s = Shape::default();
    let mut plain = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('<') {
        plain.push_str(&rest[..i]);
        rest = &rest[rest[i..].find('>').unwrap() + i + 1..];
    }
    plain.push_str(rest);
    s.sheet.text = Some(plain);
    s.sheet.text_raw = Some(raw.into());
    s.sheet.set_cell("Width", Cell::value("4.0"));
    s.sheet.set_cell("LeftMargin", Cell::value("0"));
    s.sheet.set_cell("RightMargin", Cell::value("0"));
    for (n, v) in cells {
        s.sheet.set_cell(n, Cell::value(*v));
    }
    let row = |ix: i64, cells: &[(&str, &str)]| Row {
        ix,
        cells: cells
            .iter()
            .map(|(n, v)| (n.to_string(), Cell::value(*v)))
            .collect(),
        ..Row::default()
    };
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: rows.iter().map(|(ix, c)| row(*ix, c)).collect(),
        ..Section::default()
    }));
    if !para.is_empty() {
        s.sheet.children.push(SheetChild::Section(Section {
            name: "Paragraph".into(),
            rows: vec![row(0, para)],
            ..Section::default()
        }));
    }
    let chain = [&s];
    let mut out = String::new();
    let w = cells
        .iter()
        .find(|(n, _)| *n == "Width")
        .map_or(4.0, |(_, v)| v.parse().unwrap());
    emit_text(
        &doc, &s, &chain, w, 1.0, 1.0, 1.0, false, None, None, &mut out,
    );
    out
}

/// The `x`/`y` attribute values of every `<tspan>`, in order.
fn tspan_xy(svg: &str) -> Vec<(f64, f64)> {
    svg.split("<tspan ")
        .skip(1)
        .filter_map(|t| {
            let get = |k: &str| {
                t.split(&format!(" {k}=\""))
                    .nth(1)
                    .or_else(|| t.strip_prefix(&format!("{k}=\"")))?
                    .split('"')
                    .next()?
                    .parse::<f64>()
                    .ok()
            };
            Some((get("x")?, get("y")?))
        })
        .collect()
}

/// Count the emitted text lines (tspans) of one shape.
fn text_lines(out: &str) -> Vec<String> {
    out.split("<tspan")
        .skip(1)
        .map(|l| {
            l.split('>')
                .nth(1)
                .unwrap_or("")
                .split('<')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// A shape carrying the utilisation.vsdx specimen 1065's exact
/// text-block cells (#21): TxtWidth 0.8159 in, 8 pt (0.1111 in)
/// text, zero side margins (inherited from the connector master)
/// — the authored three-line label the 0.5 em guess re-wrapped
/// into four (15 chars > its 14-char cap at that width).
fn shape_1065() -> Shape {
    let mut s = Shape::default();
    s.sheet.text = Some("If allograft\r\nIs implanted or\r\n opened".into());
    // The specimen's connector master inherits zero left/right
    // margins, so the usable width IS the block width.
    for (n, v) in [
        ("Width", "2.0"),
        ("Height", "1.0"),
        ("TxtWidth", "0.8159"),
        ("TxtHeight", "0.6"),
        ("LeftMargin", "0"),
        ("RightMargin", "0"),
    ] {
        s.sheet.set_cell(n, Cell::value(v));
    }
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Character".into(),
        rows: vec![Row {
            cells: vec![("Size".into(), Cell::value("0.1111"))],
            ..Row::default()
        }],
        ..Section::default()
    }));
    s
}

mod affine;
mod ink;
mod jumps;
mod page;
mod shape_layer;
mod text;
