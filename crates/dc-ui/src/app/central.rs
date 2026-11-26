//! The central layout: canvas, stencil pane, and their composition.

use super::*;

impl App {
    /// #192 MDI: with a document open, the drawing is a titled child
    /// window (document name + page, movable/resizable, no close —
    /// documents close through File) constrained to the client area
    /// this ui already is. The id is pinned: the TITLE changes with
    /// page/dirty flips, and an id-from-title window would lose its
    /// placement on every one of them. Without a document the start
    /// screen fills the frame unwrapped.
    pub(super) fn central(&mut self, ui: &mut Ui) {
        // The client area is what the docked panels LEFT — the
        // available rect, not max_rect (the ui's full allocation):
        // egui's inside-panels shrink the former, and a window
        // constrained to max_rect would float over the stencil pane.
        // An 8 px gutter insets the child windows inside it: the
        // workspace reads as an MDI client region, and the windows'
        // resize-border sense zones never cover a docked panel's own
        // resize handle at the shared edge.
        let avail = ui.available_rect_before_wrap();
        self.mdi_client = if avail.is_finite() && avail.width() > 24.0 && avail.height() > 24.0 {
            avail.shrink(8.0)
        } else {
            avail
        };
        if self.ed.is_none() {
            self.central_inner(ui);
            return;
        }
        let doc_name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
        let page_name = self
            .ed()
            .and_then(|e| e.doc.pages.get(self.page_idx))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let mut title = format!("{doc_name} — {page_name}");
        if self.dirty {
            title.push_str(" •");
        }
        let client = self.mdi_drawing_tile;
        // #192 spec: the drawing's title-bar close (decorated mode
        // only — the single undecorated child has no X) runs the
        // SAME flow as File > Close: dirty asks first, clean closes.
        let mut doc_close = false;
        let mut win = egui::Window::new(title)
            .id(egui::Id::unique("dc-drawing-window"))
            .title_bar(self.mdi_decorated)
            .resizable(true)
            .collapsible(false)
            .min_size(egui::vec2(240.0, 180.0));
        let mut open = true;
        if self.mdi_decorated {
            win = win.open(&mut open);
        }
        if (!self.mdi_decorated || self.mdi_arranging) && client.is_finite() && client.width() > 0.0
        {
            // Undecorated single child: pinned full-bleed; arranging:
            // pinned to the tile for the transition frame(s). (Frame 0
            // carries no tile yet — the window seeds from its
            // defaults and the pin takes over the frame after.)
            win = win.current_pos(client.left_top()).fixed_size(client.size());
        }
        if self.mdi_client.is_finite() && self.mdi_client.width() > 0.0 {
            win = win.constrain_to(self.mdi_client);
        }
        win.show(ui.ctx(), |ui| self.central_inner(ui));
        if !open {
            doc_close = true;
        }
        if doc_close {
            // Identical to BackAction::CloseDoc: a dirty document
            // asks (the modal), a clean one closes now.
            if self.dirty {
                self.confirm_close = Some(CloseTarget::Doc);
            } else {
                self.close_doc();
            }
        }
        // The arrangement pin is consumed HERE (the drawing window
        // builds in this fn, one frame after `draw` computed it on
        // the count change); the sheets consumed it in `draw`.
        self.mdi_arranging = false;
    }

