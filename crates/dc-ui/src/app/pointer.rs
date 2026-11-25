//! Pointer interaction on the canvas: drag updates, click/press/
//! double-click handling, master placement, selection changes.

use super::*;

impl App {
    /// Track an in-progress shape drag; commit on release.
    pub(super) fn update_drag(&mut self, ctx: &egui::Context) {
        let (pressed, pos) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
        let pointer = match self.ed() {
            Some(_) => pos,
            None => return,
        };
        let geom = self.page_geom();

        if pressed {
            // The drag is taken out for the frame so the snap pass can
            // borrow the app (#109); it goes straight back.
            if let (Some(p), Some(mut d)) = (pointer, self.drag.take()) {
                let x_in = (p.x - self.view.pan.x) as f64 / self.view.px_per_in() as f64;
                let y_up =
                    geom.h_in - (p.y - self.view.pan.y) as f64 / self.view.px_per_in() as f64;
                let new_pin = (x_in + d.grab_off.0, y_up + d.grab_off.1);
                let new_pin = self.apply_drag_snap(&d, new_pin);
                if (new_pin.0 - d.last_pin.0).abs() * self.view.px_per_in() as f64 > 0.5
                    || (new_pin.1 - d.last_pin.1).abs() * self.view.px_per_in() as f64 > 0.5
                {
                    d.moved = true;
                }
                d.last_pin = new_pin;
                self.drag = Some(d);
            }
        } else if let Some(d) = self.drag.take() {
            self.snap_live.clear();
            if d.moved {
                if let Some(page_id) = self.page_id() {
                    // Moving a multi-selection moves every member by the
                    // same delta, as one undo step. The group was
                    // captured at press time (members of the selection
                    // the press actually landed on). Each member moves
                    // through `move_cmd_for`: 1D members translate their
                    // endpoints too, not just the pin.
                    // A guide drag (#113) is its own commit: the
                    // guide moves and its glued dependents follow.
                    let dragging_guide = self.ed.as_ref().is_some_and(|ed| {
                        let page =
                            &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
                        vsdx::model::find_shape_by_id(&page.shapes, d.shape)
                            .is_some_and(crate::guides::is_guide)
                    });
                    if dragging_guide {
                        self.apply_guide_move(page_id, d.shape, d.last_pin);
                    } else if d.group.is_empty() {
                        // A container drag carries its members (Visio,
                        // #27): one undo step.
                        let member_ids = self.container_member_ids(page_id, d.shape);
                        if member_ids.is_empty() {
                            let label = crate::i18n::strs().msg_undo_move_shape(d.shape);
                            let cmd = self.move_cmd_for(page_id, d.shape, d.last_pin);
                            // #108: glued connectors re-attach and
                            // reroute in the same undo step.
                            let reroute = self.reroute_cmd(page_id, &[d.shape]);
                            // #113: a shape dropped onto a guide with
                            // glue-to-guides on glues its pin(s) to it
                            // — Visio's `PinX = Sheet.N!PinX` formula
                            // with the snapped cached value. Only for
                            // plain 2D shapes (1D endpoint glue to
                            // guides rides with #108's connectors).
                            let glue = self.drag_glue.take().filter(|_| {
                                self.ed.as_ref().is_some_and(|ed| {
                                    let page = &ed.doc.pages[self.page_idx];
                                    vsdx::model::find_shape_by_id(&page.shapes, d.shape)
                                        .and_then(|sh| {
                                            let chain =
                                                shape_chain(&ed.doc, page_id, d.shape).ok()?;
                                            ed.doc
                                                .resolve_cell(sh, &chain, "BeginX")
                                                .and_then(|c| c.numeric())
                                        })
                                        .is_none()
                                })
                            });
                            match glue {
                                Some((guide_id, gx, gy)) => {
                                    // Guide glue ties the PIN to the
                                    // guide (Visio semantics: the pin
                                    // rides the line). An edge-snap
                                    // must not glue — the formula
                                    // would jump the shape half its
                                    // width at drop — so each axis
                                    // glues only when the snapped pin
                                    // coordinate already IS the
                                    // guide's.
                                    let g = self
                                        .page_guides()
                                        .iter()
                                        .find(|g| g.id == guide_id)
                                        .copied();
                                    let axis_target = |axis: &str| -> Option<f64> {
                                        let g = g?;
                                        match (axis, g.kind) {
                                            ("PinX", crate::guides::GuideKind::Vertical)
                                            | ("PinX", crate::guides::GuideKind::Point) => {
                                                Some(g.x)
                                            }
                                            ("PinY", crate::guides::GuideKind::Horizontal)
                                            | ("PinY", crate::guides::GuideKind::Point) => {
                                                Some(g.y)
                                            }
                                            _ => None,
                                        }
                                    };
                                    let mut glue_cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
                                    for (axis, want) in [("PinX", gx), ("PinY", gy)] {
                                        let Some(target) = axis_target(axis) else {
                                            continue;
                                        };
                                        let pin = if axis == "PinX" {
                                            d.last_pin.0
                                        } else {
                                            d.last_pin.1
                                        };
                                        if want && (pin - target).abs() < 1e-6 {
                                            glue_cmds.push(Box::new(SetCell::new(
                                                Scope::Shape {
                                                    page: page_id,
                                                    shape: d.shape,
                                                },
                                                axis,
                                                vsdx::Cell {
                                                    v: Some(format!("{target:.6}")),
                                                    f: Some(crate::guides::glue_formula(
                                                        guide_id, axis,
                                                    )),
                                                    ..vsdx::Cell::default()
                                                },
                                            )));
                                        }
                                    }
                                    glue_cmds.insert(0, cmd);
                                    if let Some(r) = reroute {
                                        glue_cmds.push(r);
                                    }
                                    self.apply(Macro::new(label, glue_cmds));
                                }
                                None => match reroute {
                                    Some(r) => {
                                        let mut all = vec![cmd];
                                        all.push(r);
                                        self.apply(Macro::new(label, all));
                                    }
                                    None => self.apply_boxed(&label, cmd),
                                },
                            }
                        } else {
                            let (dx, dy) =
                                (d.last_pin.0 - d.start_pin.0, d.last_pin.1 - d.start_pin.1);
                            let mut ids_for_reroute: Vec<i64> = vec![d.shape];
                            ids_for_reroute.extend(member_ids.iter().copied());
                            let mut cmds: Vec<Box<dyn dc_core::Command>> =
                                vec![self.move_cmd_for(page_id, d.shape, d.last_pin)];
                            if let Some(ed) = self.ed() {
                                let page = &ed.doc.pages[self.page_idx];
                                for sid in member_ids {
                                    let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
                                        continue;
                                    };
                                    let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid)
                                    else {
                                        continue;
                                    };
                                    let g = |n: &str| {
                                        ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric())
                                    };
                                    if let (Some(px), Some(py)) = (g("PinX"), g("PinY")) {
                                        // Member pins are parent-local
                                        // (#78): map to page space,
                                        // apply the page-space delta,
                                        // and let move_cmd_for convert
                                        // the target back — a raw
                                        // local+page sum misplaces
                                        // members of transformed
                                        // groups.
                                        let target = if chain.len() >= 2 {
                                            let page_h = Self::page_height(ed, page_id);
                                            dc::svg::parent_local_point_to_page(
                                                &ed.doc,
                                                &chain,
                                                page_h,
                                                (px, py),
                                            )
                                        } else {
                                            (px, py)
                                        };
                                        cmds.push(self.move_cmd_for(
                                            page_id,
                                            sid,
                                            (target.0 + dx, target.1 + dy),
                                        ));
                                    }
                                }
                            }
                            if let Some(r) = self.reroute_cmd(page_id, &ids_for_reroute) {
                                cmds.push(r);
                            }
                            self.apply(Macro::new(
                                crate::i18n::strs().msg_move_container_and_members(),
                                cmds,
                            ));
                        }
                    } else {
                        let Some(ed) = self.ed() else { return };
                        let dx = d.last_pin.0 - d.start_pin.0;
                        let dy = d.last_pin.1 - d.start_pin.1;
                        let ids_for_reroute: Vec<i64> = std::iter::once(d.shape)
                            .chain(d.group.iter().copied())
                            .collect();
                        let mut cmds: Vec<Box<dyn dc_core::Command>> =
                            vec![self.move_cmd_for(page_id, d.shape, d.last_pin)];
                        for sid in &d.group {
                            let page = &ed.doc.pages[self.page_idx];
                            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) else {
                                continue;
                            };
                            let Ok(chain) = shape_chain(&ed.doc, page_id, *sid) else {
                                continue;
                            };
                            let get = |n: &str| {
                                ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric())
                            };
                            if let (Some(px), Some(py)) = (get("PinX"), get("PinY")) {
                                // Parent-local pin → page space (see
                                // the container path above).
                                let target = if chain.len() >= 2 {
                                    let page_h = Self::page_height(ed, page_id);
                                    dc::svg::parent_local_point_to_page(
                                        &ed.doc,
                                        &chain,
                                        page_h,
                                        (px, py),
                                    )
                                } else {
                                    (px, py)
                                };
                                cmds.push(self.move_cmd_for(
                                    page_id,
                                    *sid,
                                    (target.0 + dx, target.1 + dy),
                                ));
                            }
                        }
                        if let Some(r) = self.reroute_cmd(page_id, &ids_for_reroute) {
                            cmds.push(r);
                        }
                        self.apply(Macro::new(crate::i18n::strs().msg_move_shapes(), cmds));
                    }
                    self.set_selection(Some((page_id, d.shape)));
                }
            }
        }
    }
}

