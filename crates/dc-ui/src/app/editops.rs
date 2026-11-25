//! Selection editing: delete/marquee/lasso, resize and line-endpoint
//! drags, the move-command plumbing, and align/distribute/rotate/
//! select-all/format-painter.

use super::*;

impl App {
    pub(super) fn delete_selection(&mut self) {
        let Some((page_id, _)) = self.sel else { return };
        if self.sel_set.len() > 1 {
            // One undo step for a multi-delete.
            let ids: Vec<i64> = self
                .sel_set
                .iter()
                .filter(|(p, _)| *p == page_id)
                .map(|(_, s)| *s)
                .collect();
            let mut cmds: Vec<Box<dyn dc_core::Command>> = ids
                .iter()
                .map(|&s| Box::new(RemoveShape::new(page_id, s)) as Box<dyn dc_core::Command>)
                .collect();
            // #138: deleting a container strips its members' membership
            // rows in the same Macro — no dangling rows in the file,
            // one undo restores everything.
            if let Some(ed) = self.ed() {
                cmds.extend(dc_core::container_membership_cleanup(
                    &ed.doc, page_id, &ids,
                ));
                for sid in &ids {
                    cmds.extend(self.guide_glue_release_cmds(page_id, *sid));
                }
            }
            cmds.extend(self.connect_cleanup_cmds(page_id, &ids));
            let n = ids.len();
            self.apply(Macro::new(
                crate::i18n::strs().msg_undo_delete_n_shapes(n as i64),
                cmds,
            ));
        } else {
            let shape_id = self.sel.unwrap().1;
            self.delete_one(page_id, shape_id);
        }
        self.set_selection(None);
        self.sel_set.clear();
    }

