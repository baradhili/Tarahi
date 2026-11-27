//! #130 Slide Show — the read-only, page-at-a-time presentation:
//! F5 / Shift+F5 entry points, the keyboard navigation map with
//! end-clamping, SubAddress click jumps (and inert address-only
//! links), the Esc restore to the entry page, Presentation Mode's
//! fullscreen toggle, and a show golden.

use dc_ui::app::App;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

mod common;

/// The #130 fixture: three pages; page 1 carries two link shapes —
/// shape 1 (page-center) whose first hyperlink row is a SubAddress
/// GOTOPAGE to Page-3, shape 2 (lower left) whose first row is an
/// address-only URL. Pages 2 and 3 are plain.
fn show_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    common::shell_fixture_pages_hyperlink(dir, &["Page-1", "Page-2", "Page-3"])
}

/// The #130 background-compose fixture: a foreground 8.5×11 page
/// with one hyperlink shape, over a background page that is BOTH
/// oversized (17×22 — unclipped it would ink the letterbox on every
/// side) and carrying a red bar in the region the foreground page
/// leaves empty (only the background's ink can color it).
fn show_bg_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let document = r#"<?xml version="1.0" encoding="utf-8"?>
<VisioDocument xmlns='http://schemas.microsoft.com/office/visio/2012/main' xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'>
<StyleSheets><StyleSheet ID='0' NameU='No Style'>
  <Cell N='LineWeight' V='0.01041666666666667'/><Cell N='LineColor' V='0'/>
  <Cell N='FillForegnd' V='1'/><Cell N='FillPattern' V='1'/>
</StyleSheet></StyleSheets>
</VisioDocument>"#;
    // The foreground page names the background through BackPage.
    let pages = r#"<?xml version="1.0" encoding="utf-8"?>
<Pages xmlns='http://schemas.microsoft.com/office/visio/2012/main' xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'>
<Page ID='0' NameU='Page-1' Name='Page-1' BackPage='1'>
<PageSheet LineStyle='0' FillStyle='0' TextStyle='0'>
  <Cell N='PageWidth' V='8.5'/><Cell N='PageHeight' V='11'/>
</PageSheet>
<Rel r:id='rId1'/>
</Page>
<Page ID='1' NameU='BG' Name='BG' Background='1'>
<PageSheet LineStyle='0' FillStyle='0' TextStyle='0'>
  <Cell N='PageWidth' V='17'/><Cell N='PageHeight' V='22'/>
</PageSheet>
<Rel r:id='rId2'/>
</Page>
</Pages>"#;
    let page1 = r#"<?xml version="1.0" encoding="utf-8"?>
<PageContents xmlns='http://schemas.microsoft.com/office/visio/2012/main' xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'>
<Shapes>
<Shape ID='1' Type='Shape' LineStyle='0' FillStyle='0' TextStyle='0'>
  <Cell N='PinX' V='4.25'/><Cell N='PinY' V='8'/>
  <Cell N='Width' V='2'/><Cell N='Height' V='1'/>
  <Cell N='LocPinX' V='1'/><Cell N='LocPinY' V='0.5'/>
  <Section N='Geometry' IX='0'>
    <Row T='MoveTo' IX='1'><Cell N='X' V='0'/><Cell N='Y' V='0'/></Row>
    <Row T='LineTo' IX='2'><Cell N='X' V='2'/><Cell N='Y' V='0'/></Row>
    <Row T='LineTo' IX='3'><Cell N='X' V='2'/><Cell N='Y' V='1'/></Row>
    <Row T='LineTo' IX='4'><Cell N='X' V='0'/><Cell N='Y' V='1'/></Row>
    <Row T='LineTo' IX='5'><Cell N='X' V='0'/><Cell N='Y' V='0'/></Row>
  </Section>
  <Text>fg</Text>
</Shape>
</Shapes>
</PageContents>"#;
    // The red bar (Visio color 2 = red) spans the bg page's lower
    // middle — under the fg sheet's empty bottom half.
    let page2 = r#"<?xml version="1.0" encoding="utf-8"?>
