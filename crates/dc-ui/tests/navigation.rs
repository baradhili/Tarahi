//! #283: base canvas navigation — scrollbars, wheel semantics, pan
//! clamping, the Pan tool, and Ctrl+Home/End. The wheel semantics are
//! a deliberate muscle-memory change (Visio 2016 parity, owner
//! decision on the issue): plain wheel SCROLLS, Ctrl/Cmd+wheel zooms,
//! and the two branches are mutually exclusive.

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use dc_ui::app::App;

fn harness_with_blank() -> Harness<'static, App> {
    dc_ui::svgimg::pin_determinism_for_tests();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_max_steps(16)
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            App::with_blank(),
        )
}

/// A raw mouse-wheel event, cursor parked at `pos` first (the canvas
/// reads `hover_pos`, so the pointer must be over it).
fn wheel_at(h: &mut Harness<'static, App>, pos: egui::Pos2, dx: f32, dy: f32, ctrl: bool) {
    h.hover_at(pos);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(dx * 12.0, dy * 12.0),
        modifiers: if ctrl {
            egui::Modifiers::CTRL
        } else {
            egui::Modifiers::NONE
        },
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run();
}

/// Zoom in far enough that the page overflows the view (the scroll
/// range un-pins).
fn zoom_into_page(h: &mut Harness<'static, App>) {
    let c = h.state().debug_canvas_rect().center();
    for _ in 0..8 {
        wheel_at(h, c, 0.0, 10.0, true);
    }
    h.run();
}

/// Plain wheel scrolls (unbounded — #286 removed the pan limits);
/// horizontal wheel input scrolls horizontally too.
#[test]
fn wheel_scrolls_unbounded() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    let (_, px0, py0) = h.state().debug_view_for_test();
    assert!(px0 > 0.0 && py0 > 0.0, "a fitted blank view is on-screen");
    zoom_into_page(&mut h);
    let (_, _, py1) = h.state().debug_view_for_test();
    assert!(py1 < py0, "ctrl+wheel zooms in: {py0} → {py1}");

    // Plain wheel down scrolls the page up (pan.y shrinks).
    let c = h.state().debug_canvas_rect().center();
    wheel_at(&mut h, c, 0.0, -10.0, false);
    let (_, _, py2) = h.state().debug_view_for_test();
    assert!(py2 < py1, "plain wheel scrolls: {py1} → {py2}");

    // #286: scrolling is UNRESTRICTED — huge scrolls keep moving the
    // view (the #283 extent clamp is gone; users can park the drawing
    // anywhere and use View ▸ Whole Page to come back).
    for _ in 0..3 {
        wheel_at(&mut h, c, 0.0, -500.0, false);
    }
    let (_, _, py3) = h.state().debug_view_for_test();
    for _ in 0..3 {
        wheel_at(&mut h, c, 0.0, -500.0, false);
    }
    let (_, _, py4) = h.state().debug_view_for_test();
    assert!(
        py4 < py3,
        "scrolling past the paper keeps moving the view: {py3} → {py4}"
    );

    // Horizontal wheel input scrolls horizontally (egui routes
    // Shift+wheel / trackpad swipes into smooth_scroll_delta.x — the
    // axis the canvas never read before #283).
    let (_, px2, _) = h.state().debug_view_for_test();
    wheel_at(&mut h, c, -10.0, 0.0, false);
    let (_, px3, _) = h.state().debug_view_for_test();
    assert!(
        (px3 - px2).abs() > 0.5,
        "horizontal wheel input scrolls: {px2} → {px3}"
    );
}

/// Ctrl+wheel zooms INSTEAD of scrolling (the branches are mutually
/// exclusive by egui's routing — the scroll delta is zeroed under the
/// zoom modifier). The observable: a ctrl+wheel-down step ZOOMS OUT,
/// and pan never slams into the scroll clamp bound the way the same
/// delta fired as a scroll would.
#[test]
fn ctrl_wheel_zooms_without_scrolling_the_wheel_step() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    zoom_into_page(&mut h);
    let (z_before, px_before, py_before) = h.state().debug_view_for_test();
    let canvas = h.state().debug_canvas_rect();
    let c = canvas.center();
    wheel_at(&mut h, c, 0.0, -10.0, true);
    h.run();
    h.run();
    let (z_after, px_after, py_after) = h.state().debug_view_for_test();
    assert!(
        z_after < z_before,
        "ctrl+wheel-down zooms out: {z_before} → {z_after}"
    );
    // The zoom is ANCHORED — the hovered point keeps its page
    // position — so BOTH pan axes move exactly by the relation
    // pan' = anchor − (anchor − pan)·k, k = z_after/z_before (Greptile
    // PR #284 round 6: an is_finite() check caught nothing — a wheel
    // delta that ALSO scrolled, or any pan limit creeping back in
    // (#286 removed them), breaks this equality instead).
    let k = z_after / z_before;
    let ex = c.x - (c.x - px_before) * k;
    let ey = c.y - (c.y - py_before) * k;
    assert!(
        (px_after - ex).abs() < 0.25,
        "x pan follows the anchored zoom: {px_after} vs {ex}"
    );
    assert!(
        (py_after - ey).abs() < 0.25,
        "y pan follows the anchored zoom: {py_after} vs {ey}"
    );
}

