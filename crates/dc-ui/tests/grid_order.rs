//! GitHub #306: the page grid lies beneath the drawing. A filled shape
//! hides the grid lines under it, as in Visio; before, the grid was
//! painted after the shape layers and ran across every fill.
//! Fixtures are generated in code (repo policy: no Microsoft content).

use dc_ui::app::App;
use egui_kittest::Harness;

mod common;

#[test]
fn filled_shapes_hide_the_grid() {
    dc_ui::svgimg::force_sync_raster_for_tests();
    let dir = tempfile::tempdir().unwrap();
    // One 2 × 1 in rectangle, white fill (No Style), black outline,
    // text centered.
    let path = common::shell_fixture(dir.path());
    let app = App::with_doc(&path);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 1000.0))
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            app,
        );
    for _ in 0..8 {
        h.run();
    }
    let img = h.render().unwrap();
    let (w, hgt) = (img.width(), img.height());
    // The hairline outline paints mid-gray at this zoom (#355), darker
    // than the grid's 216 and the paper.
    let dark = |x: u32, y: u32| img.get_pixel(x, y).0[..3].iter().all(|&c| c < 200);
    let white = |x: u32, y: u32| img.get_pixel(x, y).0[..3].iter().all(|&c| c >= 250);
    // The outline's top edge: the topmost row holding a dark run as
    // long as the shape (the page frame and ruler borders run longer,
    // ribbon chrome shorter).
    let long_run = |y: u32| -> Option<(u32, u32)> {
        let (mut best, mut start) = ((0, 0), None);
        for x in 0..w {
            match (dark(x, y), start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    if x - s > best.1 - best.0 {
                        best = (s, x);
                    }
                    start = None;
                }
                _ => {}
            }
        }
        (80..400).contains(&(best.1 - best.0)).then_some(best)
    };
    let (top, (left, right)) = (0..hgt)
        .find_map(|y| long_run(y).map(|r| (y, r)))
        .expect("the rectangle's top edge");
    // Inside the fill, just below the top edge (the text sits at the
    // center): only fill white — no grid line runs across it.
    let inside: Vec<(u32, u32)> = (top + 3..top + 8)
        .flat_map(|y| (left + 3..right - 3).map(move |x| (x, y)))
        .filter(|&(x, y)| !white(x, y))
        .collect();
    assert!(
        inside.is_empty(),
        "grid ink inside the filled shape at {:?}",
        &inside[..inside.len().min(5)]
    );
    // The grid is on: the paper above the shape shows its lines.
    let above = (top.saturating_sub(40)..top.saturating_sub(5))
        .flat_map(|y| (left + 3..right - 3).map(move |x| (x, y)))
        .filter(|&(x, y)| !white(x, y))
        .count();
    assert!(above > 0, "no grid lines on the paper above the shape");
}
