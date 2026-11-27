//! #378 / #302 against the committed corpus drawing
//! `corpus/imported/nested-shapes-and-weird-connectors.vsdx` (its 600 dpi
//! golden sits next to it).

use std::path::Path;

fn doc() -> vsdx::Document {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/imported/nested-shapes-and-weird-connectors.vsdx");
    vsdx::parse::parse(&path).expect("the corpus drawing parses")
}

/// The four box titles and the connector label live on the masters as
/// field text (`<fld>` over `Sheet.5!User.ObjectTypeName`, 5 being the
/// master root); the instances carry no `<Text>`. Each instance shows
/// its own master's name, read through its own root: resolved against
/// the page, `Sheet.5` hit page shape 5 and every box read REQUIREMENT.
#[test]
fn master_field_labels_render_per_instance() {
    let d = doc();
    let page = &d.pages[0];
    // Each instance's own complete label (CodeRabbit on PR #381: a
    // page-wide count passes with two labels swapped).
    let fctx = dc::fields::FieldsCtx::for_page(&d, page);
    for (id, name) in [
        (3, "CONSTRAINT"),
        (7, "REQUIREMENT"),
        (13, "CAPABILITY"),
        (21, "GAP"),
        (24, "impacts"),
    ] {
        let mut chain = Vec::new();
        assert!(path_to(&page.shapes, id, &mut chain), "shape {id}");
        let s = *chain.last().unwrap();
        let text = dc::fields::substituted_text(&d, s, &chain, Some(&fctx));
        assert_eq!(text.as_deref().map(str::trim), Some(name), "shape {id}");
    }
    // …and nothing else on the page draws one of them again.
    let svg = dc::svg::render_page(&d, page).unwrap();
    for name in ["CONSTRAINT", "REQUIREMENT", "CAPABILITY", "GAP", "impacts"] {
        assert_eq!(
            svg.matches(&format!(">{name}</tspan>")).count()
                + svg.matches(&format!(" {name}</tspan>")).count(),
            1,
            "{name} once"
        );
    }
}

/// The ancestor chain (ending with the shape) of shape `id`.
fn path_to<'a>(shapes: &'a [vsdx::Shape], id: i64, chain: &mut Vec<&'a vsdx::Shape>) -> bool {
    for s in shapes {
        chain.push(s);
        if s.id == id || path_to(&s.sub_shapes, id, chain) {
            return true;
        }
        chain.pop();
    }
    false
}

/// The Square loop (shape 28) draws its loop from master rows its
/// inline geometry only partly overrides (Y cells `Inh`, X absent). The
/// ink bounds, which size the canvas layer, merge those rows like the
/// painter does: the loop rises above the Requirement box (shape 5).
/// Walking only the raw inline rows bounded a sliver inside the box,
/// and the canvas clipped the loop.
#[test]
fn square_loop_bounds_cover_the_merged_master_rows() {
    let d = doc();
    let page = &d.pages[0];
    let (_, loop_top, _, loop_h) = dc::svg::shape_ink_bounds(&d, page, 28).unwrap();
    let (_, box_top, _, _) = dc::svg::shape_ink_bounds(&d, page, 5).unwrap();
    assert!(loop_top < box_top - 0.1, "the loop rises above the box");
    assert!(loop_h > 0.5, "not a sliver: {loop_h}");
}

/// #397 against its own fixture, `corpus/imported/orbus-shape-layout.vsdx`
/// (Visio 2021 reference next to it): one box per layout. Layouts 0 and
/// 1 keep the type label in their panel; 2, 3 and 4 hide it, and the
/// fixture carries Visio's own `HideText V='1' F='Inh'` caches for them.
#[test]
fn orbus_layouts_hide_the_type_label_like_visio() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/imported/orbus-shape-layout.vsdx");
    let d = vsdx::parse::parse(&path).expect("the fixture parses");
    let svg = dc::svg::render_page(&d, &d.pages[0]).unwrap();
    for shown in ["PRINCIPLE", "CONSTRAINT"] {
        assert!(svg.contains(&format!("{shown}</tspan>")), "{shown} shows");
    }
    for hidden in ["REQUIREMENT", "CAPABILITY", "WORK PACKAGE"] {
        assert!(
            !svg.contains(&format!("{hidden}</tspan>")),
            "{hidden} is hidden"
        );
    }
}

/// #397: the box masters hide their type label once the layout leaves
/// the panel modes (`HideText = Sheet.5!User.ShapeLayout>1` on the
/// panel; Visio 2021 hides it for layouts 2-4). A layout switched in
/// the app writes only the box's `User.ShapeLayout`, no cache on the
/// panel, so the master's formula decides, its `Sheet.5!` read through
/// the box, the master root's counterpart.
#[test]
fn layout_switch_hides_the_panel_label() {
    for (layout, shown) in [("2", false), ("4", false), ("1", true)] {
        let mut d = doc();
        let bx = shape_mut(&mut d.pages[0].shapes, 1).expect("Constraint box");
        let row = vsdx::model::Row {
            n: Some("ShapeLayout".into()),
            cells: vec![("Value".into(), vsdx::Cell::value(layout))],
            ..Default::default()
        };
        let user = bx.sheet.children.iter_mut().find_map(|c| match c {
            vsdx::model::SheetChild::Section(sec) if sec.name == "User" => Some(sec),
            _ => None,
        });
        match user {
            Some(sec) => {
                sec.rows.retain(|r| r.n.as_deref() != Some("ShapeLayout"));
                sec.rows.push(row);
            }
            None => {
                bx.sheet
                    .children
                    .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                        name: "User".into(),
                        rows: vec![row],
                        ..Default::default()
                    }))
            }
        }
        let page = &d.pages[0];
        let svg = dc::svg::render_page(&d, page).unwrap();
        assert_eq!(svg.contains("CONSTRAINT</tspan>"), shown, "layout {layout}");
        assert!(
            svg.contains("REQUIREMENT</tspan>"),
            "layout {layout}: the other boxes keep theirs"
        );
    }
}

fn shape_mut(shapes: &mut [vsdx::Shape], id: i64) -> Option<&mut vsdx::Shape> {
    for s in shapes {
        if s.id == id {
            return Some(s);
        }
        if let Some(found) = shape_mut(&mut s.sub_shapes, id) {
            return Some(found);
        }
    }
    None
}