impl App {
    /// Route every single-selection write through here so `sel` and
    /// `sel_set` stay consistent: the primary is always a member, and a
    /// cleared selection clears the set. Multi-selection paths manage
    /// `sel_set` directly and then set `sel`.
    pub(super) fn set_selection(&mut self, sel: Option<(i64, i64)>) {
        self.sel = sel;
        match sel {
            Some(entry) if !self.sel_set.contains(&entry) => self.sel_set = vec![entry],
            Some(_) => {}
            None => self.sel_set.clear(),
        }
    }

    pub(super) fn on_canvas_click(&mut self, page_pos: (f64, f64), shift: bool, ctrl: bool) {
        self.placed_last_click = false;
        // The Connection Point tool owns clicks on shapes (#108):
        // near an existing point selects it (Delete removes); on the
        // body adds one at the click.
        if self.tool == Tool::ConnectionPoint && !ctrl {
            self.handle_conn_point_click(page_pos);
            return;
        }
        // Ctrl/Cmd+click follows the hit shape's first hyperlink
        // (Visio, #28) before any selection semantics.
        if ctrl {
            if let Some(hit) = self.shape_hit(page_pos) {
                if self.follow_first_hyperlink(hit) {
                    return;
                }
            }
        }
        // Stencil placement takes priority.
        if let Some(key) = self.stencil.armed {
            self.stencil.armed = None;
            self.ctx_menu = None;
            self.placed_last_click = true;
            if let Some(nid) = self.place_master(key, page_pos) {
                if let Some(page_id) = self.page_id() {
                    self.set_selection(Some((page_id, nid)));
                    self.sel_set = vec![(page_id, nid)];
                }
            }
            return;
        }
        // The armed Format Painter consumes the click (#110): the hit
        // shape receives the source's resolved formatting. The sticky
        // (double-clicked) painter stays armed; empty-page or
        // on-the-source clicks cancel either way.
        if let Some(src) = self.painter_armed {
            if let Some(hit) = self.shape_hit(page_pos) {
                if hit != src {
                    self.apply_format_painter(src, hit);
                    if let Some(p) = self.page_id() {
                        self.set_selection(Some((p, hit)));
                    }
                    return;
                }
            }
            self.painter_armed = None;
            self.painter_sticky = false;
            self.status = crate::i18n::strs().msg_format_painter_cancelled();
            return;
        }
        if shift {
            // Toggle membership in the multi-selection.
            if let Some(hit) = self.shape_hit(page_pos) {
                if let Some(p) = self.page_id() {
                    let entry = (p, hit);
                    if let Some(i) = self.sel_set.iter().position(|e| *e == entry) {
                        self.sel_set.remove(i);
                        self.sel = self.sel_set.last().copied();
                    } else {
                        self.sel_set.push(entry);
                        self.sel = Some(entry);
                    }
                }
            }
            self.ctx_menu = None;
            return;
        }
        match self.shape_hit(page_pos) {
            Some(shape_id) => {
                let page_id = self.page_id().expect("page");
                // Clicking a member of the current multi-selection keeps
                // the set (so a drag moves every member); clicking
                // anything else selects just that shape.
                if !self.sel_set.contains(&(page_id, shape_id)) {
                    self.sel_set = vec![(page_id, shape_id)];
                }
                self.set_selection(Some((page_id, shape_id)));
                self.status = crate::i18n::strs().msg_selected_shape_shape_id(shape_id);
                // #156: selecting a container raises the contextual
                // Container Tools tab (Visio 2016).
                if self.sel_is_container() {
                    self.tab = Tab::ContainerTools;
                }
            }
            None => {
                self.set_selection(None);
            }
        }
    }