    /// Delete a single shape, folding membership-row cleanup into the
    /// same undo step when it is a container (#138) and glue-release
    /// when it is a guide (#113: dependents keep their position, the
    /// pin formulas referencing the gone guide are stripped).
    pub(super) fn delete_one(&mut self, page_id: i64, shape_id: i64) {
        let mut cmds: Vec<Box<dyn dc_core::Command>> =
            vec![Box::new(RemoveShape::new(page_id, shape_id))];
        cmds.extend(self.guide_glue_release_cmds(page_id, shape_id));
        cmds.extend(self.connect_cleanup_cmds(page_id, &[shape_id]));
        if let Some(ed) = self.ed() {
            cmds.extend(dc_core::container_membership_cleanup(
                &ed.doc,
                page_id,
                &[shape_id],
            ));
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_delete_shape(shape_id),
            cmds,
        ));
    }

    /// Marquee selection: page-inch rect (y-down) → every top-level
    /// shape whose bbox intersects joins the selection set.
    pub(super) fn apply_marquee(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        let Some(ed) = self.ed() else { return };
        let Some(page_id) = self.page_id() else {
            return;
        };
        let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
        let mut picked = Vec::new();
        // Group-edit mode (#78): the marquee selects among the open
        // group's members only.
        let mut members = Vec::new();
        if let Some(g) = self.group_edit.last().copied() {
            if let Some(group) = vsdx::model::find_shape_by_id(&page.shapes, g) {
                collect_members(&group.sub_shapes, &mut members);
            }
        } else {
            members.extend(page.shapes.iter());
        }
        for shape in members {
            if let Some((sx, sy, w, h)) = self.shape_rect(&ed.doc, page_id, shape) {
                // Both the shape rect and the marquee rect (from
                // canvas::screen_to_page) are y-up page inches.
                if sx <= x1 && sx + w >= x0 && sy <= y1 && sy + h >= y0 {
                    picked.push((page_id, shape.id));
                }
            }
        }
        self.sel_set = picked;
        self.sel = self.sel_set.last().copied();
    }

    /// #110 (Visio's Lasso Select): the freehand trail (page inches,
    /// y-up) selects every shape it touches or encloses — the marquee's
    /// group-edit semantics, the lasso's touch geometry.
    pub(super) fn apply_lasso(&mut self, trail: &[(f64, f64)]) {
        let Some(ed) = self.ed() else { return };
        let Some(page_id) = self.page_id() else {
            return;
        };
        let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
        let mut picked = Vec::new();
        // Group-edit mode (#78): the lasso selects among the open
        // group's members only, like the marquee.
        let mut members = Vec::new();
        if let Some(g) = self.group_edit.last().copied() {
            if let Some(group) = vsdx::model::find_shape_by_id(&page.shapes, g) {
                collect_members(&group.sub_shapes, &mut members);
            }
        } else {
            members.extend(page.shapes.iter());
        }
        for shape in members {
            if let Some((sx, sy, w, h)) = self.shape_rect(&ed.doc, page_id, shape) {
                // Closure tolerance from the view scale: the pointer's
                // return to the start reads as a closed loop within a
                // few SCREEN pixels, whatever the zoom (inches/px).
                let tol = 4.0 / self.view.px_per_in() as f64;
                if canvas::lasso_hits_rect(trail, sx, sy, w, h, tol) {
                    picked.push((page_id, shape.id));
                }
            }
        }
        let n = picked.len();
        self.sel_set = picked;
        self.sel = self.sel_set.last().copied();
        self.status = crate::i18n::strs().msg_lasso_selected_n_shape_s(n);
    }

    /// #110 tab-order audit: Esc dismisses the frontmost open dialog
    /// (Visio behavior) before the tool/painter shortcuts see the key.
    /// Only one dialog is realistically open at a time; with several,
    /// each Esc closes the next in a fixed order. The Hyperlinks
    /// window manages its OWN Esc (a draft discards first — closing
    /// the window on the same key would lose the layering), and the
    /// confirm-close/new modals handle theirs. Returns true when one
    /// closed.
    pub(super) fn close_front_dialog(&mut self) -> bool {
        if self.selby_open {
            self.selby_open = false;
            return true;
        }
        // #121: the Check Spelling dialog.
        if self.spell.session.is_some() {
            self.spell.session = None;
            return true;
        }
        if self.snap_dlg_open {
            self.snap_dlg_open = false;
            return true;
        }
        if self.pagesetup_open {
            self.pagesetup_open = false;
            return true;
        }
        if self.copypic_open {
            self.copypic_open = false;
            return true;
        }
        if self.contgal_open {
            self.contgal_open = false;
            return true;
        }
        if self.backgrounds_open {
            self.backgrounds_open = false;
            return true;
        }
        if self.reorder_open {
            self.reorder_open = false;
            return true;
        }
        if self.color_pick.is_some() {
            self.color_pick = None;
            return true;
        }
        // The ShapeSheet WINDOWS (#190/#192): persistent surfaces like
        // the dialogs — Esc dismisses the most recently opened one
        // after the dialogs.
        if self.shapesheets.pop().is_some() {
            return true;
        }
        false
    }

    /// The resize/lock guard for one axis of a 2D resize (GitHub #59):
    /// Some(reason) when the size cell resolves to a formula (`GUARD(…)`,
    /// any function — a silent overwrite is never acceptable) or the
    /// Protection section locks it (`LockWidth`/`LockHeight`).
    pub(super) fn resize_guard(&self, size_cell: &str, lock_cell: &str) -> Option<String> {
        let ed = self.ed()?;
        let (page_id, shape_id) = self.sel?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        // Effective size cell formula: instance-local first, then the
        // master's (a GUARD in the master protects instances too).
        let resolved = ed.doc.resolve_cell(sh, &chain, size_cell);
        if let Some(f) = resolved.as_ref().and_then(|c| c.f.as_deref()) {
            if f != "Inh" {
                return Some(crate::i18n::strs().msg_size_cell_is_formula_driven_f(size_cell, f));
            }
        }
        if protection_flag(&ed.doc, sh, &chain, lock_cell) {
            return Some(format!("{lock_cell} is set (Protection section)"));
        }
        None
    }

    /// Commit a corner-handle resize: the dragged corner moves by the
    /// page-inch delta (y-down); the opposite corner stays anchored and
    /// LocPin stays proportional (Visio's `Width*0.5` semantics).
    /// Guarded axes (formula or Protection lock) refuse with a status
    /// note instead of being overwritten (#59); the min-size clamp
    /// stays.
    pub(super) fn apply_resize(&mut self, corner: canvas::Corner, dx: f64, dy: f64) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        let (w_guard, h_guard) = (
            self.resize_guard("Width", "LockWidth"),
            self.resize_guard("Height", "LockHeight"),
        );
        if let (Some(w), Some(h)) = (&w_guard, &h_guard) {
            self.status = format!(
                "resize refused: {} and {} — shape {shape_id} is protected",
                w, h
            );
            return;
        }
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        // #95: a member's cells are parent-local — convert the
        // page-space drag delta through the renderer's affine (any
        // base point works; the translation cancels in the
        // difference).
        let (dx, dy) = if chain.len() >= 2 {
            let page_h = Self::page_height(ed, page_id);
            let l0 = dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, (0.0, 0.0));
            let l1 = dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, (dx, dy));
            (l1.0 - l0.0, l1.1 - l0.1)
        } else {
            (dx, dy)
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let (Some(px), Some(py), Some(w), Some(h), Some(lx), Some(ly)) = (
            get("PinX"),
            get("PinY"),
            get("Width"),
            get("Height"),
            get("LocPinX"),
            get("LocPinY"),
        ) else {
            return;
        };
        // y-down delta → y-up geometry deltas per corner.
        let (dw, dh) = match corner {
            canvas::Corner::TopLeft => (-dx, -dy),
            canvas::Corner::TopRight => (dx, -dy),
            canvas::Corner::BottomLeft => (-dx, dy),
            canvas::Corner::BottomRight => (dx, dy),
        };
        let (w, h) = (w.max(1e-6), h.max(1e-6)); // ratio guards
                                                 // A guarded axis keeps its size; only the free axis changes.
        let nw = if w_guard.is_some() {
            w
        } else {
            (w + dw).max(0.05)
        };
        let nh = if h_guard.is_some() {
            h
        } else {
            (h + dh).max(0.05)
        };
        if (nw - w).abs() < 1e-9 && (nh - h).abs() < 1e-9 {
            self.status = crate::i18n::strs().msg_resize_refused_shape_is_protecte();
            return;
        }
        // The partial-guard note is written AFTER `apply`, which
        // overwrites the status with the command label.
        let guard_note = match (&w_guard, &h_guard) {
            (Some(g), _) => Some(crate::i18n::strs().msg_width_not_resized_guard(g.clone())),
            (_, Some(g)) => Some(crate::i18n::strs().msg_height_not_resized_guard(g.clone())),
            _ => None,
        };
        // Anchor the OPPOSITE corner: fractions (0|1) of its y-up bbox
        // position, selected by the dragged (screen-oriented) corner.
        // LocPin stays proportional (Visio's Width*0.5 semantics).
        let (ax, ay) = match corner {
            canvas::Corner::TopLeft => (1.0, 0.0),  // anchor bottom-right
            canvas::Corner::TopRight => (0.0, 0.0), // anchor bottom-left
            canvas::Corner::BottomLeft => (1.0, 1.0), // anchor top-right
            canvas::Corner::BottomRight => (0.0, 1.0), // anchor top-left
        };
        let anchor = (px + ax * w - lx, py + ay * h - ly);
        let nlx = lx * nw / w;
        let nly = ly * nh / h;
        let npx = anchor.0 - ax * nw + nlx;
        let npy = anchor.1 - ay * nh + nly;
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        if w_guard.is_none() {
            cmds.push(Box::new(SetCell::new(scope.clone(), "Width", cell(nw))));
            cmds.push(Box::new(SetCell::new(scope.clone(), "LocPinX", cell(nlx))));
        }
        if h_guard.is_none() {
            cmds.push(Box::new(SetCell::new(scope.clone(), "Height", cell(nh))));
            cmds.push(Box::new(SetCell::new(scope.clone(), "LocPinY", cell(nly))));
        }
        cmds.push(Box::new(SetCell::new(scope.clone(), "PinX", cell(npx))));
        cmds.push(Box::new(SetCell::new(scope, "PinY", cell(npy))));
        // A group resize scales its members (#69): literal Xform cells
        // (the common case — GroupShapes writes literals when grouping)
        // scale here; formula-driven cells stay with the cache refresh
        // below (#15's per-shape rules).
        if sh.type_.as_deref() == Some("Group") {
            cmds.extend(dc_core::scale_group_members(
                &ed.doc,
                page_id,
                shape_id,
                nw / w,
                nh / h,
            ));
        }
        // Refresh the flattened group's inherited caches (sub-shape
        // XForms, Txt block, row coordinates) to the new size — Visio's
        // save-time refresh; without it a real-file group renders stale
        // inside the grown selection box (#59).
        cmds.extend(crate::cmd::group_cache_refresh(
            &ed.doc,
            page_id,
            shape_id,
            nw / w,
            nh / h,
        ));
        if let Some(r) = self.reroute_cmd(page_id, &[shape_id]) {
            cmds.push(r);
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_resize_shape(shape_id),
            cmds,
        ));
        if let Some(note) = guard_note {
            self.status = note;
        }
    }

    /// Guard for a 1D endpoint/midpoint drag: refuse when any of the
    /// cells the drag rewrites resolves to a formula, or when the
    /// Protection section locks the endpoints (`LockBegin`/`LockEnd`).
    pub(super) fn line_drag_guard(&self, cells: &[&str], locks: &[&str]) -> Option<String> {
        let ed = self.ed()?;
        let (page_id, shape_id) = self.sel?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        for name in cells {
            let resolved = ed.doc.resolve_cell(sh, &chain, name);
            if let Some(f) = resolved.as_ref().and_then(|c| c.f.as_deref()) {
                if f != "Inh" {
                    return Some(crate::i18n::strs().msg_cell_is_formula_driven(name, f));
                }
            }
        }
        for lock in locks {
            if protection_flag(&ed.doc, sh, &chain, lock) {
                return Some(crate::i18n::strs().msg_lock_is_set_protection_section(lock));
            }
        }
        None
    }

    /// Drag one endpoint of a 1D shape to a new page position; the
    /// other endpoint stays anchored. Rewrites `BeginX`/`BeginY` (or
    /// `EndX`/`EndY`) plus the derived XForm cells the renderer reads
    /// (Width, Angle, Pin, LocPin) as consistent literals — one undo
    /// step (#59).
    pub(super) fn apply_line_endpoint(&mut self, end: HandleId, to: (f64, f64)) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        // Only the lock matching the dragged endpoint guards the drag
        // (Visio: LockBegin locks the begin point alone).
        let (guard_cells, lock): (Vec<&str>, &str) = match end {
            HandleId::LineBegin => (vec!["BeginX", "BeginY"], "LockBegin"),
            HandleId::LineEnd => (vec!["EndX", "EndY"], "LockEnd"),
            _ => return,
        };
        if let Some(reason) = self.line_drag_guard(&guard_cells, &[lock]) {
            self.status = crate::i18n::strs().msg_endpoint_drag_refused_reason(reason);
            return;
        }
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        // #95: endpoint cells are parent-local for a member — the
        // page-space target converts through the affine; the other
        // endpoint already reads parent-local.
        let to = if chain.len() >= 2 {
            let page_h = Self::page_height(ed, page_id);
            dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, to)
        } else {
            to
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let (Some(bx), Some(by), Some(ex), Some(ey)) =
            (get("BeginX"), get("BeginY"), get("EndX"), get("EndY"))
        else {
            return;
        };
        let w0 = get("Width").unwrap_or(1.0);
        let (a, b) = if end == HandleId::LineBegin {
            (to, (ex, ey))
        } else {
            ((bx, by), to)
        };
        let w = (a.0 - b.0).hypot(a.1 - b.1).max(1e-4);
        let angle = (b.1 - a.1).atan2(b.0 - a.0);
        let (pin, other) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![
            SetCell::new(scope.clone(), "BeginX", cell(a.0)),
            SetCell::new(scope.clone(), "BeginY", cell(a.1)),
            SetCell::new(scope.clone(), "EndX", cell(b.0)),
            SetCell::new(scope.clone(), "EndY", cell(b.1)),
        ]
        .into_iter()
        .map(|c| Box::new(c) as Box<dyn dc_core::Command>)
        .collect();
        // Derived cells: skip any that a formula owns (never overwrite a
        // guarded cell; the note is written after `apply`, which would
        // otherwise overwrite the status with the command label).
        let mut kept_note: Option<String> = None;
        for (name, v) in [
            ("Width", w),
            ("Angle", angle),
            ("PinX", pin),
            ("PinY", other),
            ("LocPinX", w / 2.0),
            ("LocPinY", 0.0),
        ] {
            // effective_formula, not the instance cell's own F — an
            // 'Inh' PinX over a master's GUARD((BeginX+EndX)/2) is
            // still formula-driven to the command layer, and a refused
            // write aborts the whole macro (#172).
            let formula = dc_core::commands::effective_formula(&ed.doc, sh, &chain, name);
            if formula.is_some() {
                kept_note = Some(crate::i18n::strs().msg_endpoint_moved_but_name_is_formu(name));
                continue;
            }
            cmds.push(Box::new(SetCell::new(scope.clone(), name, cell(v))));
        }
        // A 1D shape's cached row coordinates scale with its Width.
        cmds.extend(crate::cmd::group_cache_refresh(
            &ed.doc,
            page_id,
            shape_id,
            w / w0.max(1e-6),
            1.0,
        ));
        self.apply(Macro::new(
            crate::i18n::strs().msg_move_line_endpoint_shape_shape_i(shape_id),
            cmds,
        ));
        if let Some(note) = kept_note {
            self.status = note;
        }
    }

    /// Drag a 1D shape's midpoint: the whole line translates, both
    /// endpoints keep their separation (Visio's straight-line midpoint
    /// handle). One undo step (#59).
    pub(super) fn apply_line_move(&mut self, from: (f64, f64), to: (f64, f64)) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        // Guard the endpoint cells only: real Visio 1D masters commonly
        // derive PinX/PinY through GUARD((BeginX+EndX)/2) — a formula
        // there must not refuse the drag, the cell is simply kept (the
        // endpoints carry the geometry).
        if let Some(reason) = self.line_drag_guard(
            &["BeginX", "BeginY", "EndX", "EndY"],
            &["LockBegin", "LockEnd"],
        ) {
            self.status = crate::i18n::strs().msg_line_move_refused_reason(reason);
            return;
        }
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let Some(((bx, by), (ex, ey))) = get("BeginX")
            .zip(get("BeginY"))
            .zip(get("EndX").zip(get("EndY")))
        else {
            return;
        };
        let Some((px, py)) = get("PinX").zip(get("PinY")) else {
            return;
        };
        // #95: member cells are parent-local — convert the page-space
        // drag through the affine (translation cancels).
        let (dx, dy) = if chain.len() >= 2 {
            let page_h = Self::page_height(ed, page_id);
            let f0 = dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, (0.0, 0.0));
            let f1 = dc::svg::page_point_to_parent_local(
                &ed.doc,
                &chain,
                page_h,
                (to.0 - from.0, to.1 - from.1),
            );
            (f1.0 - f0.0, f1.1 - f0.1)
        } else {
            (to.0 - from.0, to.1 - from.1)
        };
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        // Pin cells are derived: keep any a formula owns instead of
        // overwriting the guard.
        let mut kept_note: Option<String> = None;
        let mut cmds: Vec<(&str, f64)> = vec![
            ("BeginX", bx + dx),
            ("BeginY", by + dy),
            ("EndX", ex + dx),
            ("EndY", ey + dy),
        ];
        for (name, v) in [("PinX", px + dx), ("PinY", py + dy)] {
            // effective_formula (#172): an 'Inh' instance cell over a
            // master's GUARD would otherwise refuse and abort the move.
            let formula = dc_core::commands::effective_formula(&ed.doc, sh, &chain, name);
            if formula.is_some() {
                kept_note = Some(crate::i18n::strs().msg_line_moved_but_name_is_formula_d(name));
            } else {
                cmds.push((name, v));
            }
        }
        let cmds: Vec<Box<dyn dc_core::Command>> = cmds
            .into_iter()
            .map(|(n, v)| {
                Box::new(SetCell::new(scope.clone(), n, cell(v))) as Box<dyn dc_core::Command>
            })
            .collect();
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_move_line_shape(shape_id),
            cmds,
        ));
        if let Some(note) = kept_note {
            self.status = note;
        }
    }

    /// #172: translate a CONNECTOR by dragging its body. Free endpoints
    /// move as plain values; glued endpoints carry their glue formula
    /// (`PAR(PNT(...))`/`_WALKGLUE`) and move through [`SetGluedCell`]
    /// — the GUARD-waived write that keeps the formula — exactly the
    /// reroute engine's own cache-update pattern, so the glue survives
    /// and the reroute tail re-attaches the moved ends (a both-ends
    /// glued connector pins back to its targets, like Visio). Derived
    /// XForm cells a formula owns are kept, never literal-written (an
    /// aborted macro was the #172 "drag does nothing").
    pub(super) fn apply_connector_body_move(&mut self, from: (f64, f64), to: (f64, f64)) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        // Locks refuse; formulas do not (they ride SetGluedCell).
        if let Some(reason) = self.line_lock_guard(&["LockBegin", "LockEnd"]) {
            self.status = crate::i18n::strs().msg_connector_move_refused_reason(reason);
            return;
        }
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let Some(((bx, by), (ex, ey))) = get("BeginX")
            .zip(get("BeginY"))
            .zip(get("EndX").zip(get("EndY")))
        else {
            return;
        };
        // A shape without a pin is not a movable 1D frame.
        if get("PinX").zip(get("PinY")).is_none() {
            return;
        }
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        // #202 owner rule: connector glue RELEASES once the drag
        // carries an end past the gluepoint snap radius — within it,
        // the reroute pins the glued end back to its anchor (Visio's
        // snap-back); past it the end detaches and moves freely.
        let dist = dx.hypot(dy);
        let radius = SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64;
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        // Endpoints: a formula-owned end (glue: PAR(PNT(...)) /
        // _WALKGLUE) is NOT written — the formula owns its value too,
        // and a moved cache would desync render from evaluation. It
        // stays pinned to its target (Visio's feel) UNLESS the drag
        // carries it past the snap radius, when it releases (the
        // Connect record drops and the endpoint cells become plain
        // values — glue formulas replaced, so nothing can snap the
        // end back to a target that no longer owns it). A free end
        // moves as a plain value through the GUARD-waived write.
        let mut free: [bool; 2] = [true, true];
        let mut releases: Vec<usize> = Vec::new(); // Connect indexes
        let mut release_ends = 0usize;
        for (i, (name, v)) in [
            ("BeginX", bx + dx),
            ("BeginY", by + dy),
            ("EndX", ex + dx),
            ("EndY", ey + dy),
        ]
        .into_iter()
        .enumerate()
        {
            if dc_core::commands::effective_formula(&ed.doc, sh, &chain, name).is_some() {
                free[i / 2] = false;
                continue;
            }
            cmds.push(Box::new(dc_core::SetGluedCell::new(
                scope.clone(),
                name,
                cell(v),
            )));
        }
        if dist > radius {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id);
            let list = page.map(|p| p.connects()).unwrap_or_default();
            for (end, cells) in [(0usize, ["BeginX", "BeginY"]), (1usize, ["EndX", "EndY"])] {
                if free[end] {
                    continue;
                }
                // Release: a plain value write replaces the cell
                // (dropping the glue formula — the next drag sees a
                // free end) and the Connect record drops with it.
                if let Some(idx) = list
                    .list
                    .iter()
                    .position(|c| c.from_sheet == shape_id && c.from_cell == cells[0])
                {
                    releases.push(idx);
                }
                for (name, v) in [
                    (cells[0], if end == 0 { bx + dx } else { ex + dx }),
                    (cells[1], if end == 0 { by + dy } else { ey + dy }),
                ] {
                    cmds.push(Box::new(dc_core::SetGluedCell::new(
                        scope.clone(),
                        name,
                        cell(v),
                    )));
                }
                release_ends += 1;
            }
            // Highest index first: RemoveConnect plans by index, and
            // each removal shifts the tail.
            releases.sort_unstable_by(|a, b| b.cmp(a));
            for idx in releases {
                cmds.push(Box::new(dc_core::RemoveConnect::new(
                    dc_core::ConnectsOwner::Page(page_id),
                    idx,
                )));
            }
        }
        if !free[0] && !free[1] && release_ends == 0 {
            self.status = crate::i18n::strs().msg_connector_is_glued_at_both_ends();
            return;
        }
        // The frame (Pin/Width/Height/LocPin caches) and the route
        // rows derive from the POST-MOVE endpoints — the reroute
        // planner's own corpus rules (review pass: the hand-rolled
        // writes here materialized missing Geometry rows on
        // master-based connectors and skipped formula-owned Pin
        // caches; the planner refreshes through guards and only
        // rewrites rows the connector already has).
        cmds.push(Box::new(crate::cmd::RerouteConnectors::new_including(
            page_id,
            vec![shape_id],
        )));
        // Connectors glued TO this one re-attach in the same undo
        // step.
        if let Some(r) = self.reroute_cmd(page_id, &[shape_id]) {
            cmds.push(r);
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_move_connector(shape_id),
            cmds,
        ));
        if release_ends > 0 {
            self.status = crate::i18n::strs().msg_connector_moved_release_ends_glu(release_ends);
        }
    }

    /// The Protection locks of the selected 1D shape, without the
    /// formula half of [`Self::line_drag_guard`] (glue formulas are
    /// expected on connectors and ride [`dc_core::SetGluedCell`]).
    pub(super) fn line_lock_guard(&self, locks: &[&str]) -> Option<String> {
        let ed = self.ed()?;
        let (page_id, shape_id) = self.sel?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        for lock in locks {
            if protection_flag(&ed.doc, sh, &chain, lock) {
                return Some(crate::i18n::strs().msg_lock_is_set_protection_section(lock));
            }
        }
        None
    }
}

