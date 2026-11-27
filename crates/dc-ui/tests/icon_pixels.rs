//! Pixel-level check that ribbon icons actually rasterize (regression
//! guard for the "icons missing from the ribbon" bug).

trait PixelImage {
    fn dims(&self) -> (u32, u32);
    fn px(&self, x: u32, y: u32) -> [u8; 4];
}
impl PixelImage for image::RgbaImage {
    fn dims(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
    fn px(&self, x: u32, y: u32) -> [u8; 4] {
        self.get_pixel(x, y).0
    }
}

use dc_ui::ribbon::{big, content_row, Action, Group, Icon, RibbonButton};
use egui_kittest::Harness;

fn ink(img: &impl PixelImage) -> usize {
    let (w, h) = img.dims();
    (0..w)
        .flat_map(|x| (0..h).map(move |y| (x, y)))
        .filter(|(x, y)| {
            let p = img.px(*x, *y);
            p[0] < 235 || p[1] < 235 || p[2] < 235
        })
        .count()
}

#[test]
fn icon_paints_pixels_standalone() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(200.0, 120.0))
        .build_ui(|ui| {
            Icon::Paste.paint(
                ui,
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(80.0, 80.0)),
                true,
            );
        });
    h.run();
    let img = h.render().unwrap();
    let n = ink(&img);
    assert!(n > 300, "standalone Paste icon painted only {n} px");
}

#[test]
fn icon_paints_pixels_inside_content_row() {
    let groups = vec![Group::new(
        "Clipboard",
        vec![big(
            RibbonButton::new(Icon::Paste, "Paste", Action::Macros).enabled(false)
        )],
    )];
    // Bare content row (no theme, no panel chrome).
    let mut h = Harness::builder()
        .with_size(egui::vec2(400.0, 120.0))
        .build_ui(|ui| {
            content_row(ui, &groups, &mut |_a| {}, &mut |_| {}, &mut |_, _| {});
        });
    h.run();
    let img = h.render().unwrap();
    let n = ink(&img);
    // the galley text alone is ~250-400 px; a visible (grayed) icon adds
    // far more than that
    assert!(
        n > 800,
        "content-row Paste painted only {n} px — icon missing"
    );

    // Same row inside the shell's chrome: theme installed and a panel
    // frame around it (folded in from the former theme+panel test; the
    // theme path is what every full-shell harness exercises).
    let mut h = Harness::builder()
        .with_size(egui::vec2(400.0, 120.0))
        .build_ui(|ui| {
            dc_ui::theme::install(ui.ctx());
            egui::Panel::top("ribbon_content")
                .frame(
                    egui::Frame::new()
                        .fill(dc_ui::theme::RIBBON_BG)
                        .inner_margin(egui::Margin::symmetric(4, 3)),
                )
                .show(ui, |ui| content_row(ui, &groups, &mut |_a| {}, &mut |_| {}, &mut |_, _| {}));
        });
    h.run();
    let img = h.render().unwrap();
    let n = ink(&img);
    assert!(
        n > 800,
        "theme+panel Paste painted only {n} px — icon missing"
    );
}

#[test]
fn icon_paints_in_full_shell() {
    // The whole app over the synthetic fixture, Home tab.
    let mut app = dc_ui::app::App::with_blank();
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui(move |ui| {
            dc_ui::theme::install(ui.ctx());
            app.draw(ui);
        });
    h.run();
    let img = h.render().unwrap();
    // scan the ribbon band (y 90..180) for icon ink left of the
    // stencil pane — the band grew with the Tools stack's fourth row
    // (the #132 Freeform button).
    let (w, _h) = img.dims();
    let mut n = 0;
    for x in 0..w.min(600) {
        for y in 90..180.min(_h) {
            let p = img.px(x, y);
            if p[0] < 235 || p[1] < 235 || p[2] < 235 {
                n += 1;
            }
        }
    }
    assert!(
        n > 2000,
        "full-shell ribbon band has only {n} ink px — icons missing"
    );
}

/// #130: the Slide Show button's MDI play asset rasterizes — a broken
/// or unparseable vendored SVG would fall to the gray square glyph
/// (the `translate` gap this suite exists for).
#[test]
fn play_icon_rasterizes_green() {
    dc_ui::svgimg::pin_determinism_for_tests();
    let mut h = Harness::builder()
        .with_size(egui::vec2(200.0, 120.0))
        .build_ui(|ui| {
            dc_ui::theme::install(ui.ctx());
            Icon::Play.paint(
                ui,
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(80.0, 80.0)),
                true,
            );
        });
    h.run();
    h.run();
    let img = h.render().unwrap();
    let (w, h) = img.dims();
    let mut green = 0;
    for x in 0..w {
        for y in 0..h {
            let p = img.px(x, y);
            // the Office green family the icon tints with (0x2e,0x8b,0x57)
            if p[0] < 0x70 && p[1] > 0x70 && p[1] < 0xa0 && p[2] > 0x40 && p[2] < 0x70 {
                green += 1;
            }
        }
    }
    assert!(green > 300, "standalone 80px play icon: {green} green px");
}

/// #130: the same play icon inside the full shell's View tab (the
/// exact `run_shell` recipe) — the standalone case above cannot catch
/// a shell-only fallback.
#[test]
fn play_icon_rasterizes_in_full_shell_view_tab() {
    dc_ui::svgimg::pin_determinism_for_tests();
    let mut app = dc_ui::app::App::with_blank();
    app.set_tab_for_test(dc_ui::ribbon::Tab::View);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui(move |ui| {
            dc_ui::theme::install_for(ui.ctx(), egui::ThemePreference::Light);
            app.draw(ui);
        });
    h.run();
    h.run();
    let img = h.render().unwrap();
    let (w, h) = img.dims();
    let mut green = 0;
    for x in 0..w {
        for y in 0..h {
            let p = img.px(x, y);
            if p[0] < 0x70 && p[1] > 0x70 && p[1] < 0xa0 && p[2] > 0x40 && p[2] < 0x70 {
                green += 1;
            }
        }
    }
    // The hero slot is 32×32 and the MDI play triangle covers under
    // half of its viewBox — ~130 saturated pixels is a full triangle.
    assert!(green > 100, "shell View tab play icon: {green} green px");
}
