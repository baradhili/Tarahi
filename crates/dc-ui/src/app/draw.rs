//! Per-frame drawing: the stencil carry update, the eframe entry
//! point, and the full testable shell draw.

use super::*;

impl App {
    /// Drag-and-drop from the stencil pane: while the primary button is
    /// held after leaving a master cell, ghost the shape at the cursor
    /// (thumbnail at real shape size); on release over the canvas,
    /// place an instance there. Click-to-arm then click-to-place still
    /// works alongside this.
    pub(super) fn stencil_carry(&mut self, ctx: &Context) {
        let Some(key) = self.stencil.carry else {
            return;
        };
        let (down, released, pos) = ctx.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
            )
        });
        if !down && !released {
            self.stencil.carry = None; // button lost (cancelled elsewhere)
            return;
        }
        let Some(p) = pos else { return };
        let ppi = self.view.px_per_in();
        if down {
            ctx.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            let Some(ed) = self.ed.as_ref() else { return };
            // The ghost renders from the master's own source document
            // (#82): a stencil master need not exist in the edited
            // document (it is copied in only on placement, #84).
            let Some((serial, src, m)) = self.stencils.resolve(ed, key) else {
                self.stencil.carry = None;
                return;
            };
            // Ghost: the master thumbnail drawn at the shape's real
            // on-page size, semi-transparent, pinned at the cursor.
            let (w, h) = master_size(src, m.id);
            let size = Vec2::new((w as f32 * ppi).max(10.0), (h as f32 * ppi).max(10.0));
            let r = Rect::from_center_size(p, size);
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("stencil-ghost"));
            let painter = ctx.layer_painter(layer);
            // While the first-use thumbnail is still on a worker
            // (#92) the ghost carries just its outline.
            if let Some(tex) = self.stencil.texture(ctx, serial, src, m.id, &mut self.pool) {
                painter.image(
                    tex.id(),
                    r,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::from_white_alpha(150),
                );
            }
            let t = theme::shell_tokens(&ctx.style_of(ctx.theme()).visuals);
            painter.rect_stroke(
                r,
                1.0,
                Stroke::new(1.0, t.selection),
                egui::StrokeKind::Middle,
            );
        }
        if released {
            self.stencil.carry = None;
            if self.canvas_rect.contains(p) {
                let geom = self.page_geom();
                let (x, y) = canvas::screen_to_page(p, &self.view, &geom);
                self.place_master(key, (x, y));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// The full shell, testable headlessly (see tests/shell.rs).
    pub fn draw(&mut self, ui: &mut Ui) {
        let _frame = crate::perf::span(crate::perf::Stage::Frame);
        // #184: the per-frame keytip captures (badges, veil rects)
        // start empty; the ribbon layout below refills them.
        self.keytips.begin_frame();
        // Off-thread rasters (#92): land worker completions before
        // layout so this frame uses fresh textures; anything landing
        // (or a post deferred by the per-frame cap) schedules the
        // repaint that shows it. The UI thread only pays load_texture.
        let landed = self.pool.drain();
        if !landed.is_empty() || self.pool.take_deferred() {
            ui.ctx().request_repaint();
        }
        for done in landed {
            match done.key {
                crate::rasterpool::RasterKey::Page {
                    part,
                    version,
                    bucket,
                } => {
                    self.tex.on_page_done(
                        ui.ctx(),
                        part,
                        version,
                        bucket,
                        done.image,
                        done.tree,
                        done.was_parse,
                    );
                }
                crate::rasterpool::RasterKey::Thumb { serial, master } => {
                    self.stencil
                        .on_thumb_done(ui.ctx(), serial, master, done.image);
                }
                crate::rasterpool::RasterKey::Shape {
                    page,
                    id,
                    version,
                    bucket,
                } => {
                    if let Some(msg) = self.layers.on_shape_done(
                        ui.ctx(),
                        page,
                        id,
                        version,
                        bucket,
                        done.image,
                        // #124 item 6.3: the parsed tree rides along —
                        // bucket changes re-render it, no re-parse.
                        done.tree,
                    ) {
                        self.status = msg;
                    }
                }
            }
        }
        // Group-edit stack coherence (#78): drop open levels that no
        // longer resolve to a Group on the active page (ungroup, undo,
        // page switch, document change — one check covers them all).
        if !self.group_edit.is_empty() {
            let page_groups = |id: i64| -> bool {
                self.ed()
                    .and_then(|ed| ed.doc.pages.get(self.page_idx))
                    .map(|p| {
                        vsdx::model::find_shape_by_id(&p.shapes, id)
                            .is_some_and(|s| s.type_.as_deref() == Some("Group"))
                    })
                    .unwrap_or(false)
            };
            let ok = self
                .group_edit
                .iter()
                .copied()
                .take_while(|id| page_groups(*id))
                .count();
            self.group_edit.truncate(ok);
        }
        let ctx = ui.ctx().clone();
        // OS theme flips (GitHub #52): the watcher thread re-probes the
        // OS preference on its period; switch the active style slot when
        // it reports a change, and ask for a repaint on the same period
        // so an idle app still wakes to pick a flip up.
        if let Some(watcher) = &self.os_theme {
            if let Some(pref) = watcher.poll() {
                ctx.set_theme(pref);
            }
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        self.handle_shortcuts(&ctx);
        // #113: a copy queued OS-clipboard text (the copy path runs
        // inside input handling without a Context).
        if let Some(txt) = self.os_clipboard_text.take() {
            ctx.copy_text(txt);
        }
        self.poll_os_image();
        self.update_drag(&ctx);
        self.poll_dialog(&ctx);
        self.poll_stencil_extraction(&ctx);
        // Backstage Export worker (#35): the finished file count (or
        // the failure) lands in the pane's note and the status bar.
        if let Some(result) = self.export_pane.poll() {
            // The pane's own note carries the full message incl. any
            // density-cap notes; the status mirrors it.
            self.status = self
                .export_pane
                .note_clone()
                .unwrap_or_else(|| match result {
                    Ok(_) => crate::i18n::strs().msg_export_finished(),
                    Err(e) => crate::i18n::strs().msg_export_failed_e(e.to_string()),
                });
        }
        self.handle_close_request(&ctx);
        self.reorder_pages_window(&ctx);
        self.page_setup_window(&ctx);
        self.copy_picture_window(&ctx);
        self.container_gallery_window(&ctx);
        self.backgrounds_window(&ctx);
        self.select_by_type_window(&ctx);
        self.snap_glue_window(&ctx);
        self.hyperlink_window(&ctx);
        self.fields_insert_window(&ctx);
        self.spell_window(&ctx);
        self.comment_window(&ctx);
        self.shape_data_window(&ctx);
        self.size_pos_window(&ctx);
        self.color_palette_window(&ctx);

        // The eframe root Ui paints no background and eframe's clear
        // color is near-black, so the shell must own one: the panel
        // fill (kittest synthesizes exactly this behind the root Ui,
        // which is why the goldens never caught the black production
        // canvas) covers the start screen, the File backstage, and any
        // margin the panels leave. The canvas paints the desk over its
        // own area on top (GitHub #52).
        ui.painter().rect_filled(
            ui.max_rect(),
            0.0,
            theme::shell_tokens(ui.visuals()).ribbon_bg,
        );

        // #67: the OS window carries the title; no in-app title band.
        // Before the backstage return: a drawing replaced while the
        // File screen is open (Save & New) must retitle immediately,
        // not when the user leaves backstage (branch review).
        self.sync_window_title(ui.ctx());
        if self.backstage {
            self.backstage_view(ui);
            // #268: the Options dialog opens over the File screen and
            // must paint inside this branch (the backstage early-
            // returns), before the confirm modals so their layering
            // wins.
            self.options_window(&ctx);
            self.confirm_new_window(&ctx);
            self.confirm_close_window(&ctx);
            return;
        }

        // Shell order: top bands, bottom bands, side panels, canvas.
        let t = theme::shell_tokens(ui.visuals());
        self.tab_strip(ui);
        egui::Panel::top("ribbon_content")
            .frame(
                egui::Frame::new()
                    .fill(t.ribbon_content)
                    .inner_margin(egui::Margin::symmetric(4, 3)),
            )
            .show(ui, |ui| self.ribbon_content(ui));
        // Visio: a hairline rule closes the ribbon off from the canvas.
        egui::Panel::top("ribbon_rule")
            .exact_size(1.0)
            .frame(egui::Frame::new().fill(t.ribbon_edge))
            .show(ui, |_| {});
        // #184: the ribbon panels have laid out — paint the armed
        // keytip veil + badges above them (no-op while disarmed).
        self.paint_keytips(&ctx);
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(t.status_bg)
                    .inner_margin(egui::Margin::symmetric(10, 3)),
            )
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("page_tabs")
            .frame(
                egui::Frame::new()
                    .fill(t.tabs_bg)
                    .inner_margin(egui::Margin::same(2)),
            )
            .show(ui, |ui| self.page_tabs(ui));
        // The pane lists the document's masters plus every open
        // stencil (#63): it stays up once a stencil is open, even for
        // a master-less document — but it needs a document to render
        // against (the pane body draws nothing without an editor).
        let pane_wanted = self
            .ed()
            .map(|ed| !ed.doc.masters.is_empty() || !self.stencils.is_empty())
            .unwrap_or(false);
        if pane_wanted {
            // #257 item 3: RTL mirrors the pane sides — Shapes on the
            // right, Format on the left (the observed Office RTL
            // behavior; the stable ids keep user sizing).
            let stencil_panel = if crate::i18n::rtl() {
                egui::Panel::right("stencil")
            } else {
                egui::Panel::left("stencil")
            };
            stencil_panel
                .frame(
                    egui::Frame::new()
                        .fill(t.stencil_bg)
                        .inner_margin(egui::Margin::same(6)),
                )
                .default_size(160.0)
                .resizable(true)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        // The pane draws its own Visio header ("Shapes"
                        // + More Shapes), search box, stencil list and
                        // master grid (crates/dc-ui/src/stencil.rs).
                        self.stencil_panel_inner = ui.max_rect();
                        let mut armed = None;
                        let mut request = None;
                        let title = stencil_title(&self.path);
                        // #266: the My Shapes menu scan is gated on the
                        // same disk flag the headless ctors pin false —
                        // kittest never reads the user's real folder.
                        let my_shapes = self.stencil_disk.then(Self::my_shapes_dir).flatten();
                        {
                            let App {
                                ed,
                                stencil,
                                stencils,
                                pool,
                                quick_shapes,
                                ..
                            } = self;
                            if let Some(ed) = ed.as_ref() {
                                if let Some(id) = stencil.show(
                                    ui,
                                    ed,
                                    stencils,
                                    &title,
                                    pool,
                                    quick_shapes,
                                    my_shapes.as_deref(),
                                ) {
                                    armed = Some(id);
                                }
                                request = stencil.request.take();
                            }
                        }
                        // Pane requests (#83/#87): the pane can't reach
                        // the dialog worker or the catalog's byte
                        // loader itself.
                        match request {
                            Some(crate::stencil::Request::OpenStencilDialog) => {
                                self.begin_dialog(PendingKind::OpenStencil);
                            }
                            Some(crate::stencil::Request::ToggleBuiltin(slug)) => {
                                self.toggle_builtin(slug);
                            }
                            Some(crate::stencil::Request::MyShapes(path)) => {
                                self.toggle_my_shapes_stencil(path);
                            }
                            Some(crate::stencil::Request::QuickShapesMenu(key)) => {
                                let anchor = ui
                                    .input(|i| i.pointer.interact_pos())
                                    .unwrap_or(egui::Pos2::ZERO);
                                self.quick_menu = Some((key, anchor));
                            }
                            None => {}
                        }
                        if let Some(id) = armed {
                            self.status = crate::i18n::strs().msg_master_armed_click_place(id.1);
                        }
                        if self.stencil.armed.is_some()
                            && ui
                                .button(crate::i18n::strs().msg_x_cancel_placement())
                                .clicked()
                        {
                            self.stencil.armed = None;
                        }
                    });
                });
        }
        if self.format_pane_open && self.sel.is_some() {
            if crate::i18n::rtl() {
                egui::Panel::left("format_pane")
            } else {
                egui::Panel::right("format_pane")
            }
            .default_size(240.0)
            .resizable(true)
            .show(ui, |ui| self.format_pane(ui));
        }
        // #192 MDI: `central` hosts the drawing as a CHILD WINDOW
        // inside the client area (its own ui — the region the docked
        // panels left), and publishes that rect. SPEC: with exactly
        // ONE child window there are NO decorators and it fills the
        // client ("treated as it is now"); once more than one child
        // exists, every child gets its decorators and the children
        // AUTOMATICALLY ARRANGE (equal tiles) to fit — any change of
        // the child count re-fires the arrangement.
        self.central(ui);
        let drawing_open = self.ed.is_some();
        let n_children = drawing_open as usize + self.shapesheets.len();
        if n_children != self.mdi_last_count {
            self.mdi_last_count = n_children;
            self.mdi_arranging = true;
        }
        // The per-child tiles: an equal-width column each (the
        // drawing first/left, sheets after), inside the client.
        let client_ok = self.mdi_client.is_finite()
            && self.mdi_client.width() > 24.0
            && self.mdi_client.height() > 24.0;
        let tiles: Vec<egui::Rect> = if n_children == 0 || !client_ok {
            Vec::new()
        } else {
            let c = self.mdi_client;
            let gutter = 8.0;
            let n = n_children as f32;
            let w = (c.width() - gutter * 2.0 - gutter * (n - 1.0)) / n;
            (0..n_children)
                .map(|i| {
                    egui::Rect::from_min_size(
                        egui::pos2(
                            c.left() + gutter + i as f32 * (w + gutter),
                            c.top() + gutter,
                        ),
                        egui::vec2(w, c.height() - 2.0 * gutter),
                    )
                })
                .collect()
        };
        let decorated = n_children >= 2;
        // Drawing window: created inside `central` (it needs the
        // client ui), but its decorators/arrangement are decided
        // here — central reads these fields the frame before... no:
        // central runs first and builds the window, so the flags it
        // reads were set by THIS block last frame. One frame of lag
        // on transitions is invisible (the count itself is stable).
        self.mdi_decorated = decorated;
        self.mdi_drawing_tile = if n_children == 1 && drawing_open {
            // Single child: pinned full-bleed, decorator-free.
            self.mdi_client
        } else {
            tiles.first().copied().unwrap_or(self.mdi_client)
        };
        // #190/#192: the ShapeSheet as a floating child window — the
        // native title bar carries the close. Painted after the
        // drawing window so it stacks above; constrained to the
        // client area so children overlap only each other, never the
        // ribbon/status/docked panes.
        if !self.shapesheets.is_empty() && client_ok {
            // One window per open sheet (#192 spec), in open order;
            // the tiles follow the drawing (first) then the sheets.
            let mut close_ix: Option<usize> = None;
            for (i, &target) in self.shapesheets.clone().iter().enumerate() {
                let mut open = true;
                let sheet_tile = if n_children == 1 {
                    // Sheets-only child (no drawing open): full-bleed,
                    // decorator-free per the spec.
                    self.mdi_client
                } else {
                    tiles
                        .get(drawing_open as usize + i)
                        .copied()
                        .unwrap_or(self.mdi_client)
                };
                let name = match target {
                    crate::shapesheet::SheetTarget::Shape(sid) => self
                        .ed()
                        .and_then(|ed| ed.doc.pages.get(self.page_idx))
                        .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, sid))
                        .and_then(|sh| sh.name_u.clone())
                        .unwrap_or_else(|| crate::i18n::strs().msg_shape_sid(sid)),
                    // #272: a page sheet carries its page's name.
                    crate::shapesheet::SheetTarget::Page(page_id) => self
                        .ed()
                        .and_then(|ed| ed.doc.pages.iter().find(|p| p.id == page_id))
                        .map(|p| p.name.clone())
                        .unwrap_or_default(),
                    crate::shapesheet::SheetTarget::Document => {
                        crate::i18n::strs().msg_document_sheet_name()
                    }
                };
                let title = crate::i18n::strs().msg_shapesheet_name(name);
                let mut win = egui::Window::new(title)
                    .id(egui::Id::unique(("dc-shapesheet", target)))
                    .open(&mut open)
                    .title_bar(n_children >= 2 || !drawing_open)
                    .resizable(true)
                    .collapsible(false)
                    .default_size([320.0, 460.0]);
                if self.mdi_arranging || n_children == 1 {
                    win = win
                        .current_pos(sheet_tile.left_top())
                        .fixed_size(sheet_tile.size());
                }
                if self.mdi_client.is_finite() && self.mdi_client.width() > 0.0 {
                    win = win.constrain_to(self.mdi_client);
                }
                win.show(ui.ctx(), |ui| {
                    // #189: the body caps its content to the tile so
                    // the window never outgrows its slot and drifts
                    // over a neighboring child's title bar.
                    self.shapesheet_panel(ui, target, sheet_tile.width() - 14.0);
                });
                if !open {
                    close_ix = Some(i);
                }
            }
            if let Some(i) = close_ix {
                self.shapesheets.remove(i);
            }
        }
        self.stencil_carry(&ctx);

        // Automated visual smoke: DC_UI_SCREENSHOT=path.png captures the
        // 30th frame and exits (headless CI / review pipeline).
        if let Some(path) = std::env::var_os("DC_UI_SCREENSHOT") {
            self.shot_frames += 1;
            if self.shot_frames == 30 {
                let path = std::path::PathBuf::from(path);
                ctx.request_screenshot(move |img| {
                    let _ = write_png(&path, &img);
                    std::process::exit(0);
                });
            }
        }
        self.confirm_new_window(&ctx);
        self.confirm_close_window(&ctx);

        // #224 overlays, painted above everything: the modal loading
        // scrim while a document parse is in flight (the poll that
        // lands the finished editor), then the branded splash. Fixed
        // light theme — the kit's light set. Time-driven repaints keep
        // both animating.
        let mut scrim = None;
        if self.loading.is_some() {
            let in_flight = self.poll_loading();
            if let Some((path, started)) = in_flight {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                scrim = Some((crate::i18n::strs().msg_opening_file(name), started));
            }
        }
        if scrim.is_none() {
            if let Some((path, started)) = &self.rendering {
                // Reveal only on the finished render: the page's
                // first render fully drained (composite ran, nothing
                // budget-deferred, every on-screen shape's texture
                // landed) AND the scrim showed its minimum window.
                let drained = self.layers.render_drained()
                    && started.elapsed().as_secs_f32() >= crate::loading::LOADING_MIN_SECS;
                if drained {
                    // The same routed message the sync landing uses
                    // (PR #269 review: this bare literal was the one
                    // unrouted status on the open path).
                    self.status = crate::i18n::strs().msg_opened_path(path.display().to_string());
                    self.rendering = None;
                } else {
                    scrim = Some((crate::i18n::strs().msg_rendering_scrim(), *started));
                }
            }
        }
        if let Some((caption, started)) = scrim {
            let screen = ui.input(|i| i.viewport_rect());
            crate::loading::draw_loading(ui, screen, &caption, started);
        }
        if self.test_click {
            // The injected click (test hook): the splash skips exactly
            // as a real click would.
            self.test_click = false;
            self.splash = None;
        }
        if let Some(started) = self.splash {
            let screen = ui.input(|i| i.viewport_rect());
            if !crate::loading::draw_splash(ui, screen, started) {
                self.splash = None;
            }
        }

        if self.about_open {
            // Esc closes the frontmost About (the #110 dialog
            // keyboard audit's rule — About slipped it; #218 review).
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.about_open = false;
                return;
            }
            let mut open = self.about_open;
            egui::Window::new(crate::i18n::strs().msg_about_tarahi())
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(220.0, 150.0), egui::Sense::hover());
                    crate::loading::draw_mark(
                        ui.painter(),
                        rect.center() + egui::Vec2::new(0.0, -14.0),
                        62.0,
                        crate::loading::SPLASH_SECS, // settled
                    );
                    ui.painter().text(
                        rect.center() + egui::Vec2::new(0.0, 48.0),
                        egui::Align2::CENTER_CENTER,
                        crate::i18n::strs().msg_splash_wordmark(),
                        egui::FontId::proportional(26.0),
                        egui::Color32::from_rgb(0x1A, 0x24, 0x44),
                    );
                    // Inside the WINDOW — the outer ui paints on the
                    // shell behind it (#218 review).
                    ui.label(
                        egui::RichText::new(
                            crate::i18n::strs()
                                .msg_tarahi_v_cargo_pkg_version_clean(env!("CARGO_PKG_VERSION")),
                        )
                        .size(12.0),
                    );
                });
            self.about_open = open && self.about_open;
        }
    }
}