impl App {
    pub(super) fn apply_boxed(&mut self, label: &str, cmd: Box<dyn dc_core::Command>) {
        self.apply(Macro::new(label, vec![cmd]));
    }

    /// The move command for one shape at a target pin: a 1D shape
    /// translates Begin/End/Pin together (a pin-only move would leave
    /// the endpoint cells stale — the handles would sit at the old
    /// positions and the next endpoint drag would snap the line back,
    /// CodeRabbit review of df4a562); 2D shapes move the pin alone.
    /// Page height in inches — the y-flip anchor of the XForm chains.
    pub(super) fn page_height(ed: &Editor, page_id: i64) -> f64 {
        ed.doc
            .pages
            .iter()
            .find(|p| p.id == page_id)
            .and_then(|p| p.sheet())
            .and_then(|sh| sh.cell("PageHeight"))
            .and_then(|c| c.numeric())
            .unwrap_or(11.0)
    }

    pub(super) fn move_cmd_for(
        &self,
        page_id: i64,
        sid: i64,
        new_pin: (f64, f64),
    ) -> Box<dyn dc_core::Command> {
        let Some(ed) = self.ed() else {
            return Box::new(MoveShape::new(page_id, sid, new_pin));
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Box::new(MoveShape::new(page_id, sid, new_pin));
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
            return Box::new(MoveShape::new(page_id, sid, new_pin));
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
            return Box::new(MoveShape::new(page_id, sid, new_pin));
        };
        // A group member's Pin cells are PARENT-LOCAL (#78): convert
        // the page-space drag target into the member's parent frame
        // through the same XForm chain the renderer composes.
        let new_pin = if chain.len() >= 2 {
            let page_h = Self::page_height(ed, page_id);
            dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, new_pin)
        } else {
            new_pin
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let Some(((bx, by), (ex, ey), (px, py))) = get("BeginX")
            .zip(get("BeginY"))
            .zip(get("EndX").zip(get("EndY")))
            .zip(get("PinX").zip(get("PinY")))
            .map(|(((bx, by), (ex, ey)), (px, py))| ((bx, by), (ex, ey), (px, py)))
        else {
            return Box::new(MoveShape::new(page_id, sid, new_pin));
        };
        let (dx, dy) = (new_pin.0 - px, new_pin.1 - py);
        let scope = Scope::Shape {
            page: page_id,
            shape: sid,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        // Real 1D masters commonly derive PinX/PinY through
        // GUARD((BeginX+EndX)/2) — a formula-driven pin must not be
        // literal-written (the command layer now refuses it, which
        // would abort the whole move Macro); the endpoints carry the
        // geometry, exactly as apply_line_move's guard documents.
        let formula_pinned =
            |n: &str| dc_core::commands::effective_formula(&ed.doc, sh, &chain, n).is_some();
        let cmds: Vec<Box<dyn dc_core::Command>> = [
            ("BeginX", bx + dx),
            ("BeginY", by + dy),
            ("EndX", ex + dx),
            ("EndY", ey + dy),
            ("PinX", px + dx),
            ("PinY", py + dy),
        ]
        .into_iter()
        .filter(|(n, _)| !matches!(*n, "PinX" | "PinY") || !formula_pinned(n))
        .map(|(n, v)| {
            Box::new(SetCell::new(scope.clone(), n, cell(v))) as Box<dyn dc_core::Command>
        })
        .collect();
        Box::new(Macro::new(
            crate::i18n::strs().msg_undo_move_line_shape(sid),
            cmds,
        ))
    }

    // -- snap & glue (GitHub #109) ----------------------------------------
}

impl App {
    /// #110 (URS-1149): align every selected shape's feature to the
    /// first-selected shape (or the page while `align_to_page`): one
    /// undo Macro of plain moves.
    pub(super) fn align_selection(&mut self, kind: crate::align::AlignKind) {
        use crate::align::AlignTarget;
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let page = &ed.doc.pages[self.page_idx];
        // Selection order: the primary leads (URS's first-selected).
        let mut ids: Vec<i64> = Vec::new();
        if let Some((p, first)) = self.sel {
            if p == page_id && !ids.contains(&first) {
                ids.push(first);
            }
        }
        for (p, sid) in &self.sel_set {
            if *p == page_id && !ids.contains(sid) {
                ids.push(*sid);
            }
        }
        let mut shapes: Vec<(i64, (f64, f64, f64, f64))> = Vec::new();
        for sid in &ids {
            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) else {
                continue;
            };
            if let Some(r) = self.shape_rect(&ed.doc, page_id, sh) {
                shapes.push((*sid, r));
            }
        }
        if shapes.len() < 2 && !self.align_to_page {
            self.status = crate::i18n::strs().msg_select_two_or_more_shapes_to_ali();
            return;
        }
        if shapes.is_empty() {
            return;
        }
        let geom = self.page_geom();
        let target = if self.align_to_page {
            AlignTarget::Page
        } else {
            AlignTarget::FirstSelected
        };
        let moves = crate::align::align_moves(&shapes, kind, target, (geom.w_in, geom.h_in));
        self.apply_moves(
            &crate::i18n::strs().msg_undo_align_what(kind.label_i18n()),
            page_id,
            &moves,
        );
    }

