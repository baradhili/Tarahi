//! HiDPI canvas density (GitHub #414): a forced pixels-per-point of
//! 2.0 must double every canvas layer's raster density, so the
//! compositor upscales nothing on a 2× display. The headless default
//! (1.0 — what every other kittest harness runs at) keeps the
//! existing goldens byte-identical; this file carries the
//! doubled-scale pass the issue's acceptance asks for.

use dc_ui::app::App;
use egui_kittest::Harness;

mod common;
use common::many_shape_fixture as fixture;

fn mount(app: App, pixels_per_point: f32) -> Harness<'static, App> {
    // Goldens/steps must be machine-independent: pin text to the
    // embedded Carlito rather than this machine's installed fonts.
    dc_ui::svgimg::pin_determinism_for_tests();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(pixels_per_point)
        // #203 review: the settle-gate repaint (#124 item 6.2) keeps
        // frames alive until the zoom bucket holds after open — give
        // run() the steps it needs.
        .with_max_steps(16)
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            app,
        )
}

/// #414 acceptance: at `pixels_per_point` 2.0 the layer store
/// rasterizes at double density — shape 1's texture (the fixture's
/// fat 2×1-in center rect) comes out twice as wide and tall as the
/// same document at 1.0. Layout, fit zoom, and placement are logical
/// points and stay identical across the two harnesses; only the
/// texture-request ppi folds the device scale, which is exactly the
/// seam under test. (Ceil rounding at the raster edge allows one
/// pixel of slack per side.)
#[test]
fn layers_rasterize_at_double_density_on_a_2x_display() {
    let tmp = tempfile::tempdir().unwrap();
    let path = fixture(tmp.path(), 8, 0);

    let mut one = mount(App::with_doc(&path), 1.0);
    one.run();
    let [w1, h1] = one
        .state()
        .debug_layer_tex_size_for_test(1)
        .expect("the center shape's layer landed at 1×");
    let mut two = mount(App::with_doc(&path), 2.0);
    two.run();
    let [w2, h2] = two
        .state()
        .debug_layer_tex_size_for_test(1)
        .expect("the center shape's layer landed at 2×");

    assert!(w1 > 32 && h1 > 16, "fixture layer must be real: {w1}×{h1}");
    assert!(
        (w2 as i32 - 2 * w1 as i32).abs() <= 1,
        "texture width doubles at 2×: {w2} vs {w1}"
    );
    assert!(
        (h2 as i32 - 2 * h1 as i32).abs() <= 1,
        "texture height doubles at 2×: {h2} vs {h1}"
    );
    // The kittest floor is untouched: the headless ceiling stays 2048
    // (egui's InputState default — no backend reports a limit here),
    // and the 8-layer fixture's projection is nowhere near the byte
    // budget, so nothing degraded either.
    assert!(w2 < 2048, "the headless texture cap stays 2048: {w2}");
}
