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
    let svg = dc::svg::render_page(&d, &d.pages[0]).unwrap();
    for name in ["CONSTRAINT", "REQUIREMENT", "CAPABILITY", "GAP", "impacts"] {
        assert_eq!(
            svg.matches(&format!("{name}</tspan>")).count(),
            1,
            "{name} once"
        );
    }
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