    /// #110 (URS-1161): distribute the selection with even gaps
    /// (3+ shapes, Visio's "make spacing equal").
    pub(super) fn distribute_selection(&mut self, vertical: bool) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let page = &ed.doc.pages[self.page_idx];
        let mut shapes: Vec<(i64, (f64, f64, f64, f64))> = Vec::new();
        for (p, sid) in &self.sel_set {
            if *p != page_id {
                continue;
            }
            if let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) {
                if let Some(r) = self.shape_rect(&ed.doc, page_id, sh) {
                    shapes.push((*sid, r));
                }
            }
        }
        match crate::align::distribute_moves(&shapes, vertical) {
            Some(moves) => {
                let label = if vertical {
                    crate::i18n::strs().msg_distribute_vertical_spacing()
                } else {
                    crate::i18n::strs().msg_distribute_horizontal_spacing()
                };
                self.apply_moves(&label, page_id, &moves);
            }
            None => self.status = crate::i18n::strs().msg_distribute_needs_three_or_more_s(),
        }
    }

    /// Apply (id, delta) moves as one undo Macro — the same move path
    /// drags use, so 1D members translate their endpoints too.
    pub(super) fn apply_moves(&mut self, label: &str, page_id: i64, moves: &[(i64, (f64, f64))]) {
        let Some(ed) = self.ed() else { return };
        let page = &ed.doc.pages[self.page_idx];
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (sid, (dx, dy)) in moves {
            if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
                continue;
            }
            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) else {
                continue;
            };
            let Ok(chain) = shape_chain(&ed.doc, page_id, *sid) else {
                continue;
            };
            let pin = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
            if let (Some(px), Some(py)) = (pin("PinX"), pin("PinY")) {
                cmds.push(self.move_cmd_for(page_id, *sid, (px + dx, py + dy)));
            }
        }
        if cmds.is_empty() {
            self.status = crate::i18n::strs().msg_already_aligned();
            return;
        }
        self.apply(Macro::new(label.to_string(), cmds));
    }

    /// #110 (URS-1217 slice): keyboard nudge — one undo step per
    /// press, the whole selection moves.
    pub(super) fn nudge_selection(&mut self, dx: f64, dy: f64) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let ids: Vec<i64> = if self.sel_set.len() > 1 {
            self.sel_set
                .iter()
                .filter(|(p, _)| *p == page_id)
                .map(|(_, s)| *s)
                .collect()
        } else {
            self.sel.map(|(_, s)| vec![s]).unwrap_or_default()
        };
        let moves: Vec<(i64, (f64, f64))> = ids.iter().map(|id| (*id, (dx, dy))).collect();
        self.apply_moves(&crate::i18n::strs().msg_nudge_selection(), page_id, &moves);
    }

    /// #180: the documented Visio unglue-all operation — any arrow key
    /// with glued connectors selected releases every end of every
    /// selected connector in ONE undo step ("select one or more
    /// connectors and press any arrow key", MS Learn — Glue or unglue
    /// connectors). The press is consumed: a glued connector does not
    /// nudge until unglued, Visio's own tradeoff. Returns false when
    /// nothing in the selection is glued so the arrow falls through to
    /// nudge; locked ends refuse atomically through the Macro
    /// rollback (a failed child leaves the document untouched).
    pub(super) fn unglue_selection_connectors(&mut self) -> bool {
        let Some(page_id) = self.page_id() else {
            return false;
        };
        let ids: Vec<i64> = if self.sel_set.len() > 1 {
            self.sel_set
                .iter()
                .filter(|(p, _)| *p == page_id)
                .map(|(_, s)| *s)
                .collect()
        } else {
            self.sel.map(|(_, s)| vec![s]).unwrap_or_default()
        };
        let Some(ed) = self.ed() else {
            return false;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return false;
        };
        let (mut cmds, released) = crate::connectors::unglue_cmds(&ed.doc, page, &ids);
        if released.is_empty() {
            return false;
        }
        // The released connectors' routes refresh in the same undo
        // step (FixedRoute skips), and anything glued TO them
        // re-attaches — the same tail every release path carries.
        cmds.push(Box::new(crate::cmd::RerouteConnectors::new_including(
            page_id,
            released.clone(),
        )));
        if let Some(r) = self.reroute_cmd(page_id, &released) {
            cmds.push(r);
        }
        let n = released.len();
        self.apply(Macro::new(
            crate::i18n::strs().msg_unglue_connectors(n),
            cmds,
        ));
        true
    }
}