<PageContents xmlns='http://schemas.microsoft.com/office/visio/2012/main' xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'>
<Shapes>
<Shape ID='1' Type='Shape' LineStyle='0' FillStyle='0' TextStyle='0'>
  <Cell N='PinX' V='8.5'/><Cell N='PinY' V='3'/>
  <Cell N='Width' V='12'/><Cell N='Height' V='2'/>
  <Cell N='LocPinX' V='6'/><Cell N='LocPinY' V='1'/>
  <Cell N='FillForegnd' V='2'/>
  <Section N='Geometry' IX='0'>
    <Row T='MoveTo' IX='1'><Cell N='X' V='0'/><Cell N='Y' V='0'/></Row>
    <Row T='LineTo' IX='2'><Cell N='X' V='12'/><Cell N='Y' V='0'/></Row>
    <Row T='LineTo' IX='3'><Cell N='X' V='12'/><Cell N='Y' V='2'/></Row>
    <Row T='LineTo' IX='4'><Cell N='X' V='0'/><Cell N='Y' V='2'/></Row>
    <Row T='LineTo' IX='5'><Cell N='X' V='0'/><Cell N='Y' V='0'/></Row>
  </Section>
</Shape>
</Shapes>
</PageContents>"#;
    let masters = r#"<?xml version="1.0" encoding="utf-8"?>
<Masters xmlns='http://schemas.microsoft.com/office/visio/2012/main' xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'>
</Masters>"#;
    let page_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id='rId1' Type='http://schemas.microsoft.com/visio/2010/relationships/page' Target='page1.xml'/>
<Relationship Id='rId2' Type='http://schemas.microsoft.com/visio/2010/relationships/page' Target='page2.xml'/>
</Relationships>"#;
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/visio/document.xml" ContentType="application/vnd.ms-visio.drawing.main+xml"/>
<Override PartName="/visio/pages/pages.xml" ContentType="application/vnd.ms-visio.pages+xml"/>
<Override PartName="/visio/pages/page1.xml" ContentType="application/vnd.ms-visio.pages+xml"/>
<Override PartName="/visio/pages/page2.xml" ContentType="application/vnd.ms-visio.pages+xml"/>
<Override PartName="/visio/masters/masters.xml" ContentType="application/vnd.ms-visio.masters+xml"/>
</Types>"#;
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.microsoft.com/visio/2010/relationships/document" Target="visio/document.xml"/>
</Relationships>"#;
    let doc_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.microsoft.com/visio/2010/relationships/pages" Target="pages/pages.xml"/>
<Relationship Id="rId2" Type="http://schemas.microsoft.com/visio/2010/relationships/masters" Target="masters/masters.xml"/>
</Relationships>"#;

    let path = dir.join("showbg.vsdx");
    let file = std::fs::File::create(&path).unwrap();
    let mut w = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let parts: Vec<(String, String)> = [
        ("[Content_Types].xml".to_string(), content_types.to_string()),
        ("_rels/.rels".to_string(), root_rels.to_string()),
        ("visio/document.xml".to_string(), document.to_string()),
        (
            "visio/_rels/document.xml.rels".to_string(),
            doc_rels.to_string(),
        ),
        ("visio/pages/pages.xml".to_string(), pages.to_string()),
        (
            "visio/pages/_rels/pages.xml.rels".to_string(),
            page_rels.to_string(),
        ),
        ("visio/pages/page1.xml".to_string(), page1.to_string()),
        ("visio/pages/page2.xml".to_string(), page2.to_string()),
        ("visio/masters/masters.xml".to_string(), masters.to_string()),
    ]
    .into_iter()
    .collect();
    for (name, data) in &parts {
        w.start_file(name.as_str(), opts).unwrap();
        std::io::Write::write_all(&mut w, data.as_bytes()).unwrap();
    }
    w.finish().unwrap();
    path
}

fn harness_with(path: &std::path::Path) -> Harness<'static, App> {
    dc_ui::svgimg::pin_determinism_for_tests();
    // The show's page raster must land in the same pass (the spec's
    // placeholder rule is exercised by the raster_async suite).
    dc_ui::svgimg::force_sync_raster_for_tests();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_max_steps(16)
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            App::with_doc(path),
        )
}

/// The show's fitted page rect on the 1280×800 harness, from the
/// spec's aspect-fit math (largest centered rect, letterboxed) — the
/// same formula `slideshow_view` applies.
fn fitted_rect(page_w: f64, page_h: f64) -> egui::Rect {
    let scale = (1280.0 / page_w).min(800.0 / page_h);
    let size = egui::vec2((page_w * scale) as f32, (page_h * scale) as f32);
    egui::Rect::from_center_size(egui::pos2(640.0, 400.0), size)
}