    pub(super) fn central_inner(&mut self, ui: &mut Ui) {
        if self.ed.is_none() {
            // Visio 2016 start screen shape: Recent on the left,
            // template choices on the right.
            ui.horizontal_centered(|ui| {
                ui.add_space(40.0);
                ui.vertical(|ui| {
                    ui.add_space(24.0);
                    ui.heading(crate::i18n::strs().msg_recent());
                    ui.add_space(6.0);
                    if let Some(p) = self.recent_list_ui(ui) {
                        self.load(p);
                    }
                });
                // #257 item 3: the New column hugs the mirrored far
                // end of the start screen.
                let start_new_dir = if crate::i18n::rtl() {
                    egui::Layout::left_to_right(egui::Align::TOP)
                } else {
                    egui::Layout::right_to_left(egui::Align::TOP)
                };
                ui.with_layout(start_new_dir, |ui| {
                    ui.vertical(|ui| {
                        ui.add_space(24.0);
                        ui.heading(crate::i18n::strs().msg_rail_new());
                        ui.add_space(6.0);
                        if ui
                            .button(crate::i18n::strs().msg_blank_drawing())
                            .on_hover_text(
                                crate::i18n::strs().msg_start_an_empty_us_letter_drawing(),
                            )
                            .clicked()
                        {
                            self.do_new();
                        }
                        if ui
                            .button(crate::i18n::strs().msg_open_a_visio_file())
                            .on_hover_text(crate::i18n::strs().msg_open_vsdx_vstx_vssx_ctrl_o())
                            .clicked()
                        {
                            self.do_open();
                        }
                    });
                });
            });
            return;
        }
        let npages = self.ed.as_ref().unwrap().doc.pages.len();
        self.page_idx = self.page_idx.min(npages.saturating_sub(1));
        // Annotation visibility (#29): hide the Annotation layer's
        // members when the Review toggle is off.
        let mut hidden: Vec<i64> = Vec::new();
        if !self.show_comments {
            if let Some(ed) = self.ed.as_ref() {
                if let Some(page) = ed.doc.pages.get(self.page_idx) {
                    if let Ok(rows) = dc_core::layers::layer_rows(&ed.doc, page.id) {
                        for r in &rows {
                            if dc_core::layers::layer_name(r) == Some("Annotation") {
                                hidden.push(r.ix);
                            }
                        }
                    }
                }
            }
        }
        // Group-edit breadcrumb bar (#78): "Page ▸ <Group> ▸ …" with a
        // Close Group control — Visio shows the path in the group
        // window's title; v1 pins a slim bar above the canvas. Esc
        // pops one level (the same close semantics as Visio's window).
        if !self.group_edit.is_empty() {
            let names: Vec<String> = self
                .group_edit
                .iter()
                .map(|gid| {
                    self.ed()
                        .and_then(|ed| ed.doc.pages.get(self.page_idx))
                        .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, *gid))
                        .and_then(|s| s.name_u.clone())
                        .unwrap_or_else(|| format!("Group.{gid}"))
                })
                .collect();
            let mut pop = false;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(
                        crate::i18n::strs().msg_group_breadcrumb(names.join(" › ")),
                    )
                    .small()
                    .color(ui.visuals().weak_text_color()),
                );
                if ui
                    .small_button(crate::i18n::strs().msg_close_group())
                    .clicked()
                {
                    pop = true;
                }
            });
            if pop {
                self.group_edit.pop();
            }
            // Esc pops one level (after the text editor and any open
            // menu had their chance — those consume the key first).
            if self.drag.is_none()
                && self.text_edit.is_none()
                && self.ctx_menu.is_none()
                && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
            {
                self.group_edit.pop();
            }
        }
        let geom = self.page_geom();
        let sel_rect = self.sel_page_rect();
        let extra_rects: Vec<Rect> = self.sel_extra_page_rects();
        // #156: the selected container's member rects — the highlight
        // overlay (default on; the Container Tools toggle writes the
        // container's msvSDContainerNoHighlight row).
        let member_hl: Vec<Rect> = if self.sel_is_container() && self.container_highlight_on() {
            let (page_id, sid) = self.sel.unwrap();
            self.ed()
                .and_then(|ed| ed.doc.pages.iter().find(|p| p.id == page_id))
                .map(|page| {
                    self.container_member_ids(page_id, sid)
                        .iter()
                        .filter_map(|m| {
                            let doc = &self.ed().unwrap().doc;
                            vsdx::model::find_shape_by_id(&page.shapes, *m)
                                .and_then(|sh| self.shape_rect(doc, page_id, sh))
                        })
                        .map(|(x, y, w, h)| {
                            // page y-up → the canvas's y-down rect frame
                            let geom = self.page_geom();
                            Rect::from_min_max(
                                egui::pos2(x as f32, (geom.h_in - y - h) as f32),
                                egui::pos2((x + w) as f32, (geom.h_in - y) as f32),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // The page's guides (#113): collected before the borrows below
        // (the marquee-gating hit closure and the overlay both use
        // them).
        let page_guides = self.page_guides();
        let guide_fx: Vec<canvas::GuideFx> = page_guides
            .iter()
            .map(|g| canvas::GuideFx {
                id: g.id,
                vertical: match g.kind {
                    crate::guides::GuideKind::Vertical => Some(true),
                    crate::guides::GuideKind::Horizontal => Some(false),
                    crate::guides::GuideKind::Point => None,
                },
                x: g.x,
                y: g.y,
                selected: self.sel.is_some_and(|(_, sid)| sid == g.id),
            })
            .collect();
        let guide_hit_threshold = 5.0 / self.view.px_per_in() as f64;
        // Live move feedback (#96): during a moved drag the dragged
        // shapes' static layers are suppressed and ghosted at the live
        // position — Visio moves the shape under the cursor for the
        // whole gesture, and a move ghost is a quad translation, never
        // a re-raster (the #93 O(0)-move guarantee holds).
        let mut drag_fx = canvas::DragFx::default();
        let mut hide_layers: Vec<i64> = Vec::new();
        // #226: an endpoint-handle drag on the selected 1D shape hides
        // its static layer quad — the styled live ghost (built after
        // the snap closures below) replaces it for the gesture. The
        // ghost plans through [`crate::cmd::plan_connector_frame`] for
        // BOTH connectors and plain 1D lines: the planner's line_frame
        // branch and `apply_line_endpoint`'s angle model write
        // different cell sets, but both draw the same segment ink the
        // release lands, so the preview matches the drop (#185
        // review — verified; the cells diverge, the visible ink does
        // not).
        let armed_handle_now = canvas::armed_handle(ui.ctx());
        let endpoint_drag_live = matches!(
            armed_handle_now,
            Some(HandleId::LineBegin) | Some(HandleId::LineEnd)
        ) && self.sel.is_some_and(|(p, _)| Some(p) == self.page_id());
        if endpoint_drag_live {
            if let Some((_, sid)) = self.sel {
                hide_layers.push(sid);
            }
        } else {
            self.endpoint_preview = None;
        }
        let mut live_drag = false;
        if let Some(d) = self.drag.as_ref().filter(|d| d.moved) {
            if self.sel.is_some_and(|(p, _)| Some(p) == self.page_id()) {
                live_drag = true;
                // #109: the dynamic-grid hints computed by this frame's
                // `apply_drag_snap` (update_drag runs before the paint).
                drag_fx.hints = self.snap_live.clone();
                // y-up pin delta → y-down bbox delta flips y.
                let (dx, dy) = (d.last_pin.0 - d.start_pin.0, d.last_pin.1 - d.start_pin.1);
                let mut ids: Vec<i64> = vec![d.shape];
                ids.extend(d.group.iter().copied());
                let ed = self.ed.as_ref().unwrap();
                let page = &ed.doc.pages[self.page_idx];
                for id in ids {
                    hide_layers.push(id);
                    match self.layers.quad_for(id) {
                        Some(q) => {
                            let bbox = (q.bbox.0 + dx, q.bbox.1 - dy, q.bbox.2, q.bbox.3);
                            drag_fx.ghosts.push(crate::layers::ShapeQuad {
                                id,
                                tex: q.tex,
                                bbox,
                            });
                            // The primary carries its selection frame
                            // along (Visio keeps the outline on the
                            // moving shape).
                            if id == d.shape {
                                let tl = canvas::page_to_screen(
                                    bbox.0,
                                    geom.h_in - bbox.1,
                                    &self.view,
                                    &geom,
                                );
                                let br = canvas::page_to_screen(
                                    bbox.0 + bbox.2,
                                    geom.h_in - (bbox.1 + bbox.3),
                                    &self.view,
                                    &geom,
                                );
                                drag_fx.outlines.push(Rect::from_two_pos(tl, br));
                            }
                        }
                        None => {
                            // Texture still rasterizing (or a sub-shape
                            // selection): a dashed outline at the live
                            // position keeps the gesture visible.
                            if let Some(shape) = vsdx::model::find_shape_by_id(&page.shapes, id) {
                                // Nested-aware: a group-edit member
                                // maps its parent-local rect through
                                // the XForm chain to page space.
                                if let Some((x, y, w, h)) = self.shape_rect(&ed.doc, page.id, shape)
                                {
                                    let tl = canvas::page_to_screen(
                                        x + dx,
                                        y + h + dy,
                                        &self.view,
                                        &geom,
                                    );
                                    let br = canvas::page_to_screen(
                                        x + w + dx,
                                        y + dy,
                                        &self.view,
                                        &geom,
                                    );
                                    drag_fx.outlines.push(Rect::from_two_pos(tl, br));
                                }
                            }
                        }
                    }
                }
            }
        }
        self.last_drag_ghosts = drag_fx.ghosts.len();
        // The moved selection frame follows the gesture; static chrome
        // (frame, members, handles) hides while it moves (Visio look).
        let (sel_rect, extra_rects, handles) = if live_drag {
            (None, Vec::new(), Vec::new())
        } else {
            (sel_rect, extra_rects, self.selection_handles())
        };
        let armed = self.stencil.armed.is_some();
        // #90/#91/#92: the page SVG is built by the cache's miss
        // closure — only when the (part, version, ppi-bucket) texture
        // is actually missing — and the parse/render runs on the raster
        // pool. Until a first raster lands, `None` paints the page as
        // its natural white paper (canvas::show's None path); a render
        // failure surfaces as the error label (and is retried next
        // frame, since failures are not cached).
        // #93: per-shape layers replace the one-page texture. Each
        // top-level shape subtree is its own layer (same SVG backend
        // walk, viewBox scoped to its ink bounds), rasterized on the
        // #92 pool and composited in z-order; only the shapes a
        // command touched ever re-render. Group-edit mode (#78)
        // composites only the open group's layer — its subtree at page
        // positions through the normal group transforms, the same
        // content the scoped monolith drew. `render_page_with` stays
        // the export/print path (and the Info thumbnail's).
        let (mut quads, layer_status) = {
            let ed = self.ed.as_ref().unwrap();
            let page = &ed.doc.pages[self.page_idx];
            // Viewport (visible page rect, y-down inches) from the
            // last frame's canvas rect — whole page until it exists.
            let vp = if self.canvas_rect.is_positive() {
                let (x0, y0) =
                    canvas::screen_to_page(self.canvas_rect.left_top(), &self.view, &geom);
                let (x1, y1) =
                    canvas::screen_to_page(self.canvas_rect.right_bottom(), &self.view, &geom);
                let y0 = geom.h_in - y0; // → y-down viewBox space
                let y1 = geom.h_in - y1;
                (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
            } else {
                (0.0, 0.0, geom.w_in, geom.h_in)
            };
            // The compositing scope is the TOP-LEVEL group on the
            // group-edit stack: composite iterates page.shapes (top
            // level), so the innermost id of a nested stack ([G1, G2])
            // matches nothing and blanked the canvas. Hit-testing
            // stays scoped to the innermost group (shape_hit).
            self.layers.composite(
                ui.ctx(),
                &mut self.pool,
                &ed.doc,
                page,
                self.view.px_per_in(),
                vp,
                self.group_edit.first().copied(),
                &hidden,
                // While the in-place editor is open (#22) its shape's
                // layer renders without the text — the editor overlay
                // is the only copy on screen.
                self.text_edit.as_ref().map(|s| s.shape),
            )
        };
        if let Some(msg) = layer_status {
            self.status = msg;
        }
        quads.retain(|q| !hide_layers.contains(&q.id));
        // Background pages render beneath this page's ink (#113) —
        // built before the field borrows below.
        let underlays = self.build_bg_underlays(ui.ctx());
        // #108: line-jump hops for the frame's connector crossings.
        // The page's Show-Line-Jumps switch gates the overlay (#251);
        // the hop radius is the page's own (#294 — LineToLineX ×
        // LineJumpFactorX / 2, style-resolved).
        let jump_radius = {
            let doc = &self.ed.as_ref().unwrap().doc;
            let page = &doc.pages[self.page_idx];
            if page.line_jumps_shown() {
                crate::connectors::jump_radii_in(doc, page)
            } else {
                (0.0, 0.0)
            }
        };
        let jump_fx: Vec<canvas::JumpFx> = if jump_radius.0 > 0.0 || jump_radius.1 > 0.0 {
            let doc = &self.ed.as_ref().unwrap().doc;
            let page = &doc.pages[self.page_idx];
            // THE crossing set (#336): the canvas draws from the same
            // `page_jumps` computation as the export and the
            // reconciliation — the page's own jump rules
            // (`LineJumpCode`, `ConLineJumpCode`), the straight-run
            // eligibility, the hidden-layer skips — never a
            // canvas-private re-crossing that could drift.
            dc::geom::page_jumps(doc, page, &hidden)
                .into_iter()
                .map(|j| {
                    // The gap paints the BACKDROP — the fill of the
                    // topmost filled shape beneath the connector at
                    // the hop, else page white: the canvas cannot cut
                    // cached rasters, so this mimics the export's cut
                    // in the color that shows through it (#324
                    // phase 3).
                    let (gr, gg, gb) =
                        dc::svg::backdrop_color_at(doc, page, j.at, j.jumper, &hidden);
                    canvas::JumpFx {
                        gap_color: Color32::from_rgb(gr, gg, gb),
                        at: j.at,
                        dir: j.dir,
                        under: j.under,
                        weight: j.weight,
                        under_weight: j.under_weight,
                        color: Color32::from_rgb(j.color.0, j.color.1, j.color.2),
                        under_color: Color32::from_rgb(
                            j.under_color.0,
                            j.under_color.1,
                            j.under_color.2,
                        ),
                        // The hop sizes with the radius of the jumping
                        // segment's own axis (the svg overlay's rule —
                        // PR #299 review).
                        radius_in: (if j.dir.0.abs() >= j.dir.1.abs() {
                            jump_radius.0
                        } else {
                            jump_radius.1
                        }) as f32,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        self.jumps_last = jump_fx.len();
        // #177: everything the connection-point overlay needs, built
        // before the destructuring borrows below (the closure runs
        // inside `canvas::show`, while `view` is borrowed).
        let conn_cands = self.glue_candidates(&[]);
        let conn_threshold = SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64;
        let conn_snap = self.snap;
        let conn_sel = self.sel;
        // #108: the Connection Point tool's state, precomputed (the
        // fx closure below cannot borrow self).
        let conn_tool = self.tool == Tool::ConnectionPoint;
        let connpt_sel_pos = self.connpt_sel.and_then(|(sid, ix)| {
            self.glue_candidates(&[])
                .into_iter()
                .find(|c| c.id == sid)?
                .conn_rows
                .into_iter()
                .find(|(rix, _)| *rix == ix)
                .map(|(_, p)| p)
        });
        // #179: live endpoint snapping — two target sets: one
        // excluding the selected connector (its own endpoint handle
        // is the dragged thing) and one excluding nothing (the
        // Connector Tool's fresh draw). The context is built inside
        // the closure (SnapCtx borrows the targets).
        let snap_settings = self.snap;
        // #202: connector-endpoint drags magnetize to CONNECTION
        // POINTS (+ guides) only — the shape-handle/vertex magnets
        // snapped drops onto the old shape's edges and re-glued them
        // there (the hard-glued / one-axis-only feel).
        let sel_is_conn = self.sel.is_some_and(|(p, sid)| {
            p == self.page_id().unwrap_or(0)
                && self.ed.as_ref().is_some_and(|ed| {
                    let page = &ed.doc.pages[self.page_idx];
                    vsdx::model::find_shape_by_id(&page.shapes, sid).is_some_and(|sh| {
                        shape_chain(&ed.doc, page.id, sid)
                            .ok()
                            .is_some_and(|ch| crate::connectors::is_connector_in(&ed.doc, &ch, sh))
                    })
                })
        });
        let (snap_grid, snap_ruler, snap_thr, snap_mirror) = (
            canvas::grid_step_in(&self.view),
            canvas::ruler_interval_in(&self.view),
            SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64,
            // The RTL ruler's zero at the page's right edge (see
            // SnapCtx::ruler_mirror_x_in).
            crate::i18n::rtl().then(|| self.page_geom().w_in),
        );
        let (t_sel, g_sel, t_all, g_all) = {
            let pid = self.page_id().unwrap_or(-1);
            let sel_id = conn_sel.map(|(_, s)| s).unwrap_or(-1);
            (
                self.snap_targets(pid, &[sel_id]),
                self.snap_guides(&[sel_id]),
                self.snap_targets(pid, &[-1]),
                self.snap_guides(&[-1]),
            )
        };
        // Both closures below share t_sel/g_sel, so they capture
        // references, not the vectors.
        let (t_sel_r, g_sel_r) = (&t_sel, &g_sel);
        let snap_end = move |endpoint_drag: bool, p: (f64, f64)| -> (f64, f64) {
            let (targets, guides) = if endpoint_drag {
                (t_sel_r, g_sel_r)
            } else {
                (&t_all, &g_all)
            };
            let ctx = crate::snap::SnapCtx {
                targets,
                guides,
                grid_step_in: snap_grid,
                ruler_step_in: snap_ruler,
                ruler_mirror_x_in: snap_mirror,
                threshold_in: snap_thr,
            };
            let mut settings = snap_settings;
            if endpoint_drag && sel_is_conn {
                settings.glue_shape_handles = false;
                settings.glue_shape_vertices = false;
            }
            crate::snap::point_snap(p, &ctx, &settings, true).map_or(p, |(q, _)| q)
        };
        // #160: the live corner-resize preview snaps per axis under
        // the SNAP settings against the page minus the selected shape
        // (t_sel/g_sel exclude it) — the commit path runs the
        // identical snap on the release pointer.
        let snap_corner = move |p: (f64, f64)| -> Option<(f64, f64)> {
            let ctx = crate::snap::SnapCtx {
                targets: t_sel_r,
                guides: g_sel_r,
                grid_step_in: snap_grid,
                ruler_step_in: snap_ruler,
                ruler_mirror_x_in: snap_mirror,
                threshold_in: snap_thr,
            };
            crate::snap::corner_snap(p, &ctx, &snap_settings)
        };
        let conn_fx_seen = std::cell::RefCell::new(crate::connectors::ConnPointFx::default());
        // #226: the live endpoint-drag ghost — the connector's styled
        // ink re-rendered through the SHARED planner at the snapped
        // pointer; the bare live line stands down while it paints.
        let mut endpoint_ghost = false;
        if endpoint_drag_live {
            if let (Some((page_id, sid)), Some(hid)) = (self.sel, armed_handle_now) {
                if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                    let ptr_page = canvas::screen_to_page(ptr, &self.view, &geom);
                    if let Some(q) =
                        self.endpoint_preview_quad(ui.ctx(), page_id, sid, hid, &snap_end, ptr_page)
                    {
                        drag_fx.ghosts.push(q);
                        endpoint_ghost = true;
                    }
                }
            }
        }
        drag_fx.endpoint_ghost = endpoint_ghost;
        self.last_endpoint_ghost = endpoint_ghost;
        let mut mouse = self.mouse_page;
        // #202: computed before the destructuring self-borrow the
        // canvas call holds.
        let sel_line_curved = self.sel_1d_curved();
        // #237: a pending zoom-to-selection centers + fills the
        // selection's union bounds on this frame — the same view ledger
        // the Fit Page and #22 text-edit zoom paths live on. Runs
        // before the destructuring self-borrow and the canvas frame so
        // the wheel anchor and the paint agree.
        if self.zoom_selection_pending {
            self.zoom_selection_pending = false;
            if let Some(rect) = self.sel_union_page_rect() {
                let (origin, content) =
                    canvas::content_area(self.canvas_rect, self.view.show_rulers);
                self.view
                    .zoom_to_selection(rect, content, origin.to_vec2(), &geom);
            }
        }
        let App {
            view, ed, page_idx, ..
        } = self;
        let ed = ed.as_ref();
        let page = ed.map(|e| &e.doc.pages[(*page_idx).min(e.doc.pages.len().saturating_sub(1))]);
        let hit = |pos: (f64, f64)| -> bool {
            let Some(ed) = ed else { return false };
            let Some(page) = page else { return false };
            // Guides count as occupied page (#113): a press on one
            // arms its move-drag, never a marquee.
            if crate::guides::guide_hit(&page_guides, pos, guide_hit_threshold).is_some() {
                return true;
            }
            for shape in page.shapes.iter().rev() {
                if let Some((x, y, w, h)) = Self::shape_rect_static(&ed.doc, page.id, shape) {
                    if hit_in_rect(pos, x, y, w, h) {
                        return true;
                    }
                }
            }
            false
        };
        // #177: connection-point affordances for the frame — the
        // closure paints from inside `canvas::show` and its result is
        // kept as the test seam.
        let conn_fx = |live: Option<(f64, f64)>| -> crate::connectors::ConnPointFx {
            let mut fx = crate::connectors::ConnPointFx::default();
            if let (Some(ed), Some(pg)) = (ed, page) {
                if let Some((_, cid)) = conn_sel.filter(|(p, _)| *p == pg.id) {
                    fx.glued = crate::connectors::glued_sites(&ed.doc, pg, cid, &conn_cands);
                }
            }
            if let Some(p) = live {
                let g = crate::connectors::conn_point_fx(
                    &conn_cands,
                    p,
                    p,
                    conn_threshold,
                    CONN_REVEAL_IN,
                    conn_snap.glue_active,
                    conn_snap.glue_connection_points,
                );
                fx.reveal = g.reveal;
                fx.glue_target = g.glue_target;
            }
            // #108: the Connection Point tool's selection ring — the
            // selected row's page position.
            if conn_tool {
                if let Some(q) = connpt_sel_pos {
                    fx.tool_selected = Some(q);
                }
            }
            *conn_fx_seen.borrow_mut() = fx.clone();
            fx
        };
        let out = canvas::show(
            ui,
            view,
            &geom,
            &quads,
            sel_rect,
            &extra_rects,
            &member_hl,
            &handles,
            &drag_fx,
            &guide_fx,
            &jump_fx,
            &underlays,
            self.tool == Tool::Connector,
            self.tool == Tool::Lasso,
            self.tool == Tool::Freeform,
            self.tool == Tool::Pan,
            armed,
            sel_line_curved,
            &conn_fx,
            &snap_end,
            &snap_corner,
            &hit,
            &mut mouse,
            // While the in-place text editor is open (#22) the canvas
            // is interaction-locked: a plain click ends the edit and
            // nothing else reports (no select/marquee/dbl-click).
            self.text_edit.is_some(),
        );
        self.mouse_page = mouse;
        self.canvas_rect = out.canvas_rect;
        // #283 review: the live drag ghost's exact screen rect (the
        // canvas's own quad_to_screen mapping), for the #96 contract.
        self.last_drag_ghost_rect = out.drag_ghost_rects.first().copied();
        self.conn_fx_last = conn_fx_seen.into_inner();
        if self.text_edit.is_some() {
            // Click anywhere on the canvas = click outside the text:
            // end the edit (commit; the editor decided Esc the same
            // way). No other canvas event is reported while locked.
            if out.click_page_pos.is_some() {
                self.end_text_edit(true);
            }
        } else {
            // An AutoConnect arrow click (#108) consumes the gesture
            // BEFORE the empty-page click semantics run — the arrow
            // sits outside the shape, and a plain click there would
            // clear the selection the AutoConnect needs.
            if let Some(dir) = out.autoclick {
                self.autoconnect(dir);
            }
            // #283: the Pan tool's clicks are inert — no selection, no
            // double-click text edit, no move-drag arming (right-click
            // keeps its context menu, like Visio's pan mode).
            let pan_clicks = self.tool != Tool::Pan;
            if pan_clicks {
                if let Some(pos) = out.dbl_click_page_pos {
                    // The second click of the pair completes a double-click
                    // instead of a plain click: the first click already ran its
                    // select (and possible drag-start) semantics on its frame.
                    self.on_canvas_dbl_click(pos);
                } else if out.autoclick.is_none() {
                    if let Some(pos) = out.click_page_pos {
                        self.on_canvas_click(pos, out.shift_click, out.ctrl_click);
                    }
                }
                if let Some(pos) = out.press_page_pos {
                    self.on_canvas_press(pos);
                }
            }
            // Marquee/lasso/freeform/connector drops need no gating:
            // the Pan tool's press-arming gate in the canvas means
            // those gestures never report under it.
            if let Some((x0, y0, x1, y1)) = out.marquee_done {
                self.apply_marquee(x0, y0, x1, y1);
            }
            // The Lasso trail finished (#110): touch/enclose selection.
            if let Some(trail) = out.lasso_done {
                self.apply_lasso(&trail);
            }
            // The Freeform stroke finished (#132): build the shape.
            if let Some(stroke) = out.freeform_done {
                self.create_freeform(stroke);
            }
            // A ruler drag released over the page drops a guide
            // (#113, URS-1076).
            if let Some((src, pos)) = out.guide_drop {
                self.add_guide(src, pos);
            }
            // The Connector Tool's gesture (#108, URS-1067).
            if let Some((from, to)) = out.connector_drop {
                self.create_connector(from, to);
            }
        }
        // Comment badges (#29 rework): a small bubble at the top-left
        // corner of every commented shape — the shipped 2016 indicator,
        // non-intrusive (an overlay, never part of the drawing), shown
        // while comments are visible; clicking opens the thread popup.
        if self.show_comments {
            let targets: Vec<i64> = self
                .ed()
                .map(|ed| {
                    let page =
                        &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
                    page.shapes
                        .iter()
                        .filter(|s| {
                            s.sheet
                                .sections_named("User")
                                .iter()
                                .find_map(|sec| sec.named_row("AnnotationRef"))
                                .and_then(|r| r.cell("Value"))
                                .and_then(|c| c.numeric())
                                .is_some_and(|v| page.shapes.iter().any(|t| t.id as f64 == v))
                        })
                        .filter_map(|s| {
                            s.sheet
                                .sections_named("User")
                                .iter()
                                .find_map(|sec| sec.named_row("AnnotationRef"))
                                .and_then(|r| r.cell("Value"))
                                .and_then(|c| c.numeric())
                                .map(|v| v as i64)
                        })
                        .collect::<Vec<i64>>()
                })
                .unwrap_or_default();
            // Distinct targets, badge per target.
            let mut seen = std::collections::HashSet::new();
            for target in targets {
                if !seen.insert(target) {
                    continue;
                }
                let count = self.comments_for(target).len();
                let Some(r) = self.shape_screen_rect(target) else {
                    continue;
                };
                let badge = Rect::from_min_size(
                    r.left_top() + Vec2::new(-9.0, -9.0),
                    Vec2::new(18.0, 15.0),
                );
                let resp = ui.interact(
                    badge,
                    egui::Id::unique(("dc-comment-badge", target)),
                    Sense::click(),
                );
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        crate::i18n::strs().msg_comments_on_shape_target_count(target, count),
                    )
                });
                let t = theme::shell_tokens(ui.visuals());
                ui.painter().rect_filled(badge, 4.0, t.accent_band);
                ui.painter().rect_stroke(
                    badge,
                    4.0,
                    Stroke::new(1.0, Color32::WHITE),
                    egui::StrokeKind::Middle,
                );
                crate::ribbon::Icon::Comment.paint(ui, badge, true);
                let gal = ui.painter().layout_no_wrap(
                    format!("{count}"),
                    egui::FontId::proportional(9.0),
                    Color32::WHITE,
                );
                ui.painter().galley(
                    Pos2::new(
                        badge.right() - gal.size().x + 2.0,
                        badge.bottom() - gal.size().y,
                    ),
                    gal,
                    Color32::WHITE,
                );
                if resp.clicked() {
                    self.comment_target = Some(target);
                    self.comment_body.clear();
                    self.comment_edit = None;
                }
            }
        }
        if let Some((id, start, end)) = out.handle_drag_done {
            // A press on a handle released without travel is just a
            // click: committing it would create no-op undo steps (or a
            // bogus "refused" status for an unprotected shape).
            let travel_px = (end.0 - start.0).hypot(end.1 - start.1) * self.view.px_per_in() as f64;
            if travel_px > 1.0 {
                match id {
                    HandleId::Corner(corner) => {
                        // #160: snap the resized edge under the snap
                        // settings — the same per-axis snap the live
                        // preview showed. Snap the CORNER position
                        // (handle origin + drag delta), not the raw
                        // pointer: the grab may start up to
                        // HANDLE_GRAB away from the corner, and
                        // snapping the pointer would leave the edge
                        // at grid + grab-offset (CodeRabbit). Applied
                        // AFTER the travel check so a no-travel press
                        // never gains a phantom snap.
                        let dragged = self.sel.map(|(_, s)| s);
                        let origin = handles.iter().find(|h| h.id == id).map(|h| h.pos);
                        let end = match (dragged, origin) {
                            (Some(sid), Some(o)) => {
                                let raw = (o.0 + end.0 - start.0, o.1 + end.1 - start.1);
                                self.snap_corner_for(sid, raw)
                                    .map(|sn| (start.0 + sn.0 - o.0, start.1 + sn.1 - o.1))
                                    .unwrap_or(end)
                            }
                            _ => end,
                        };
                        // y-down screen-orientation delta, as before.
                        let dx = end.0 - start.0;
                        let dy = start.1 - end.1;
                        self.apply_resize(corner, dx, dy);
                    }
                    HandleId::LineBegin | HandleId::LineEnd => {
                        // The endpoint moves by the drag DELTA from the
                        // handle's own position — the grab may start up
                        // to HANDLE_GRAB px away from the handle center,
                        // and snapping to the raw pointer would jump it.
                        // #108: connectors get the full glue lifecycle —
                        // a drop on a connection point (static) or a
                        // shape (dynamic) writes the corpus formula +
                        // `<Connect>`; a drop on empty page releases
                        // the glue. Plain 1D shapes keep the #109 glue
                        // snap with a note (their glue rides with
                        // connectors-as-masters later).
                        if let Some(origin) = handles.iter().find(|h| h.id == id).map(|h| h.pos) {
                            let raw = (origin.0 + end.0 - start.0, origin.1 + end.1 - start.1);
                            let dragged = self.sel.map(|(_, s)| s);
                            let is_conn = dragged.is_some_and(|sid| {
                                self.ed.as_ref().is_some_and(|ed| {
                                    let page = &ed.doc.pages[self.page_idx];
                                    vsdx::model::find_shape_by_id(&page.shapes, sid)
                                        .and_then(|sh| {
                                            shape_chain(&ed.doc, page.id, sid)
                                                .ok()
                                                .map(|ch| (sh, ch))
                                        })
                                        .is_some_and(|(sh, ch)| {
                                            crate::connectors::is_connector_in(&ed.doc, &ch, sh)
                                        })
                                })
                            });
                            if is_conn {
                                // #179: snap the drop like the plain
                                // 1D path — Visio's endpoint magnetizes
                                // to connection points (and the other
                                // snap targets) on release.
                                let snapped =
                                    dragged.map_or(raw, |sid| self.snap_endpoint_for(sid, raw));
                                self.commit_connector_endpoint(id, snapped);
                            } else {
                                let glue =
                                    dragged.and_then(|sid| self.snap_point_for(sid, raw, true));
                                let (target, note) = match glue {
                                    Some((p, hit)) => (
                                        p,
                                        Some(
                                            crate::i18n::strs()
                                                .msg_endpoint_glued_to(snap_hit_text(hit)),
                                        ),
                                    ),
                                    None => (raw, None),
                                };
                                self.apply_line_endpoint(id, target);
                                if let Some(note) = note {
                                    self.status = note;
                                }
                            }
                        }
                    }
                    HandleId::LineMid => {
                        // #172: connectors translate like any shape —
                        // free ends move plainly, glued ends move through
                        // the GUARD-waived write that KEEPS the glue
                        // formula (the reroute tail then re-attaches
                        // them, Visio's pinned feel). Master instances
                        // count as connectors (ObjType on the master).
                        let is_conn = self.sel.is_some_and(|(_, sid)| {
                            self.ed.as_ref().is_some_and(|ed| {
                                let page = &ed.doc.pages[self.page_idx];
                                vsdx::model::find_shape_by_id(&page.shapes, sid)
                                    .and_then(|sh| {
                                        shape_chain(&ed.doc, page.id, sid).ok().map(|ch| (sh, ch))
                                    })
                                    .is_some_and(|(sh, ch)| {
                                        crate::connectors::is_connector_in(&ed.doc, &ch, sh)
                                    })
                            })
                        });
                        if is_conn {
                            self.apply_connector_body_move(start, end);
                        } else {
                            self.apply_line_move(start, end);
                        }
                    }
                    HandleId::GeoPoint {
                        section_ix,
                        row_ix,
                        cell,
                    } => {
                        // The DELTA from the handle's own position (the
                        // grab may start inside the grab radius).
                        // #109: a vertex drag runs through the snap
                        // engine's point snap (grid, ruler, alignment
                        // targets — no glue; glue is for connectors).
                        if let Some(origin) = handles.iter().find(|h| h.id == id).map(|h| h.pos) {
                            let raw = (origin.0 + end.0 - start.0, origin.1 + end.1 - start.1);
                            let target = self
                                .sel
                                .and_then(|(_, sid)| self.snap_point_for(sid, raw, false))
                                .map_or(raw, |(p, _)| p);
                            self.apply_geo_vertex(section_ix, row_ix, cell, target);
                        }
                    }
                }
            }
        }
        if let Some(pos) = out.right_click_page_pos {
            // Visio: right-click selects the shape under the cursor and
            // opens its context menu (empty page = no shape menu yet).
            // Guides carry no shape context menu (#113): they are
            // page furniture — right-click on one opens nothing.
            let hit = self.shape_hit(pos).and_then(|id| {
                let ed = self.ed()?;
                let page_id = self.page_id()?;
                let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
                match vsdx::model::find_shape_by_id(&page.shapes, id) {
                    Some(sh) if crate::guides::is_guide(sh) => None,
                    _ => Some(id),
                }
            });
            match hit {
                Some(shape_id) => {
                    if let Some(p) = self.page_id() {
                        // Right-clicking a member of the current
                        // multi-selection keeps the selection (Visio's
                        // Group flow depends on it, #26); clicking
                        // outside it collapses to the hit shape.
                        if !self.sel_set.contains(&(p, shape_id)) {
                            self.set_selection(Some((p, shape_id)));
                        }
                    }
                    let anchor = ui
                        .ctx()
                        .input(|i| i.pointer.interact_pos())
                        .unwrap_or_default();
                    self.ctx_menu = Some((shape_id, anchor, pos));
                    self.page_menu = None;
                }
                None => {
                    self.ctx_menu = None;
                    // #272: the empty page carries its own menu.
                    self.page_menu = ui.ctx().input(|i| i.pointer.interact_pos());
                }
            }
        }
        // In-place WYSIWYG text editor overlay (GitHub #22): laid over
        // the canvas at the shape's text-block rect, styled from the
        // same resolved cells the renderer paints with. The entry zoom
        // (once) focuses the block; ending flows (Esc, click outside)
        // commit through the retired dialog's SetText path.
        if self.text_edit.is_some() {
            if let Some(sess) = self.text_edit.as_mut() {
                if let Some(ed) = self.ed.as_ref() {
                    if let Some(page) = ed.doc.pages.iter().find(|p| p.id == sess.page) {
                        if let Some(b) = dc::svg::text_block_page_rect(&ed.doc, page, sess.shape) {
                            sess.enter_zoom(&mut self.view, b, self.canvas_rect, &geom);
                        }
                    }
                }
            }
            let mut flow = crate::textedit::EditFlow::Active;
            if let Some(sess) = self.text_edit.as_mut() {
                if let Some(ed) = self.ed.as_ref() {
                    // The session's page, not the active tab: the edit
                    // can outlive a page switch (tabs sit outside the
                    // canvas's interaction lock), and shape ids collide
                    // across pages — indexing by page_idx would lay the
                    // overlay over the wrong shape's text block. No
                    // matching page: skip the overlay this frame (#22
                    // review).
                    if let Some(page) = ed.doc.pages.iter().find(|p| p.id == sess.page) {
                        flow = crate::textedit::show_overlay(
                            ui, sess, &ed.doc, page, &geom, &self.view,
                        );
                    }
                }
            }
            match flow {
                // A click that opened the Insert Field dialog does
                // NOT also end the edit (#267: the dialog inserts at
                // the caret mid-edit, like Visio's).
                crate::textedit::EditFlow::Commit if self.fields_open.is_some() => {}
                crate::textedit::EditFlow::Commit => self.end_text_edit(true),
                crate::textedit::EditFlow::Abandon => self.end_text_edit(false),
                crate::textedit::EditFlow::Active => {}
            }
        }
        self.shape_context_menu(ui.ctx());
        self.page_context_menu(ui.ctx());
        self.quick_shapes_menu(ui.ctx());
        self.tab_context_menu(ui.ctx());
    }
}
