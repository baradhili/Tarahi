//! The Pan & Zoom window (#211, URS-1051): a floating task pane with
//! a page thumbnail and the live viewport rectangle (Visio 2016's
//! View ▸ Task Panes ▸ Pan & Zoom). Dragging the rectangle pans the
//! canvas 1:1; pressing/dragging outside it recenters the view there;
//! the −/+ steps and the percent entry set the zoom — bucket-floored
//! onto the raster ppi grid (#67/#141's fit lesson) and anchored at
//! the visible center so the rectangle grows about its middle. Every
//! write is view-only: like the wheel and the scrollbars, nothing here
//! can enter undo history. The thumbnail itself renders through the
//! existing `TextureCache` + raster pool at a small fixed scale — the
//! Info tab's pipeline, keyed by the page part and invalidated on
//! every version bump, no new render path.

use super::*;

/// The thumbnail's page box (screen px): the paper letterboxes inside
/// it, aspect-true. Visio's pane is a compact overview — the drawing
/// reads as shapes, not glyphs.
const THUMB_BOX: Vec2 = Vec2::new(216.0, 160.0);

/// Visio 2016's viewport rectangle: a red outline (a faint red fill
/// keeps it findable over white paper).
const VP_RED: Color32 = Color32::from_rgb(0xE2, 0x1B, 0x1B);

impl App {
    /// The visible page rectangle, y-down inches, UNCLAMPED (the view
    /// can sit partly off the paper) — from the last frame's canvas
    /// content area; the whole page until a canvas exists.
    fn panzoom_viewport(&self, geom: &PageGeom) -> (f64, f64, f64, f64) {
        if self.canvas_rect.is_positive() {
            let (origin, content) = canvas::content_area(self.canvas_rect, self.view.show_rulers);
            let tl = canvas::screen_to_page(origin, &self.view, geom);
            let br = canvas::screen_to_page(origin + content, &self.view, geom);
            // page y-up → the thumbnail's y-down frame.
            (
                tl.0.min(br.0),
                geom.h_in - tl.1.max(br.1),
                tl.0.max(br.0),
                geom.h_in - tl.1.min(br.1),
            )
        } else {
            (0.0, 0.0, geom.w_in, geom.h_in)
        }
    }

    /// Park the view so the page point `c` (y-down inches) sits at the
    /// canvas content area's center — the Pan & Zoom pan write. The
    /// zoom is untouched, so the viewport center follows the pointer
    /// exactly 1:1 in page inches. View-only, never undoable.
    fn panzoom_center_view(&mut self, c: (f64, f64)) {
        let (origin, content) = canvas::content_area(self.canvas_rect, self.view.show_rulers);
        let ppi = self.view.px_per_in() as f64;
        let center = origin + content / 2.0;
        self.view.pan = Vec2::new(
            center.x - (c.0 * ppi) as f32,
            center.y - (c.1 * ppi) as f32,
        );
        self.view.fit_mode = false;
    }

    /// One zoom write from the window (the % entry or a −/+ step).
    /// `pct` is a percent: it clamps to the 10–800 window range, floors
    /// onto the 1/384 raster-bucket grid (`View::apply_fit`'s rule —
    /// the layer store quantizes ppi, and an unfloored zoom would
    /// re-raster every layer across each settle nudge), and anchors at
    /// the visible center so the thumbnail rectangle scales about its
    /// own middle. View-only, never undoable.
    fn panzoom_set_zoom(&mut self, pct: f32) {
        if !pct.is_finite() {
            return;
        }
        let z = ((pct.clamp(10.0, 800.0) / 100.0) * 384.0).floor() / 384.0;
        let z = z.clamp(0.1, 8.0);
        let (origin, content) = canvas::content_area(self.canvas_rect, self.view.show_rulers);
        let anchor = origin + content / 2.0;
        let old = self.view.zoom;
        if old > 0.0 {
            let k = z / old;
            self.view.pan = anchor.to_vec2() - (anchor.to_vec2() - self.view.pan) * k;
        }
        self.view.zoom = z;
        self.view.fit_mode = false;
    }

    /// #211 (PR #413 review): whether the overview's rectangle lags
    /// the live view — the view was written after the overview painted
    /// this frame (wheel zoom/scroll, Fit Page, the zoom chrome) and a
    /// catch-up frame is owed. `draw` turns this into the repaint that
    /// delivers it; None (window not painting) never lags.
    pub(super) fn panzoom_view_lags(&self) -> bool {
        self.panzoom_view_drawn
            .is_some_and(|v| v != (self.view.zoom, self.view.pan.x, self.view.pan.y))
    }