/// A page point (inches, y-up) → harness screen pos inside the show.
fn show_point(page: (f64, f64)) -> egui::Pos2 {
    let r = fitted_rect(8.5, 11.0);
    let ppi = r.width() / 8.5;
    egui::pos2(
        r.left() + page.0 as f32 * ppi,
        r.bottom() - page.1 as f32 * ppi,
    )
}

/// Click (press + release queued together, then settle — the kittest
/// convention; settling between press and release cancels the click).
fn click_at(h: &mut Harness<'static, App>, pos: egui::Pos2) {
    let b = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.hover_at(pos);
    h.event(b(true));
    h.event(b(false));
    h.run();
}

/// F5 enters from the first page; Esc exits back on the ENTRY page
/// with the window un-fullscreened and the chrome back (#130).
#[test]
fn f5_esc_restores_entry_page() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    // Park the editor on the middle page — the show's entry page.
    h.get_by_label("Page-2").click();
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 1);
    h.key_press(egui::Key::F5);
    h.run();
    assert!(h.state().debug_slideshow_for_test(), "F5 enters the show");
    assert_eq!(
        h.state().debug_page_idx_for_test(),
        0,
        "F5 starts from the first page"
    );
    assert_eq!(h.state().debug_fullscreen_for_test(), Some(true));
    // Advance two, then exit — the entry page returns, not page 2 of
    // the show's walk.
    h.key_press(egui::Key::ArrowRight);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 1);
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 2);
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().debug_slideshow_for_test(), "Esc exits the show");
    assert_eq!(
        h.state().debug_page_idx_for_test(),
        1,
        "exit restores the entry page"
    );
    assert_eq!(h.state().debug_fullscreen_for_test(), Some(false));
    assert!(!h.state().debug_chrome_hidden_for_test());
}

/// Shift+F5 starts the show from the CURRENT page (#130).
#[test]
fn shift_f5_starts_from_current_page() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.get_by_label("Page-3").click();
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::F5);
    h.run();
    assert!(h.state().debug_slideshow_for_test());
    assert_eq!(
        h.state().debug_page_idx_for_test(),
        2,
        "Shift+F5 starts from the current page"
    );
}

/// The navigation keys walk the deck and CLAMP at both ends — no wrap
/// (#130).
#[test]
fn navigation_clamps_at_the_ends() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    // Previous at the first slide stays.
    h.key_press(egui::Key::ArrowLeft);
    h.run();
    h.key_press(egui::Key::PageUp);
    h.run();
    h.key_press(egui::Key::Backspace);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 0);
    // Forward through the whole deck.
    for expect in 1..3 {
        h.key_press(egui::Key::PageDown);
        h.run();
        assert_eq!(h.state().debug_page_idx_for_test(), expect);
    }
    // Next at the last slide stays — arrows, Space and PageDown all.
    h.key_press(egui::Key::ArrowRight);
    h.run();
    h.key_press(egui::Key::Space);
    h.run();
    h.key_press(egui::Key::PageDown);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 2);
    // First / last jumps.
    h.key_press(egui::Key::Home);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 0);
    h.key_press(egui::Key::End);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 2);
}

/// While the show runs it owns the keyboard ahead of the app
/// shortcuts (#130, the Options-modal tier): F7 mid-show never opens
/// the spelling pass — and entering closed the one already open.
#[test]
fn show_owns_the_keyboard() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    // A flagged typo opens the spelling pass before the show.
    h.state_mut().debug_set_shape_text_for_test(2, "a tset word");
    h.run();
    h.key_press(egui::Key::F7);
    h.run();
    assert_eq!(h.state().debug_spell_word().as_deref(), Some("tset"));
    h.key_press(egui::Key::F5);
    h.run();
    assert_eq!(
        h.state().debug_spell_word(),
        None,
        "entering the show closes the open spelling pass (chrome)"
    );
    h.key_press(egui::Key::F7);
    h.run();
    assert_eq!(
        h.state().debug_spell_word(),
        None,
        "F7 never fires while the show owns the keyboard"
    );
    assert!(h.state().debug_slideshow_for_test());
}

/// A click on a shape whose first hyperlink row carries a SubAddress
/// jumps to that page (GOTOPAGE, no Ctrl); the show stays up (#130).
#[test]
fn subaddress_click_navigates() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    assert_eq!(h.state().debug_page_idx_for_test(), 0);
    // Shape 1 sits at the page center (4.25, 8), inches y-up.
    click_at(&mut h, show_point((4.25, 8.0)));
    assert_eq!(
        h.state().debug_page_idx_for_test(),
        2,
        "the SubAddress click went to Page-3"
    );
    assert!(
        h.state().debug_slideshow_for_test(),
        "the jump stays in the show"
    );
}