    /// Place the armed/carried master `key` at `page_pos` (inches).
    /// Returns the new shape id when a placement happened.
    ///
    /// Document masters place directly. A master from an open stencil
    /// is first copied into the drawing (Visio's first-use import,
    /// #84) and the instance placed referencing the document-local
    /// copy — one undo step removes both together.
    pub(super) fn place_master(&mut self, key: MasterKey, page_pos: (f64, f64)) -> Option<i64> {
        let page_id = self.page_id()?;
        match key.0 {
            StencilId::Document => {
                let new_id = self
                    .ed
                    .as_ref()
                    .map(|ed| dc_core::commands::next_shape_id(&ed.doc, page_id));
                self.apply(AddMasterInstance::new(page_id, key.1, page_pos));
                new_id
            }
            StencilId::Open(ix) => {
                let src = self
                    .stencils
                    .get(ix)
                    .and_then(|st| st.doc.master(key.1))
                    .cloned();
                let Some(src) = src else {
                    self.stencil.armed = None;
                    return None;
                };
                let ed = self.ed.as_ref()?;
                let new_id = dc_core::commands::next_shape_id(&ed.doc, page_id);
                let cmd = dc_core::place_master_from_stencil(&ed.doc, page_id, &src, page_pos);
                self.apply(cmd);
                Some(new_id)
            }
        }
    }