impl App {
    /// #110 (URS-1160): select every shape on the page.
    pub(super) fn select_all(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let page = &ed.doc.pages[self.page_idx];
        let ids: Vec<(i64, i64)> = page.shapes.iter().map(|s| (page_id, s.id)).collect();
        if ids.is_empty() {
            return;
        }
        self.sel = Some(ids[0]);
        self.sel_set = ids;
        self.status = crate::i18n::strs().msg_selected_all_shapes();
    }

    /// #110 (URS-1053): arm the Format Painter on the primary
    /// selection — the next shape click receives its resolved
    /// formatting (single-shot, Visio's click mode).
    pub(super) fn arm_format_painter(&mut self, sticky: bool) {
        // A second click while armed disarms (Visio's toggle).
        if self.painter_armed.is_some() && !sticky {
            self.painter_armed = None;
            self.painter_sticky = false;
            self.status = crate::i18n::strs().msg_format_painter_cancelled();
            return;
        }
        let Some((_, sid)) = self.sel else {
            self.status = crate::i18n::strs().msg_select_a_shape_to_copy_formattin();
            return;
        };
        self.painter_armed = Some(sid);
        self.painter_sticky = sticky;
        self.status = if sticky {
            crate::i18n::strs().msg_format_painter_click_shapes_to_a(sid)
        } else {
            crate::i18n::strs().msg_format_painter_click_a_shape_to(sid)
        };
    }