    /// The Pan & Zoom window itself (View ▸ Task Panes ▸ Pan & Zoom,
    /// #211). Nothing paints without a document — the Task Panes
    /// entries arm only with one, and a closed document keeps the flag
    /// for the next open (the Size & Position contract).
    pub(super) fn panzoom_window(&mut self, ctx: &egui::Context) {
        self.panzoom_thumb = Rect::NOTHING;
        self.panzoom_vp = Rect::NOTHING;
        if !self.panzoom_open || self.ed.is_none() {
            self.panzoom_grab = None;
            self.panzoom_view_drawn = None;
            return;
        }
        let geom = self.page_geom();
        let (page_w, page_h) = (geom.w_in as f32, geom.h_in as f32);
        let part = {
            let ed = self.ed.as_ref().unwrap();
            ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))]
                .part
                .clone()
        };
        // The thumbnail raster: the Info tab's cache-and-pool path at
        // the letterbox scale, keyed by page part + version so every
        // edit refreshes it and nothing else re-renders. `None` for a
        // frame while the pool works — the white paper shows through.
        let scale = (THUMB_BOX.x / page_w).min(THUMB_BOX.y / page_h);
        let key = format!("panzoom-{}", part.replace('/', "_"));
        let thumb = self
            .tex
            .get(
                ctx,
                &mut self.pool,
                geom.w_in,
                &key,
                self.version,
                scale,
                || {
                    let ed = self.ed.as_ref().unwrap();
                    let page =
                        &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
                    dc::svg::render_page(&ed.doc, page)
                },
            )
            .ok()
            .flatten()
            .cloned();
        let mut open = true;
        let mut step: Option<f32> = None;
        let mut commit: Option<f32> = None;
        let tip = crate::i18n::strs().msg_pan_zoom_tip();
        egui::Window::new(crate::i18n::strs().msg_pan_zoom())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                // --- the thumbnail ---
                let (box_r, resp) = ui.allocate_exact_size(THUMB_BOX, Sense::click_and_drag());
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, &tip)
                });
                let resp = resp.on_hover_text(&tip);
                let paper = Rect::from_center_size(
                    box_r.center(),
                    Vec2::new(page_w * scale, page_h * scale),
                );
                let t = theme::shell_tokens(ui.visuals());
                ui.painter().rect_filled(paper, 1.0, Color32::WHITE);
                ui.painter().rect_stroke(
                    paper,
                    1.0,
                    Stroke::new(1.0, t.ribbon_group),
                    egui::StrokeKind::Middle,
                );
                if let Some(tex) = &thumb {
                    ui.painter().image(
                        tex.id(),
                        paper,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                // The live viewport rectangle, clamped to the paper for
                // drawing (the view may sit partly off it).
                let vp = self.panzoom_viewport(&geom);
                let to_x =
                    |v: f64| paper.left() + (v as f32).clamp(0.0, page_w) * scale;
                let to_y =
                    |v: f64| paper.top() + (v as f32).clamp(0.0, page_h) * scale;
                let vr = Rect::from_min_max(
                    Pos2::new(to_x(vp.0), to_y(vp.1)),
                    Pos2::new(to_x(vp.2), to_y(vp.3)),
                );
                ui.painter().rect_filled(
                    vr,
                    0.0,
                    Color32::from_rgba_unmultiplied(0xE2, 0x1B, 0x1B, 24),
                );
                ui.painter().rect_stroke(
                    vr,
                    0.0,
                    Stroke::new(1.5, VP_RED),
                    egui::StrokeKind::Middle,
                );
                self.panzoom_thumb = paper;
                self.panzoom_vp = vr;
                // The view this rectangle was drawn FROM (captured
                // before the gesture section below can write the view
                // — those in-frame writes are exactly what the
                // end-of-frame check in `draw` must see).
                self.panzoom_view_drawn = Some((
                    self.view.zoom,
                    self.view.pan.x,
                    self.view.pan.y,
                ));
                // --- the gesture ---
                // Arm on the PRESS frame (the canvas's #59 lesson:
                // egui's click-vs-drag disambiguation reports late, so
                // the grab offset and the outside-recenter must be
                // decided while the button goes down). A press inside
                // the rectangle grabs it (center − pointer); a press
                // outside recenters immediately — a click is a
                // zero-travel drag.
                let page_of = |p: Pos2| -> (f64, f64) {
                    (
                        ((p.x - paper.left()) / scale) as f64,
                        ((p.y - paper.top()) / scale) as f64,
                    )
                };
                if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        let pp = page_of(p);
                        let center = ((vp.0 + vp.2) / 2.0, (vp.1 + vp.3) / 2.0);
                        let inside = pp.0 >= vp.0.min(vp.2)
                            && pp.0 <= vp.0.max(vp.2)
                            && pp.1 >= vp.1.min(vp.3)
                            && pp.1 <= vp.1.max(vp.3);
                        if inside {
                            self.panzoom_grab = Some((center.0 - pp.0, center.1 - pp.1));
                        } else {
                            self.panzoom_grab = Some((0.0, 0.0));
                            self.panzoom_center_view(pp);
                        }
                    }
                }
                if resp.dragged() {
                    if let (Some(g), Some(p)) = (
                        self.panzoom_grab,
                        ui.input(|i| i.pointer.interact_pos()),
                    ) {
                        let pp = page_of(p);
                        self.panzoom_center_view((pp.0 + g.0, pp.1 + g.1));
                    }
                }
                // Release ends the gesture — NOT "!dragged()": egui's
                // drag decision arrives frames after the press, and
                // clearing on non-drag frames would wipe the grab the
                // press just armed.
                if !ui.input(|i| i.pointer.primary_down()) {
                    self.panzoom_grab = None;
                }
                // --- the zoom row: − [ % ] + ---
                ui.add_space(4.0);
                row(ui, |ui| {
                    if ui
                        .button("−")
                        .on_hover_text(crate::i18n::strs().msg_zoom_out())
                        .clicked()
                    {
                        step = Some(self.view.zoom * 100.0 / 1.25);
                    }
                    // The entry tracks the live view while not focused
                    // (the Size & Position buffer pattern); a committed
                    // edit floors onto the bucket grid in
                    // `panzoom_set_zoom`.
                    let id = egui::Id::unique("dc-panzoom-pct");
                    let shown = format!("{}", (self.view.zoom * 100.0).round() as u32);
                    if !ui.memory(|m| m.has_focus(id)) {
                        self.panzoom_pct = shown.clone();
                    }
                    let zoom_label = crate::i18n::strs().msg_zoom();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.panzoom_pct)
                            .id(id)
                            .desired_width(52.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, &zoom_label)
                    });
                    ui.label("%");
                    if r.lost_focus() && self.panzoom_pct != shown {
                        match sizepos::parse_field(&self.panzoom_pct) {
                            Some(v) => commit = Some(v as f32),
                            None => self.panzoom_pct = shown,
                        }
                    }
                    if ui
                        .button("+")
                        .on_hover_text(crate::i18n::strs().msg_zoom_in())
                        .clicked()
                    {
                        step = Some(self.view.zoom * 100.0 * 1.25);
                    }
                });
            });
        if !open {
            self.panzoom_open = false;
            self.panzoom_grab = None;
        }
        if let Some(pct) = step.or(commit) {
            self.panzoom_set_zoom(pct);
        }
    }

    /// #211: the View ▸ Task Panes dropdown — Visio's task-pane
    /// visibility menu. Pan & Zoom opens the window; Size & Position
    /// moves under the menu from its #210 standalone button (that
    /// button's own comment said it stood in "until Task Panes is a
    /// menu"). The anchor comes from the dispatch site (the pointer,
    /// when there is one — the click sits on the ribbon button).
    pub(super) fn task_panes_menu(&mut self, ctx: &egui::Context) {
        let Some(anchor) = self.task_panes_menu else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.task_panes_menu = None;
            self.task_panes_fresh = false;
            return;
        }
        let has_doc = self.ed.is_some();
        let mut pick: Option<Action> = None;
        let menu = egui::Popup::new(
            egui::Id::unique("dc-task-panes-menu"),
            ctx.clone(),
            anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup = menu.show(|ui| {
            ui.set_min_width(176.0);
            // Checkable entries — the open window's row carries the
            // selected treatment (Visio's pane menus check their open
            // panes).
            let pz = ui.add_enabled(
                has_doc,
                egui::Button::new(crate::i18n::strs().msg_pan_zoom())
                    .small()
                    .selected(self.panzoom_open),
            );
            let pz = pz.on_hover_text(crate::i18n::strs().msg_pan_zoom_tip());
            if pz.clicked() {
                pick = Some(Action::TogglePanZoom);
            }
            let sp = ui.add_enabled(
                has_doc,
                egui::Button::new(crate::i18n::strs().msg_size_position())
                    .small()
                    .selected(self.size_pos_open),
            );
            let sp = sp.on_hover_text(crate::i18n::strs().msg_size_position_tip());
            if sp.clicked() {
                pick = Some(Action::ToggleSizePosition);
            }
        });
        if let Some(a) = pick {
            self.task_panes_menu = None;
            self.task_panes_fresh = false;
            self.dispatch(a);
            return;
        }
        // Outside-primary close (the page context menu's idiom). Gated
        // one frame: the click that OPENED the menu is itself a
        // primary click whose release must not close the menu it made.
        if self.task_panes_fresh {
            self.task_panes_fresh = false;
            return;
        }
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.task_panes_menu = None;
            }
        }
    }
}