    /// Primary press on a shape (page inches, y-up), reported by the
    /// canvas on the press frame. Arms the move-drag — the click path
    /// cannot: egui's click-vs-drag disambiguation means a natural
    /// press-drag-release never fires `clicked()`, so arming there made
    /// moves no-ops (GitHub #59). Selection itself still happens on the
    /// release-as-click (`on_canvas_click`).
    pub(super) fn on_canvas_press(&mut self, page_pos: (f64, f64)) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(shape_id) = self.shape_hit(page_pos) else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let page = &ed.doc.pages[self.page_idx];
        let Some(s) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        let pin = ed
            .doc
            .resolve_cell(s, &chain, "PinX")
            .and_then(|c| c.numeric())
            .zip(
                ed.doc
                    .resolve_cell(s, &chain, "PinY")
                    .and_then(|c| c.numeric()),
            );
        let Some((px, py)) = pin else { return };
        // The drag tracks the pin in PAGE space: a nested member's Pin
        // cells are parent-local (#78), so map them through the XForm
        // chain — the gesture delta and move_cmd_for's page-space
        // targets must agree (a mixed-frame pin misplaces members of
        // translated/rotated groups).
        let (pin_px, pin_py) = if chain.len() >= 2 {
            let page_h = Self::page_height(ed, page_id);
            dc::svg::parent_local_point_to_page(&ed.doc, &chain, page_h, (px, py))
        } else {
            (px, py)
        };
        // Group semantics at press time: pressing a member of the
        // current multi-selection drags the whole set; pressing an
        // unselected shape moves only that shape (its release-click then
        // selects it solo).
        let group: Vec<i64> = if self.sel_set.contains(&(page_id, shape_id)) {
            self.sel_set
                .iter()
                .filter(|(p, s)| *p == page_id && *s != shape_id)
                .map(|(_, s)| *s)
                .collect()
        } else {
            Vec::new()
        };
        self.drag = Some(DragState {
            shape: shape_id,
            grab_off: (pin_px - page_pos.0, pin_py - page_pos.1),
            start_pin: (pin_px, pin_py),
            last_pin: (pin_px, pin_py),
            moved: false,
            group,
        });
    }

    /// Double-click on the canvas (page inches, y-up): Visio's route into
    /// a shape's linked text. The shape's `EventDblClick` formula,
    /// resolved through the inheritance chain (instance → master, like
    /// the Paragraph/Character handling), decides what happens.
    pub(super) fn on_canvas_dbl_click(&mut self, page_pos: (f64, f64)) {
        // A stencil placement wins the click pair: its first click
        // placed the master, and the pair must not also open an editor.
        if self.stencil.armed.is_some() || self.placed_last_click {
            self.placed_last_click = false;
            return;
        }
        let Some(shape_id) = self.shape_hit(page_pos) else {
            return;
        };
        let Some(page_id) = self.page_id() else {
            return;
        };
        self.set_selection(Some((page_id, shape_id)));
        // A guide has no text and no event to run (#113): selecting
        // it is the whole double-click.
        if let Some(ed) = self.ed() {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id);
            if let Some(sh) = page.and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id))
            {
                if crate::guides::is_guide(sh) {
                    return;
                }
            }
        }
        let formula = self.ed.as_ref().and_then(|ed| {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
            let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
            ed.doc
                .resolve_cell(sh, &chain, "EventDblClick")
                .and_then(|c| c.f)
        });
        match classify_dbl_click(formula.as_deref()) {
            DblClick::OpenText => {
                // The F2 path reached by mouse: seed the draft and let
                // the in-place editor run it.
                let draft = self.selected_text(shape_id).unwrap_or_default();
                self.begin_text_edit(shape_id, draft);
                self.status = crate::i18n::strs().msg_editing_text_of_shape_shape_id(shape_id);
            }
            DblClick::Deferred(msg) => self.status = msg,
        }
    }
}