    /// The painter's application: copy the resolved FORMAT_CELLS from
    /// the source onto the target as one undo Macro.
    pub(super) fn apply_format_painter(&mut self, from: i64, to: i64) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let page = &ed.doc.pages[self.page_idx];
        let (Some(src), Some(_dst)) = (
            vsdx::model::find_shape_by_id(&page.shapes, from),
            vsdx::model::find_shape_by_id(&page.shapes, to),
        ) else {
            return;
        };
        let Ok(src_chain) = shape_chain(&ed.doc, page_id, from) else {
            return;
        };
        let scope = Scope::Shape {
            page: page_id,
            shape: to,
        };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for name in crate::align::FORMAT_CELLS {
            if let Some(c) = ed.doc.resolve_cell(src, &src_chain, name) {
                if let Some(v) = c.numeric() {
                    cmds.push(Box::new(SetCell::new(
                        scope.clone(),
                        *name,
                        vsdx::Cell::value(format!("{v:.9}")),
                    )));
                }
            }
        }
        let n = cmds.len();
        self.apply(Macro::new(
            crate::i18n::strs().msg_paint_formatting_onto_shape_to(to),
            cmds,
        ));
        // #110: the sticky (double-clicked) painter stays armed for
        // the next click; the single-shot one is spent.
        if self.painter_sticky {
            self.status = crate::i18n::strs().msg_painter_cells_sticky(n, to);
        } else {
            self.status = crate::i18n::strs().msg_painter_cells(n, to);
            self.painter_armed = None;
        }
    }
}

impl App {
    pub(super) fn rotate_selection(&mut self, deg: f64) {
        let targets = self.sel_set.clone();
        let Some(ed) = self.ed.as_ref() else { return };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (page_id, sid) in targets {
            let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
                continue;
            };
            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
                continue;
            };
            let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
                continue;
            };
            let cur = ed
                .doc
                .resolve_cell(sh, &chain, "Angle")
                .and_then(|c| c.numeric())
                .unwrap_or(0.0);
            let next = cur + deg.to_radians();
            cmds.push(Box::new(SetCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                "Angle",
                vsdx::Cell::value(format!("{next:.8}")),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(crate::i18n::strs().msg_rotate_shapes(), cmds));
        }
    }
}