/// An address-only first row is inert in the show: no navigation, no
/// advance — and no OS spawn mid-presentation (#130).
#[test]
fn address_only_click_is_inert() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    // Shape 2 sits at (2, 2), inches y-up.
    click_at(&mut h, show_point((2.0, 2.0)));
    assert_eq!(
        h.state().debug_page_idx_for_test(),
        0,
        "an address-only link neither advances nor navigates"
    );
    assert!(h.state().debug_slideshow_for_test());
}

/// A click that hits no hyperlink advances — including the page's
/// blank paper and the letterbox surround (#130).
#[test]
fn plain_click_advances() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    click_at(&mut h, show_point((7.5, 10.5)));
    assert_eq!(h.state().debug_page_idx_for_test(), 1);
    // The black letterbox (left of the fitted page) advances too.
    click_at(&mut h, egui::pos2(60.0, 400.0));
    assert_eq!(h.state().debug_page_idx_for_test(), 2);
}

/// #130 closes the #209 §3.4 variance: Presentation Mode is full
/// screen — the toggle sends Fullscreen(true), the chip/Esc restore
/// sends Fullscreen(false).
#[test]
fn presentation_mode_toggles_fullscreen() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    // The footer Zone C chip (its accessible name is the tooltip).
    h.get_by_label("Presentation Mode").click();
    h.run();
    assert!(h.state().debug_chrome_hidden_for_test());
    assert_eq!(h.state().debug_fullscreen_for_test(), Some(true));
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().debug_chrome_hidden_for_test());
    assert_eq!(h.state().debug_fullscreen_for_test(), Some(false));
}

/// The show, mid-deck: black letterbox, the fitted white page with
/// its ink, no chrome anywhere (#130 golden).
#[test]
fn slideshow_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    h.run(); // second pass: the page texture settles
    h.snapshot_options(
        "dc_ui_slideshow",
        &egui_kittest::SnapshotOptions::new()
            .threshold(3.0)
            .max_failed_pixels(10_000),
    );
}

/// Review round 1: the page layers render ink-only over one white
/// sheet, so an oversized background page's red bar SHOWS through the
/// foreground page's empty paper — and its ink stays clipped to the
/// fitted page rect, never touching the black letterbox.
#[test]
fn background_ink_composes_and_clips_in_the_show() {
    let tmp = tempfile::tempdir().unwrap();
    let mut h = harness_with(&show_bg_fixture(tmp.path()));
    h.run();
    h.run();
    h.key_press(egui::Key::F5);
    h.run();
    h.run();
    h.run();
    let img = h.render().unwrap();
    // The fg page's fitted rect (8.5×11 in 1280×800, letterboxed).
    let r = fitted_rect(8.5, 11.0);
    let ppi = r.width() / 8.5;
    // The bg bar spans y 2..4 in (page inches, y-up) under the fg
    // sheet's EMPTY bottom half — with the opaque-paper bug it was
    // white; composing shows the red bar.
    let probe = |x_in: f64, y_in: f64| {
        let p = egui::pos2(
            r.left() + x_in as f32 * ppi,
            r.bottom() - y_in as f32 * ppi,
        );
        img.get_pixel(p.x as u32, p.y as u32)
    };
    let px = probe(4.25, 3.0);
    assert!(
        px[0] > 150 && px[1] < 110 && px[2] < 110,
        "the background bar composes under the fg paper: {px:?}"
    );
    // The same bar at natural size extends past the fitted rect (a
    // 17in-wide page at the fg ppi): where it crosses the sheet's
    // right edge it must be CLIPPED — the letterbox stays black.
    let lb = probe(11.0, 3.0);
    assert!(lb[0] < 12 && lb[1] < 12 && lb[2] < 12, "letterbox inked: {lb:?}");
    let lb2 = img.get_pixel(60, 400);
    assert!(lb2[0] < 12 && lb2[1] < 12 && lb2[2] < 12, "letterbox inked: {lb2:?}");
    // The foreground page's own ink still draws (its shape is at
    // (3.25..5.25, 7.5..8.5) — the bar below does not cover it).
    let fgpx = probe(4.25, 8.0);
    assert!(fgpx[0] > 200 && fgpx[1] > 200 && fgpx[2] > 200, "{fgpx:?}");
}