/// The vertical thumb drag pans the view; a track click page-jumps.
#[test]
fn scrollbar_thumb_drag_and_track_click_move_the_view() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    zoom_into_page(&mut h);
    let canvas = h.state().debug_canvas_rect();
    let (_, _, py0) = h.state().debug_view_for_test();

    // The vertical bar hugs the right edge (LTR shell): press the
    // track's upper region, drag downward, release.
    let bar_x = canvas.right() - 7.0;
    let top = canvas.top() + 40.0;
    h.hover_at(egui::pos2(bar_x, top));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, top),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for k in 1..=4 {
        h.hover_at(egui::pos2(bar_x, top + 60.0 * k as f32));
        h.run();
    }
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, top + 240.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let (_, _, py1) = h.state().debug_view_for_test();
    // Standard scrollbar direction (PR #284 review): dragging the
    // thumb DOWN scrolls DOWN — pan.y decreases (the page moves up
    // the screen).
    assert!(
        py1 < py0,
        "dragging the v-thumb downward scrolls down: {py0} → {py1}"
    );

    // A track click ABOVE the thumb page-jumps UP (the thumb centers
    // on the click, Visio's track behavior): scrolling up increases
    // pan.y.
    h.hover_at(egui::pos2(bar_x, canvas.top() + 10.0));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, canvas.top() + 10.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, canvas.top() + 10.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let (_, _, py2) = h.state().debug_view_for_test();
    assert!(
        py2 > py1,
        "a track click above the thumb jumps the view up: {py1} → {py2}"
    );
}

/// Ctrl+Home / Ctrl+End jump the view to the page's top / bottom, and
/// a repeated jump stays pinned at the bound.
#[test]
fn ctrl_home_end_jump_the_page_edges() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    zoom_into_page(&mut h);
    let (_, _, py0) = h.state().debug_view_for_test();

    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::End);
    h.run();
    let (_, _, py_end) = h.state().debug_view_for_test();
    assert!(py_end < py0, "ctrl+end jumps toward the page bottom");

    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Home);
    h.run();
    let (_, _, py_home) = h.state().debug_view_for_test();
    assert!(py_home > py_end, "ctrl+home jumps toward the page top");
    // The jumps target the PAPER edges exactly (PR #284 review round
    // 5 — not the manual-pan range bounds, which would hide half the
    // page on a fitted drawing), so repeats land identically.
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Home);
    h.run();
    let (_, _, py_home2) = h.state().debug_view_for_test();
    assert_eq!(py_home, py_home2, "ctrl+home repeats identically");
}

/// The Pan tool (Home ▸ Tools): a plain left drag pans, clicks and
/// double-clicks on a SHAPE change nothing (PR #284 review — the
/// regression needs a shape on the page to guard), and Esc restores
/// the pointer tool's selection behavior.
#[test]
fn pan_tool_drags_the_view_and_clicks_select_nothing() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    // A shape on the page: click the first Quick Shapes master tile,
    // then the canvas center places it (the stencil paradigm).
    let cell = h.state().debug_master_cells()[0].1.center();
    h.hover_at(cell);
    h.event(egui::Event::PointerButton {
        pos: cell,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: cell,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let canvas = h.state().debug_canvas_rect();
    let center = canvas.center();
    h.hover_at(center);
    h.event(egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let (_, shape) = h.state().debug_sel().expect("the placed shape is selected");
    // Deselect with the pointer tool first: Pan's inert clicks are
    // then provable against an EMPTY selection.
    let empty = canvas.left_top() + egui::vec2(30.0, 30.0);
    h.hover_at(empty);
    h.event(egui::Event::PointerButton {
        pos: empty,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: empty,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert_eq!(h.state().debug_sel(), None, "deselected before the tool");

    // Arm the Pan tool: the button's ACCESSIBLE label is its
    // compact-tier alt text ("Pan") — exact match, so the planned
    // Task panes stub (whose label contains "Pan & Zoom") can't
    // shadow it.
    h.get_by_label("Pan").click();
    h.run();

    // A left drag over the canvas pans (the tool's gesture).
    let (_, _, py0) = h.state().debug_view_for_test();
    let start = center - egui::vec2(60.0, -60.0);
    h.hover_at(start);
    h.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for k in 1..=4 {
        h.hover_at(start + egui::vec2(0.0, 40.0 * k as f32));
        h.run();
    }
    h.event(egui::Event::PointerButton {
        pos: start + egui::vec2(0.0, 160.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let (_, _, py1) = h.state().debug_view_for_test();
    assert!(
        py1 > py0,
        "the Pan tool's left drag pans down the page: {py0} → {py1}"
    );

    // A click ON the shape with the Pan tool selects NOTHING (the
    // pointer tool would select it) — the click gate's regression.
    let sr = h.state().debug_shape_screen_rect(shape).expect("the shape");
    let sc = sr.center();
    h.hover_at(sc);
    h.event(egui::Event::PointerButton {
        pos: sc,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: sc,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert_eq!(
        h.state().debug_sel(),
        None,
        "Pan clicks are inert on shapes"
    );

    // Esc returns to the pointer tool — which then SELECTS on click
    // (the tool contract, asserted behaviorally per the PR review).
    h.key_press(egui::Key::Escape);
    h.run();
    h.hover_at(sc);
    h.event(egui::Event::PointerButton {
        pos: sc,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: sc,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let sel = h.state().debug_sel();
    assert_eq!(
        sel.map(|(_, id)| id),
        Some(shape),
        "Esc restores the pointer tool's selection behavior"
    );
}
