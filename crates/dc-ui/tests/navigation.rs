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

/// Local CodeRabbit round 3: a scrollbar press must not arm canvas
/// gestures underneath — egui's `contains_pointer` ignores occlusion
/// (the canvas and the later-allocated bar responses all contain the
/// press), so a thumb drag over a shape would also arm that shape's
/// move-drag and drag it with the view.
#[test]
fn scrollbar_drag_over_a_shape_moves_only_the_view() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    // Place a master rectangle, then zoom until the page overflows.
    // The zoom anchors on the SELECTION (the placed shape), so the
    // shape holds its screen position while it grows; horizontal
    // wheel scrolls then carry it under the right-edge v-band.
    let from = h.state().debug_master_cells()[0].1.center();
    h.hover_at(from);
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let canvas = h.state().debug_canvas_rect();
    let c = canvas.center();
    let target = egui::pos2(c.x + canvas.width() * 0.2, c.y);
    h.hover_at(target);
    h.event(egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let shape = h
        .state()
        .debug_sel()
        .expect("placement selects the shape")
        .1;
    zoom_into_page(&mut h);
    let canvas = h.state().debug_canvas_rect();
    let c = canvas.center();
    for _ in 0..10 {
        let sr = h
            .state()
            .debug_shape_screen_rect(shape)
            .expect("the shape renders");
        if sr.center().x > canvas.right() - 60.0 {
            break;
        }
        wheel_at(&mut h, c, 10.0, 0.0, false);
    }
    let sr = h.state().debug_shape_screen_rect(shape).unwrap();
    let band_x = canvas.right() - 6.0;
    assert!(
        sr.contains(egui::pos2(band_x, sr.center().y))
            && sr.center().y > canvas.top() + 20.0
            && sr.center().y < canvas.bottom() - 40.0,
        "fixture: the shape covers the v-band at x={band_x}: {sr:?} canvas {canvas:?}"
    );
    let pinx = h.state().debug_cell_for_test(shape, "PinX");
    let piny = h.state().debug_cell_for_test(shape, "PinY");
    let (_, _, py0) = h.state().debug_view_for_test();
    // Press the v-band over the shape, drag down, release — a thumb
    // drag on the scrollbar.
    let press_y = sr.center().y;
    h.hover_at(egui::pos2(band_x, press_y));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(band_x, press_y),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for k in 1..=3 {
        h.hover_at(egui::pos2(band_x, press_y + 50.0 * k as f32));
        h.run();
    }
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(band_x, press_y + 150.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    // The view panned (the bar owned the gesture)…
    let (_, _, py1) = h.state().debug_view_for_test();
    assert!(py1 < py0, "the thumb drag panned the view: {py0} → {py1}");
    // …and the shape UNDER the band stayed put in the document —
    // BOTH axes (the drag is vertical, so PinY is the axis a
    // regressed move-drag would actually touch; Greptile round 7).
    assert_eq!(
        h.state().debug_cell_for_test(shape, "PinX"),
        pinx,
        "the shape under the scrollbar did not move (x)"
    );
    assert_eq!(
        h.state().debug_cell_for_test(shape, "PinY"),
        piny,
        "the shape under the scrollbar did not move (y)"
    );
}

/// Final local CodeRabbit pass: grabbing a SATURATED thumb — the view
/// parked far past the mapped range by #286's unrestricted panning —
/// must not snap back into range on the first pixel. The drag moves
/// the pan by the scaled pointer displacement (~4 px of pan per track
/// px on this fixture); the old absolute inverse jumped the view by
/// the whole out-of-range overshoot (tens of thousands of px here).
#[test]
fn saturated_thumb_drag_moves_the_view_smoothly() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    zoom_into_page(&mut h);
    // Scroll far past the paper: pan leaves thumb_geom's mapped
    // range and the thumb pins at a track edge.
    let c = h.state().debug_canvas_rect().center();
    for _ in 0..6 {
        wheel_at(&mut h, c, 0.0, -500.0, false);
    }
    // Let egui's wheel-smoothing tail drain (it decays for a few
    // frames after the events — the measurement must be stable).
    for _ in 0..12 {
        h.run();
    }
    let (_, _, py0) = h.state().debug_view_for_test();
    h.run();
    h.run();
    let (_, _, py0b) = h.state().debug_view_for_test();
    assert!(
        (py0 - py0b).abs() < 0.5,
        "fixture: the view settled before the drag ({py0} vs {py0b})"
    );
    // A SHORT thumb drag from mid-track: the pan moves smoothly.
    let canvas = h.state().debug_canvas_rect();
    let bar_x = canvas.right() - 7.0;
    let press_y = canvas.center().y;
    h.hover_at(egui::pos2(bar_x, press_y));
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, press_y),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for k in 1..=3 {
        h.hover_at(egui::pos2(bar_x, press_y + 10.0 * k as f32));
        h.run();
    }
    h.event(egui::Event::PointerButton {
        pos: egui::pos2(bar_x, press_y + 30.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let (_, _, py1) = h.state().debug_view_for_test();
    assert!(py1 < py0, "the drag still scrolls: {py0} → {py1}");
    // The drag pans by the pointer displacement × the track↔span
    // scale — ≈18× on this fixture (the synthetic wheel events push
    // zoom to its 8.0 clamp, page ≈ 8400 px, so a 30 px drag moves
    // the view ≈530 px). The old absolute inverse snapped by the
    // whole out-of-range overshoot (tens of thousands of px here) —
    // a bound of 1500 separates the two decisively.
    assert!(
        py0 - py1 < 1500.0,
        "no snap from the saturated range: Δ = {}",
        py0 - py1
    );
}

// #211 (URS-1051): the Pan & Zoom window — the floating overview with
// the live viewport rectangle. Opened from View ▸ Task Panes ▸ Pan &
// Zoom; every write it makes is view-only (never undo history).

/// Open the Pan & Zoom window through the ribbon route and settle.
fn open_pan_zoom(h: &mut Harness<'static, App>) {
    h.get_by_label("View").click();
    h.run();
    h.get_by_label("Task Panes").click();
    h.run();
    h.get_by_label("Pan & Zoom").click();
    h.run();
    h.run();
}

/// Type a percent into the window's zoom entry and commit it with
/// Enter (the Size & Position field-edit route).
fn pan_zoom_enter_pct(h: &mut Harness<'static, App>, pct: &str) {
    use egui_kittest::kittest::By;
    let at = h
        .get(By::default().role(egui::Role::TextInput).label("Zoom"))
        .rect()
        .center();
    h.hover_at(at);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    h.get(By::default().role(egui::Role::TextInput).label("Zoom"))
        .set_text_value_accesskit(pct);
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
}

/// A raw press-drag-release over the Pan & Zoom thumbnail.
fn pan_zoom_drag(h: &mut Harness<'static, App>, from: egui::Pos2, by: egui::Vec2) {
    h.hover_at(from);
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    for k in 1..=4 {
        h.hover_at(from + by * (k as f32 / 4.0));
        h.run();
    }
    h.event(egui::Event::PointerButton {
        pos: from + by,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
}

/// The blank fixture's page (inches) — the thumb↔page scale derives
/// from it and the drawn paper width.
const BLANK_PAGE_W: f32 = 8.5;

/// Dragging the viewport rectangle pans the canvas 1:1 (page inches
/// under the pointer follow the pointer exactly) with the zoom
/// untouched — and nothing lands in undo history.
#[test]
fn pan_zoom_thumb_drag_pans_one_to_one_without_undo() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    open_pan_zoom(&mut h);
    let thumb = h.state().debug_panzoom_thumb();
    let vp = h.state().debug_panzoom_vp();
    assert!(thumb.is_positive(), "the thumbnail paper paints");
    assert!(vp.is_positive(), "the viewport rectangle paints");
    let undo0 = h.state().debug_undo_len();
    let (z0, px0, py0) = h.state().debug_view_for_test();

    // Drag the rectangle by (−36, −24) THUMB px: the pan must move by
    // the same page-inch displacement, opposite on screen (the page
    // follows the pointer 1:1).
    let by = egui::vec2(-36.0, -24.0);
    pan_zoom_drag(&mut h, vp.center(), by);
    let (z1, px1, py1) = h.state().debug_view_for_test();
    assert!((z1 - z0).abs() < 1e-6, "a thumb drag never zooms");
    let s = thumb.width() / BLANK_PAGE_W; // thumb px per page inch
    let ppi = 96.0 * z0; // canvas px per page inch
    let ex = px0 - by.x / s * ppi;
    let ey = py0 - by.y / s * ppi;
    assert!(
        (px1 - ex).abs() < 1.5,
        "the pan follows the drag 1:1 in x: {px1} vs {ex}"
    );
    assert!(
        (py1 - ey).abs() < 1.5,
        "the pan follows the drag 1:1 in y: {py1} vs {ey}"
    );
    assert_eq!(
        h.state().debug_undo_len(),
        undo0,
        "the Pan & Zoom drag is view-only: no undo entries"
    );
}

/// The rectangle tracks the live view: a zoom from the entry shrinks
/// it proportionally, a canvas ctrl+wheel zoom shrinks it again, and
/// Fit Page restores it to the whole paper.
#[test]
fn pan_zoom_rectangle_tracks_the_live_view() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    open_pan_zoom(&mut h);
    // The fitted rect at open — the reference Whole Page must return
    // to (the same clamped-to-paper computation both times).
    let vp_fit = h.state().debug_panzoom_vp();
    // Start from an UNCLAMPED rectangle: at 400 % the page overflows
    // the view, so the drawn rect maps the viewport 1:1 (a fitted
    // rect is clamped to the paper and carries no zoom information).
    pan_zoom_enter_pct(&mut h, "400");
    let vp0 = h.state().debug_panzoom_vp();
    let (z0, _, _) = h.state().debug_view_for_test();

    // A 200 % entry zoom halves the visible page fraction.
    pan_zoom_enter_pct(&mut h, "200");
    let vp1 = h.state().debug_panzoom_vp();
    let (z1, _, _) = h.state().debug_view_for_test();
    assert!((z1 - 2.0).abs() < 1e-6, "the entered percent applies: {z1}");
    let expect = vp0.width() * z0 / z1;
    assert!(
        (vp1.width() - expect).abs() < 0.03 * expect + 2.0,
        "the rectangle tracks the entry zoom: {} vs {expect}",
        vp1.width()
    );

    // A canvas ctrl+wheel zoom (hovered clear of the floating window)
    // shrinks it live.
    let canvas = h.state().debug_canvas_rect();
    let at = egui::pos2(canvas.left() + canvas.width() * 0.2, canvas.center().y);
    wheel_at(&mut h, at, 0.0, 10.0, true);
    wheel_at(&mut h, at, 0.0, 10.0, true);
    h.run();
    let vp2 = h.state().debug_panzoom_vp();
    assert!(
        vp2.width() < vp1.width() && vp2.height() < vp1.height(),
        "the rectangle tracks the wheel zoom: {} then {}",
        vp1.width(),
        vp2.width()
    );

    // Fit Page (the ribbon button is on the already-active View tab)
    // returns to the fitted overview rect.
    h.get_by_label("Whole Page").click();
    h.run();
    h.run();
    let vp3 = h.state().debug_panzoom_vp();
    assert!(
        (vp3.width() - vp_fit.width()).abs() < 1.0
            && (vp3.height() - vp_fit.height()).abs() < 1.0,
        "the rectangle tracks the fit: {vp3:?} vs {vp_fit:?}"
    );
}

/// The entered percent FLOORS onto the raster bucket grid (#67/#141)
/// and clamps to the 10–800 % window; a press OUTSIDE the rectangle
/// recenters the view there.
#[test]
fn pan_zoom_percent_floors_and_outside_press_recenters() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    open_pan_zoom(&mut h);
    // 33 % floors onto the 1/384 grid: floor(0.33·384) = 126.
    pan_zoom_enter_pct(&mut h, "33");
    assert_eq!(h.state().debug_view_for_test().0, 126.0 / 384.0);
    // Out-of-range values clamp, not refuse.
    pan_zoom_enter_pct(&mut h, "5000");
    assert_eq!(h.state().debug_view_for_test().0, 8.0);

    // At the 8.0 clamp the rectangle is a small centered box — a
    // press on the paper OUTSIDE it recenters the view there (the new
    // rectangle's center lands on the pressed point).
    pan_zoom_enter_pct(&mut h, "800");
    let thumb = h.state().debug_panzoom_thumb();
    let vp = h.state().debug_panzoom_vp();
    assert!(
        vp.width() < thumb.width() * 0.5,
        "zoomed in far enough that paper lies outside the rectangle"
    );
    let at = egui::pos2(thumb.left() + 8.0, thumb.top() + 8.0);
    h.hover_at(at);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    h.event(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    let vp2 = h.state().debug_panzoom_vp();
    assert!(
        vp2.center().distance(at) < 2.5,
        "the press outside recenters the view there: {:?} vs {at:?}",
        vp2.center()
    );

    // Out-of-range values clamp low as well as high.
    pan_zoom_enter_pct(&mut h, "5");
    assert_eq!(h.state().debug_view_for_test().0, 0.1);
}

/// PR #413 review: a discrete view write that lands AFTER the overview
/// painted — here a plain wheel SCROLL, the sharpest case (pan-only:
/// no ppi re-bucket, so the layer settle machinery requests nothing
/// either) — must be detected and a catch-up frame requested, or the
/// rectangle sits one interaction behind until unrelated input arrives.
/// kittest's `run()` forces frames by itself, so the DETECTION is what
/// this pins (the `debug_panzoom_view_lags` predicate `draw` turns into
/// the repaint); the follow-up assertion checks the rectangle then
/// follows the scrolled view.
#[test]
fn pan_zoom_rectangle_requests_its_catch_up_frame() {
    let mut h = harness_with_blank();
    h.run();
    h.run();
    open_pan_zoom(&mut h);
    // Zoom until the rectangle is unclamped, then settle: nothing owed.
    pan_zoom_enter_pct(&mut h, "200");
    h.run();
    assert!(
        !h.state().debug_panzoom_view_lags(),
        "settled: the rectangle agrees with the view"
    );
    let vp0 = h.state().debug_panzoom_vp();

    // ONE plain wheel scroll over the canvas, ONE pass.
    let c = h.state().debug_canvas_rect().center();
    h.hover_at(c);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -120.0),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.step();
    assert!(
        h.state().debug_panzoom_view_lags(),
        "the scroll wrote the view after the overview painted — a catch-up frame is owed"
    );

    // The owed frame runs and the rectangle follows the scroll (wheel
    // down: the page moves up the screen, the viewport slides DOWN the
    // thumbnail).
    h.run();
    assert!(
        !h.state().debug_panzoom_view_lags(),
        "the catch-up frame redrew the rectangle from the current view"
    );
    let vp1 = h.state().debug_panzoom_vp();
    assert!(
        vp1.top() > vp0.top() + 1.0,
        "the rectangle followed the scroll: {} → {}",
        vp0.top(),
        vp1.top()
    );
}
