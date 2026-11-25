//! Connectors: endpoint commit and autoconnect, connection points,
//! route styles, jump lines, glue candidates, and creation.

use super::*;

impl App {
    /// #108: a connector endpoint drag's glue lifecycle. Build the
    /// endpoint cells (the #59 derived-cell logic via
    /// `apply_line_endpoint` semantics), then resolve glue at the drop:
    /// static (connection point) or dynamic (shape) write the corpus
    /// formula and Connect; empty page releases. One undo step.
    pub(super) fn commit_connector_endpoint(&mut self, end_id: HandleId, raw: (f64, f64)) {
        use crate::connectors::{End, GlueKind};
        let Some((page_id, sid)) = self.sel else {
            return;
        };
        let end = match end_id {
            HandleId::LineBegin => End::Begin,
            HandleId::LineEnd => End::End,
            _ => return,
        };
        // Where the OTHER endpoint sits (page space) — the facing
        // anchor for dynamic attachment.
        let other = self.ed.as_ref().and_then(|ed| {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
            let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
            let (b, e) = match end {
                End::Begin => ("EndX", "EndY"),
                End::End => ("BeginX", "BeginY"),
            };
            let g = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
            Some((g(b)?, g(e)?))
        });
        let Some(other) = other else { return };
        let cands = self.glue_candidates(&[sid]);
        let threshold = SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64;
        // #202: the endpoint drop glues STRICTLY — a connection point
        // within the snap radius, or a shape under the cursor (no
        // halo: the ring around every shape re-attached endpoints the
        // cursor never touched, pinning them to the shape's edges).
        let target = crate::connectors::glue_decision_ex(
            &cands,
            raw,
            other,
            threshold,
            self.snap.glue_active,
            self.snap.glue_connection_points,
            false,
        );
        let attach = target.map(|t| t.attach).unwrap_or(raw);
        // The plain endpoint move first (derived cells + guards).
        let mut cmds: Vec<Box<dyn dc_core::Command>> =
            vec![self.move_endpoint_cmd(page_id, sid, end, attach)];
        // Resolve the glue state change against the existing Connect.
        let page = self
            .ed
            .as_ref()
            .and_then(|ed| ed.doc.pages.iter().find(|p| p.id == page_id));
        let existing = page.and_then(|p| {
            p.connects()
                .list
                .iter()
                .position(|c| c.from_sheet == sid && c.from_cell == end.cell())
        });
        let same_glue = |t: &crate::connectors::GlueTarget, idx: usize| -> bool {
            let Some(p) = page else { return false };
            let list = p.connects();
            let Some(c) = list.list.get(idx) else {
                return false;
            };
            c.to_sheet == t.shape_id
                && c.to_part
                    == match t.kind {
                        GlueKind::Static { row_ix } => 100 + row_ix,
                        GlueKind::Dynamic => crate::connectors::PART_WALKGLUE,
                    }
        };
        if let Some(idx) = existing {
            cmds.push(Box::new(dc_core::RemoveConnect::new(
                dc_core::ConnectsOwner::Page(page_id),
                idx,
            )));
        }
        if let Some(t) = target {
            // Formula cells overwrite the plain values from the move.
            let (f, trig) = match t.kind {
                GlueKind::Static { row_ix } => (
                    crate::connectors::static_glue_formula(t.shape_id, row_ix),
                    None,
                ),
                GlueKind::Dynamic => (crate::connectors::walkglue_formula(end), Some(t.shape_id)),
            };
            for cell in [end.cell(), end.y_cell()] {
                cmds.push(Box::new(dc_core::SetGluedCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: sid,
                    },
                    cell,
                    vsdx::Cell {
                        v: Some(format!(
                            "{:.6}",
                            if cell == end.cell() {
                                attach.0
                            } else {
                                attach.1
                            }
                        )),
                        f: Some(f.clone()),
                        ..vsdx::Cell::default()
                    },
                )));
            }
            if let Some(tid) = trig {
                cmds.push(Box::new(SetCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: sid,
                    },
                    end.trigger_cell(),
                    vsdx::Cell {
                        v: Some("2".into()),
                        f: Some(format!("GUARD(_XFTRIGGER(Sheet.{tid}!EventXFMod))")),
                        ..vsdx::Cell::default()
                    },
                )));
            }
            cmds.push(Box::new(dc_core::AddConnect {
                owner: dc_core::ConnectsOwner::Page(page_id),
                connect: crate::connectors::glue_connect(sid, end, &t),
            }));
            let same = existing.is_some_and(|idx| same_glue(&t, idx));
            self.status = if same {
                crate::i18n::strs().msg_connector_stays_glued(t.shape_id)
            } else {
                match t.kind {
                    GlueKind::Static { .. } => {
                        crate::i18n::strs().msg_connector_glued_shape_point(t.shape_id)
                    }
                    GlueKind::Dynamic => crate::i18n::strs().msg_connector_glued_shape(t.shape_id),
                }
            };
        } else if existing.is_some() {
            self.status = crate::i18n::strs().msg_connector_end_released();
        }
        // #179: the drop moved an endpoint — a master-based
        // connector's formula-driven frame caches and geometry rows
        // must refresh in the SAME undo step or the render keeps the
        // pre-drag pose (the glue writes alone leave Pin/Width/Height
        // stale under their GUARDs). Planned by ID: works for the
        // re-glue and the release alike. The PRE-drag endpoints ride
        // along (#218 review): the endpoint writes above land first,
        // so re-reading the pose here would name the NEW ends and the
        // literal-row translation delta would collapse to zero.
        let pre_pose = self.ed.as_ref().and_then(|ed| {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
            let chain = dc_core::scope::shape_chain(&ed.doc, page_id, sid).ok()?;
            let g = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
            Some(((g("BeginX")?, g("BeginY")?), (g("EndX")?, g("EndY")?)))
        });
        let mut reroute = crate::cmd::RerouteConnectors::new_including(page_id, vec![sid]);
        if let Some(pre) = pre_pose {
            reroute = crate::cmd::RerouteConnectors::new_including_with_pre(
                page_id,
                vec![sid],
                [(sid, pre)].into_iter().collect(),
            );
        }
        cmds.push(Box::new(reroute));
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_glue_connector(sid, crate::connectors::endpoint_word(end)),
            cmds,
        ));
    }

    /// #108 AutoConnect: an arrow click duplicates the selected shape
    /// one spacing unit in the arrow's direction and connects the pair
    /// (dynamic glue both ends) — one undo Macro, Visio's
    /// click-the-blue-arrow flow.
    pub(super) fn autoconnect(&mut self, dir: (f64, f64)) {
        use crate::connectors::{
            attach_point, bake_end, bake_trigger, connector_shape, glue_connect,
            rewrite_route_rows, route, route_rows, walkglue_formula, End, GlueKind, GlueTarget,
            ROUTE_RIGHT_ANGLE,
        };
        let Some((page_id, sid)) = self.sel else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
            return;
        };
        if crate::connectors::is_connector(sh) || crate::guides::is_guide(sh) {
            self.status = crate::i18n::strs().msg_autoconnect_needs_a_2d_shape();
            return;
        }
        let Some(rect) = self.shape_rect(&ed.doc, page_id, sh) else {
            return;
        };
        // Visio's spacing: one shape-size plus a gap, along the arrow.
        let gap = 0.4;
        let offset = (
            dir.0 * (rect.2 + gap) * dir.0.abs(),
            dir.1 * (rect.3 + gap) * dir.1.abs(),
        );
        let dup_id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let conn_id = dup_id + 1;
        let src_c = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
        let dup_rect = (rect.0 + offset.0, rect.1 + offset.1, rect.2, rect.3);
        let dup_c = (dup_rect.0 + rect.2 / 2.0, dup_rect.1 + rect.3 / 2.0);
        let beg_attach = attach_point(rect, dup_c);
        let end_attach = attach_point(dup_rect, src_c);
        let mut conn = connector_shape(conn_id, beg_attach, end_attach);
        let pts = route(ROUTE_RIGHT_ANGLE, beg_attach, end_attach);
        rewrite_route_rows(&mut conn, &route_rows(beg_attach, &pts));
        let beg_t = GlueTarget {
            shape_id: sid,
            kind: GlueKind::Dynamic,
            attach: beg_attach,
        };
        let end_t = GlueTarget {
            shape_id: dup_id,
            kind: GlueKind::Dynamic,
            attach: end_attach,
        };
        bake_end(
            &mut conn,
            End::Begin,
            beg_attach,
            &walkglue_formula(End::Begin),
        );
        bake_trigger(&mut conn, End::Begin, sid);
        bake_end(&mut conn, End::End, end_attach, &walkglue_formula(End::End));
        bake_trigger(&mut conn, End::End, dup_id);
        let cmds: Vec<Box<dyn dc_core::Command>> = vec![
            Box::new(dc_core::commands::DuplicateShape::new(page_id, sid, offset)),
            Box::new(dc_core::AddShape {
                page: page_id,
                parent: None,
                shape: conn,
            }),
            Box::new(dc_core::AddConnect {
                owner: dc_core::ConnectsOwner::Page(page_id),
                connect: glue_connect(conn_id, End::Begin, &beg_t),
            }),
            Box::new(dc_core::AddConnect {
                owner: dc_core::ConnectsOwner::Page(page_id),
                connect: glue_connect(conn_id, End::End, &end_t),
            }),
        ];
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_autoconnect(sid),
            cmds,
        ));
        self.set_selection(Some((page_id, dup_id)));
        self.sel_set = vec![(page_id, dup_id)];
        self.status = crate::i18n::strs().msg_shape_dup_id_connected_to_sid_au(dup_id, sid);
    }
}

impl App {
    /// #108: the Connection Point tool's click — near an existing
    /// point of the hit shape selects it (a second click elsewhere
    /// re-selects), on the body adds a fresh row at the click, both in
    /// the shape's local frame.
    pub(super) fn handle_conn_point_click(&mut self, page_pos: (f64, f64)) {
        let Some(hit) = self.shape_hit(page_pos) else {
            self.connpt_sel = None;
            self.status = crate::i18n::strs().msg_click_a_shape_to_add_a_connectio();
            return;
        };
        let Some(page_id) = self.page_id() else {
            return;
        };
        // Existing (effective) points as page positions — the same
        // resolution the glue engine uses.
        let near = SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64;
        if let Some(cand) = self
            .glue_candidates(&[])
            .iter()
            .find(|c| c.id == hit && !c.conn_rows.is_empty())
        {
            if let Some((ix, _)) = cand
                .conn_rows
                .iter()
                .min_by_key(|(_, p)| ((p.0 - page_pos.0).hypot(p.1 - page_pos.1) * 1e6) as i64)
                .filter(|(_, p)| (p.0 - page_pos.0).hypot(p.1 - page_pos.1) <= near)
            {
                self.connpt_sel = Some((hit, *ix));
                self.status = crate::i18n::strs().msg_connection_point_ix_selected_del(ix);
                return;
            }
        }
        // Add: the local frame of the (top-level) hit shape — the
        // glue engine's rect convention (Pin − LocPin origin).
        let Some(ed) = self.ed() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, hit) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, hit) else {
            return;
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let (Some(px), Some(py), Some(lx), Some(ly)) =
            (get("PinX"), get("PinY"), get("LocPinX"), get("LocPinY"))
        else {
            return;
        };
        let local = (page_pos.0 - (px - lx), page_pos.1 - (py - ly));
        // The next row IX: max across the EFFECTIVE section (a
        // collision would shadow a master row).
        let eff = ed.doc.effective_sheet(sh, &chain);
        let next_ix = eff
            .sections_named("Connection")
            .first()
            .map(|sec| sec.rows.iter().map(|r| r.ix).max().unwrap_or(1) + 1)
            .unwrap_or(2);
        let scope = Scope::Shape {
            page: page_id,
            shape: hit,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        let cmds: Vec<Box<dyn dc_core::Command>> = vec![
            Box::new(dc_core::SetRowCell::new(
                scope.clone(),
                "Connection",
                0,
                dc_core::commands::RowSel::Ix(next_ix),
                "X",
                cell(local.0),
            )),
            Box::new(dc_core::SetRowCell::new(
                scope,
                "Connection",
                0,
                dc_core::commands::RowSel::Ix(next_ix),
                "Y",
                cell(local.1),
            )),
        ];
        self.apply(Macro::new(
            crate::i18n::strs().msg_add_connection_point_next_ix_on(next_ix, hit),
            cmds,
        ));
        self.connpt_sel = Some((hit, next_ix));
        self.status = crate::i18n::strs().msg_connection_point_next_ix_added_t(next_ix, hit);
    }

    /// #108: Delete on the Connection Point tool's selection — the
    /// LOCAL row goes; a master-inherited point explains instead
    /// (Visio edits those on the master).
    pub(super) fn delete_selected_conn_point(&mut self) {
        let Some((shape, ix)) = self.connpt_sel else {
            return;
        };
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let local = ed
            .doc
            .pages
            .iter()
            .find(|p| p.id == page_id)
            .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, shape))
            .map(|sh| {
                sh.sheet
                    .sections_named("Connection")
                    .iter()
                    .any(|sec| sec.rows.iter().any(|r| r.ix == ix))
            })
            .unwrap_or(false);
        if !local {
            self.status = crate::i18n::strs().msg_connection_point_ix_is_inherited(ix);
            return;
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_remove_connection_point_ix(ix),
            vec![Box::new(dc_core::commands::RemoveSectionRow::new(
                Scope::Shape {
                    page: page_id,
                    shape,
                },
                "Connection",
                0,
                dc_core::commands::RowSel::Ix(ix),
            ))],
        ));
        self.connpt_sel = None;
        self.status = crate::i18n::strs().msg_connection_point_ix_removed(ix);
    }

    /// #108/URS-1114: set a connector's route style and re-route its
    /// geometry immediately — one undo step.
    pub(super) fn apply_connector_route_style(&mut self, page_id: i64, sid: i64, style: &str) {
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
            return;
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let (Some(bx), Some(by), Some(ex), Some(ey)) =
            (get("BeginX"), get("BeginY"), get("EndX"), get("EndY"))
        else {
            return;
        };
        let pts = crate::connectors::route(style, (bx, by), (ex, ey));
        let rows = crate::connectors::route_rows((bx, by), &pts);
        let scope = Scope::Shape {
            page: page_id,
            shape: sid,
        };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(SetCell::new(
            scope.clone(),
            "ShapeRouteStyle",
            vsdx::Cell::value(style),
        ))];
        for (i, (x, y)) in rows.iter().enumerate() {
            let ix = (i + 2) as i64;
            for (cell, v) in [("X", *x), ("Y", *y)] {
                cmds.push(Box::new(dc_core::SetRowCell::new(
                    scope.clone(),
                    "Geometry",
                    0,
                    dc_core::commands::RowSel::Ix(ix),
                    cell,
                    vsdx::Cell::value(format!("{v:.6}")),
                )));
            }
        }
        // Curved (#108): row 2 becomes a CubicBezTo whose A/B/C/D
        // controls sit at mid-x/the endpoints' heights as Width/Height
        // formulas (an S — chord-collinear controls would render
        // straight; a reroute's size rewrite re-tracks them
        // automatically); every other style keeps the rows plain
        // LineTo.
        let curved = style == crate::connectors::ROUTE_CURVED;
        for ix in 2i64..=4 {
            let t = if ix == 2 && curved {
                Some("CubicBezTo".to_string())
            } else {
                Some("LineTo".to_string())
            };
            cmds.push(Box::new(dc_core::commands::SetSectionRowType::new(
                scope.clone(),
                "Geometry",
                0,
                dc_core::commands::RowSel::Ix(ix),
                t,
            )));
        }
        // #303: a reconciled connector can carry MORE than the four
        // route rows (stored hops add rows) — the style rewrite covers
        // rows 2..=4 only, so any row beyond 4 is stale after the
        // change and is removed here, before the reconcile re-adds
        // what the new route actually needs. `sections_named` returns
        // IX-sorted sections, so `.first()` IS the IX-0 section the
        // rewrite targets (a #303-batch review worried about
        // sheet-child enumeration order; the sort in vsdx's model
        // makes that unreachable).
        if let Some(geo) = sh.sheet.sections_named("Geometry").first() {
            for ix in geo.rows.iter().map(|r| r.ix).collect::<Vec<_>>() {
                if ix > 4 {
                    cmds.push(Box::new(dc_core::commands::RemoveSectionRow::new(
                        scope.clone(),
                        "Geometry",
                        0,
                        dc_core::commands::RowSel::Ix(ix),
                    )));
                }
            }
        }
        if curved {
            for (name, f, v) in crate::connectors::curved_row_extras(ex - bx, ey - by) {
                cmds.push(Box::new(dc_core::SetRowCell::new(
                    scope.clone(),
                    "Geometry",
                    0,
                    dc_core::commands::RowSel::Ix(2),
                    name,
                    vsdx::Cell {
                        v: Some(format!("{v:.6}")),
                        f: Some(f),
                        ..vsdx::Cell::default()
                    },
                )));
            }
        }
        // #303: the new route may cross differently — the stored
        // jumps reconcile in the same undo step.
        if let Some(page_id) = self.page_id() {
            cmds.push(Box::new(crate::cmd::ReconcileLineJumps::new(page_id)));
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_set_connector_route_style(),
            cmds,
        ));
    }

    /// The plain endpoint-move command (the #59 derived-cell set) for
    /// `commit_connector_endpoint` — the same cells
    /// `apply_line_endpoint` writes, as one boxed command.
    pub(super) fn move_endpoint_cmd(
        &self,
        page_id: i64,
        sid: i64,
        end: crate::connectors::End,
        to: (f64, f64),
    ) -> Box<dyn dc_core::Command> {
        let Some(ed) = self.ed() else {
            unreachable!("caller checked the document")
        };
        let page = ed.doc.pages.iter().find(|p| p.id == page_id);
        let Some(sh) = page.and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, sid)) else {
            unreachable!("caller checked the shape")
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
            unreachable!("caller checked the chain")
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let (bx, by, ex, ey) = (
            get("BeginX").unwrap_or(0.0),
            get("BeginY").unwrap_or(0.0),
            get("EndX").unwrap_or(0.0),
            get("EndY").unwrap_or(0.0),
        );
        let (a, b) = match end {
            crate::connectors::End::Begin => (to, (ex, ey)),
            crate::connectors::End::End => ((bx, by), to),
        };
        let w = (a.0 - b.0).hypot(a.1 - b.1).max(1e-4);
        let angle = (b.1 - a.1).atan2(b.0 - a.0);
        let scope = Scope::Shape {
            page: page_id,
            shape: sid,
        };
        let cell = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        // NOTE: for the corpus-model connector (unrotated, signed
        // Width/Height) the derived set differs from the 1D master
        // model — Width/Height are the axis deltas and the pin the
        // midpoint.
        let cmds: Vec<Box<dyn dc_core::Command>> = [
            ("BeginX", a.0),
            ("BeginY", a.1),
            ("EndX", b.0),
            ("EndY", b.1),
            ("PinX", (a.0 + b.0) / 2.0),
            ("PinY", (a.1 + b.1) / 2.0),
            ("Width", b.0 - a.0),
            ("Height", b.1 - a.1),
            ("LocPinX", (b.0 - a.0) / 2.0),
            ("LocPinY", (b.1 - a.1) / 2.0),
        ]
        .into_iter()
        // Visio-authored connectors GUARD their derived cells
        // (GUARD(EndX-BeginX)…) — a literal write would refuse and
        // abort the whole Macro (CodeRabbit); only formula-free
        // derived cells write. And only cells the INSTANCE owns: an
        // inherited master literal (the Line-arc corpus Height=0)
        // stays inherited — materializing a signed-delta literal over
        // it corrupts the master link and the frame (#226 ownership;
        // the reroute that follows in the same Macro derives the
        // chord-model frame cells from the endpoints).
        // The endpoint cells go through SetGluedCell (the glue engine
        // owns their formulas).
        .filter(|(n, _)| {
            matches!(*n, "BeginX" | "BeginY" | "EndX" | "EndY")
                || (dc_core::commands::effective_formula(&ed.doc, sh, &chain, n).is_none()
                    && sh.sheet.cell(n).is_some())
        })
        .map(|(n, v)| {
            if matches!(n, "BeginX" | "BeginY" | "EndX" | "EndY") {
                // The ANCHORED end keeps its glue formula: a plain
                // Cell::value write replaces the cell wholesale and
                // un-glues it — the begin formula died on the first
                // endpoint drag (utilisation.vsdx, #226 evidence). The
                // glue engine updates the cache behind its formula;
                // the DRAGGED end writes a literal (a release makes it
                // plain, and a re-glue overwrites it with the glue
                // formula in the same Macro).
                let f = if (matches!(end, crate::connectors::End::End)
                    && matches!(n, "BeginX" | "BeginY"))
                    || (matches!(end, crate::connectors::End::Begin)
                        && matches!(n, "EndX" | "EndY"))
                {
                    sh.sheet.cell(n).and_then(|c| c.f.clone())
                } else {
                    None
                };
                Box::new(dc_core::SetGluedCell::new(
                    scope.clone(),
                    n,
                    vsdx::Cell {
                        v: Some(format!("{v:.6}")),
                        f,
                        ..vsdx::Cell::default()
                    },
                )) as Box<dyn dc_core::Command>
            } else {
                Box::new(SetCell::new(scope.clone(), n, cell(v))) as Box<dyn dc_core::Command>
            }
        })
        .collect();
        let _ = (w, angle);
        Box::new(Macro::new(
            crate::i18n::strs().msg_undo_move_endpoint(crate::connectors::endpoint_word(end)),
            cmds,
        ))
    }

    /// The page's glue candidates (#108): every non-connector shape's
    /// page rect with its connection-point rows (IX → page position —
    /// own rows first, the master's for instances, like #109's
    /// snap-target collection).
    /// #108 line jumps: every top-level connector's page polyline
    /// (unrotated, instance- or master-geometry through the hit
    /// frame) with its stroke, in z order — the input to
    /// [`crate::connectors::line_jumps`].
    pub(super) fn jump_lines(&self) -> Vec<crate::connectors::JumpLine> {
        let mut out = Vec::new();
        let Some(ed) = self.ed() else {
            return out;
        };
        let Some(page_id) = self.page_id() else {
            return out;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return out;
        };
        for (z, sh) in page.shapes.iter().enumerate() {
            let Ok(chain) = shape_chain(&ed.doc, page_id, sh.id) else {
                continue;
            };
            if !crate::connectors::is_connector_in(&ed.doc, &chain, sh) {
                continue;
            }
            // #187 slice 2: rotated connectors hop too — the polyline
            // source (HitFrame + geometry_hit_segments) maps the
            // rotated frame, and the overlay's arc normal is chosen
            // per jump in page space, so no Angle gate remains.
            let (root, msize) = sh
                .master
                .and_then(|mid| ed.doc.master(mid))
                .and_then(|m| m.shapes.first())
                .map(|r| {
                    let g = |n: &str| {
                        ed.doc
                            .resolve_cell(r, &[r], n)
                            .and_then(|c| c.numeric())
                            .unwrap_or(1.0)
                    };
                    (Some(r), (g("Width"), g("Height")))
                })
                .unwrap_or((None, (1.0, 1.0)));
            let frame = crate::connectors::hit_frame(&ed.doc, &chain, sh, msize);
            // The loop gate above guarantees a connector, so stored
            // jump arcs strip from the polyline (#294) — with the
            // page's own radius as the evidence.
            let segs = crate::connectors::geometry_hit_segments(
                sh,
                root,
                &frame,
                // jump_radius_in resolves through the Page, not its id.
                Some(
                    ed.doc
                        .pages
                        .iter()
                        .find(|p| p.id == page_id)
                        .map(|p| crate::connectors::jump_radii_in(&ed.doc, p))
                        .unwrap_or((0.0, 0.0)),
                ),
            );
            let mut pts = Vec::with_capacity(segs.len() + 1);
            if let Some((a, _)) = segs.first() {
                pts.push(*a);
            }
            for (_, b) in &segs {
                pts.push(*b);
            }
            let weight = ed
                .doc
                .resolve_cell(sh, &chain, "LineWeight")
                .and_then(|c| c.numeric())
                .unwrap_or(0.01);
            // #235 corollary (CodeRabbit, #278): an explicit-zero
            // LineWeight paints nothing — the export path
            // (`page_jumps`) drops such connectors from the crossing
            // set, and the canvas overlay must agree or a visible
            // connector hops over an invisible shaft on screen while
            // the export shows no hop.
            if weight == 0.0 {
                continue;
            }
            let color = ed
                .doc
                .resolve_cell(sh, &chain, "LineColor")
                .and_then(|c| c.v.clone())
                .and_then(|v| parse_hex_rgb(&v))
                .unwrap_or((0x42, 0x42, 0x42));
            out.push(crate::connectors::JumpLine {
                id: sh.id,
                z,
                weight,
                color,
                pts,
            });
        }
        out
    }

    pub(super) fn glue_candidates(&self, excluding: &[i64]) -> Vec<crate::connectors::GlueCandidate> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page_id) = self.page_id() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        page.shapes
            .iter()
            .filter(|s| {
                !excluding.contains(&s.id)
                    && !crate::connectors::is_connector(s)
                    && !crate::guides::is_guide(s)
            })
            .filter_map(|s| {
                let rect = self.shape_rect(&ed.doc, page_id, s)?;
                let mut c = crate::connectors::GlueCandidate {
                    id: s.id,
                    rect,
                    conn_rows: Vec::new(),
                };
                let collect =
                    |sheet: &vsdx::model::Sheet, c: &mut crate::connectors::GlueCandidate| {
                        for sec in sheet.sections_named("Connection") {
                            for row in &sec.rows {
                                if let (Some(x), Some(y)) = (
                                    row.cell("X").and_then(|v| v.numeric()),
                                    row.cell("Y").and_then(|v| v.numeric()),
                                ) {
                                    c.conn_rows.push((row.ix, (rect.0 + x, rect.1 + y)));
                                }
                            }
                        }
                    };
                collect(&s.sheet, &mut c);
                if c.conn_rows.is_empty() {
                    if let Some(root) = s
                        .master
                        .and_then(|mid| ed.doc.master(mid))
                        .and_then(|m| m.shapes.first())
                    {
                        collect(&root.sheet, &mut c);
                    }
                }
                Some(c)
            })
            .collect()
    }

    /// The Connector Tool's gesture finished (#108, URS-1067): create
    /// the connector between the two points, gluing each end per the
    /// #109 glue settings (connection point → static, shape →
    /// dynamic) — corpus formulas baked into the shape cells, Connect
    /// records in the same undo step.
    pub(super) fn create_connector(&mut self, from: (f64, f64), to: (f64, f64)) {
        use crate::connectors::{End, GlueKind};
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let _geom = self.page_geom();
        let cands = self.glue_candidates(&[]);
        let threshold = SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64;
        let decide = |p, toward| {
            crate::connectors::glue_decision(
                &cands,
                p,
                toward,
                threshold,
                self.snap.glue_active,
                self.snap.glue_connection_points,
            )
        };
        let beg_t = decide(from, to);
        let end_t = decide(to, from);
        let beg = beg_t.map(|t| t.attach).unwrap_or(from);
        let end = end_t.map(|t| t.attach).unwrap_or(to);
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let mut shape = crate::connectors::connector_shape(id, beg, end);
        // Route the fresh connector per its style before it lands.
        let style = shape
            .sheet
            .cell("ShapeRouteStyle")
            .and_then(|c| c.v.clone())
            .unwrap_or_else(|| crate::connectors::ROUTE_RIGHT_ANGLE.into());
        let pts = crate::connectors::route(&style, beg, end);
        let rows = crate::connectors::route_rows(beg, &pts);
        let _ = rows;
        // (the straight default rows the constructor wrote stand until
        // the first reroute; a right-angle start routes now)
        if style != crate::connectors::ROUTE_STRAIGHT {
            crate::connectors::rewrite_route_rows(&mut shape, &rows);
        }
        // Bake the glue: endpoint formulas + triggers into the cells.
        for (end, t) in [(End::Begin, beg_t), (End::End, end_t)] {
            match t.map(|t| (t.shape_id, t.kind, t.attach)) {
                Some((tid, GlueKind::Static { row_ix }, attach)) => {
                    let f = crate::connectors::static_glue_formula(tid, row_ix);
                    crate::connectors::bake_end(&mut shape, end, attach, &f);
                }
                Some((tid, GlueKind::Dynamic, attach)) => {
                    let f = crate::connectors::walkglue_formula(end);
                    crate::connectors::bake_end(&mut shape, end, attach, &f);
                    crate::connectors::bake_trigger(&mut shape, end, tid);
                }
                None => {}
            }
        }
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(dc_core::AddShape {
            page: page_id,
            parent: None,
            shape,
        })];
        for (end, t) in [(End::Begin, beg_t), (End::End, end_t)] {
            if let Some(t) = t {
                cmds.push(Box::new(dc_core::AddConnect {
                    owner: dc_core::ConnectsOwner::Page(page_id),
                    connect: crate::connectors::glue_connect(id, end, &t),
                }));
            }
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_add_connector(id),
            cmds,
        ));
        self.set_selection(Some((page_id, id)));
        self.sel_set = vec![(page_id, id)];
        self.status = match (beg_t, end_t) {
            (Some(_), Some(_)) => crate::i18n::strs().msg_connector_glued_at_both_ends(),
            (Some(_), None) | (None, Some(_)) => {
                crate::i18n::strs().msg_connector_glued_at_one_end()
            }
            (None, None) => crate::i18n::strs().msg_connector_drawn_free_ends(),
        };
    }

    /// #132 (URS-1225): a sampled pen stroke becomes a new shape —
    /// [`crate::freeform`] fits LineTo/ArcTo rows into the bbox
    /// frame; ordinary editable geometry through the add-shape path,
    /// one undo step, selected on landing.
    pub(super) fn create_freeform(&mut self, stroke: Vec<(f64, f64)>) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else {
            return;
        };
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let Some(shape) = crate::freeform::build_shape(id, &stroke) else {
            self.status = crate::i18n::strs().msg_freeform_stroke_too_short_to_dra();
            return;
        };
        let segs = shape
            .sheet
            .sections_named("Geometry")
            .first()
            .map(|g| g.rows.len().saturating_sub(1))
            .unwrap_or(0);
        self.apply(Macro::new(
            crate::i18n::strs().msg_freeform_shape_id(id),
            vec![Box::new(dc_core::AddShape {
                page: page_id,
                parent: None,
                shape,
            })],
        ));
        self.set_selection(Some((page_id, id)));
        self.sel_set = vec![(page_id, id)];
        self.status = crate::i18n::strs().msg_freeform_shape_created_segs_segm(segs);
    }

    /// #108 reroute: a deferred [`crate::cmd::RerouteConnectors`]
    /// riding at the tail of the move/resize Macro — it evaluates
    /// against the post-move document (an eager build would read
    /// stale, pre-move geometry). `None` when nothing glues to the
    /// moved shapes, so Macros stay clean of no-ops.
    pub(super) fn reroute_cmd(&self, page_id: i64, moved: &[i64]) -> Option<Box<dyn dc_core::Command>> {
        let ed = self.ed()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let touches = page
            .connects()
            .list
            .iter()
            .any(|c| moved.contains(&c.to_sheet));
        touches.then(|| {
            Box::new(crate::cmd::RerouteConnectors::new(page_id, moved.to_vec()))
                as Box<dyn dc_core::Command>
        })
    }
}

impl App {
    /// #109: snap a single dragged point — a 1D endpoint under the
    /// glue settings, a geometry vertex under the snap settings —
    /// against the page minus the dragged shape.
    pub(super) fn snap_point_for(
        &self,
        dragged: i64,
        p: (f64, f64),
        glue: bool,
    ) -> Option<((f64, f64), crate::snap::SnapHit)> {
        let page_id = self.page_id()?;
        let targets = self.snap_targets(page_id, &[dragged]);
        let guide_list = self.snap_guides(&[dragged]);
        let ctx = self.snap_ctx_of(&targets, &guide_list);
        crate::snap::point_snap(p, &ctx, &self.snap, glue)
    }

    /// #202: the CONNECTOR-ENDPOINT snap — connection points and
    /// guides only. The shape-handle/vertex magnets made drops near a
    /// shape's edge snap onto the edge and re-glue there (the
    /// endpoints felt hard-glued and afterwards slid only along the
    /// shape's edges); Visio's endpoint drag magnetizes to connection
    /// points, and a drop off-shape stays loose.
    /// #226: the live endpoint-drag ghost — the dragged connector's
    /// ink re-rendered through [`plan_connector_frame`] (the SAME
    /// planner the release commits) at the snapped pointer, so what
    /// you drag is exactly what lands. Re-rasters only when the
    /// snapped point moves; motionless frames reuse the texture.
    pub(super) fn endpoint_preview_quad(
        &mut self,
        ctx: &egui::Context,
        page_id: i64,
        sid: i64,
        hid: HandleId,
        snap: crate::canvas::SnapEndpointFn,
        ptr_page: (f64, f64),
    ) -> Option<crate::layers::ShapeQuad> {
        // (Re-)arm the snapshot: the doc at gesture start, the #175
        // row-translation anchor (the pre-drag END), no texture yet.
        let needs_arm = self
            .endpoint_preview
            .as_ref()
            .map(|p| p.conn_id != sid)
            .unwrap_or(true);
        if needs_arm {
            let ed = self.ed.as_ref()?;
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
            let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
            let g = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
            let pre_end = (g("EndX")?, g("EndY")?);
            let pre_beg = (g("BeginX")?, g("BeginY")?);
            // The dragged end's own origin at gesture start — the
            // commit path's drag anchor (HandleId::LineBegin drags the
            // Begin pair).
            let origin = match hid {
                HandleId::LineEnd => pre_end,
                _ => pre_beg,
            };
            self.endpoint_preview = Some(EndpointPreview {
                doc: ed.doc.clone(),
                conn_id: sid,
                pre_end,
                pre_beg,
                grab_off: (origin.0 - ptr_page.0, origin.1 - ptr_page.1),
                last_to: (f64::NAN, f64::NAN),
                tex: None,
                bbox: (0.0, 0.0, 0.0, 0.0),
            });
        }
        // The preview endpoint: the pointer PLUS the arm-time offset —
        // the same place the commit's delta-from-origin lands — then
        // the endpoint snap, exactly as the release path snaps.
        let to = {
            let ep = self.endpoint_preview.as_ref()?;
            snap(
                true,
                (ptr_page.0 + ep.grab_off.0, ptr_page.1 + ep.grab_off.1),
            )
        };
        // The endpoint pair in DOCUMENT order: the dragged end moves
        // to `to`, the anchored end stays at its resolved position (a
        // glued anchor's cache already names its attach point).
        let (beg, endp) = {
            let ed = self.ed.as_ref()?;
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
            let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
            let g = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
            match hid {
                HandleId::LineEnd => ((g("BeginX")?, g("BeginY")?), to),
                _ => (to, (g("EndX")?, g("EndY")?)),
            }
        };
        let ep = self.endpoint_preview.as_mut()?;
        if ep.last_to == to && ep.tex.is_some() {
            let tex = ep.tex.as_ref()?;
            return Some(crate::layers::ShapeQuad {
                id: sid,
                tex: tex.id(),
                bbox: ep.bbox,
            });
        }
        // A fresh connector into the snapshot each frame: the plan's
        // frame cells are absolute but its #175 row translations are
        // relative to the PRE-drag pose — re-planning over an
        // already-translated clone would accumulate the delta.
        let fresh = {
            let ed = self.ed.as_ref()?;
            let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
            vsdx::model::find_shape_by_id(&page.shapes, sid)?.clone()
        };
        let plan = {
            let page2 = ep.doc.pages.iter_mut().find(|p| p.id == page_id)?;
            let slot = vsdx::model::find_shape_by_id_mut(&mut page2.shapes, sid)?;
            *slot = fresh;
            let page2 = ep.doc.pages.iter().find(|p| p.id == page_id)?;
            let conn = vsdx::model::find_shape_by_id(&page2.shapes, sid)?;
            let chain2 = shape_chain(&ep.doc, page_id, sid).ok()?;
            crate::cmd::plan_connector_frame(
                &ep.doc,
                conn,
                &chain2,
                beg,
                endp,
                (ep.pre_beg, ep.pre_end),
                dc::geom::jump_radii_in(&ep.doc, page2),
            )
        };
        // Apply the plan's writes to the snapshot (plain cache writes
        // — the renderer reads values, not formulas).
        {
            let page2 = ep.doc.pages.iter_mut().find(|p| p.id == page_id)?;
            let conn = vsdx::model::find_shape_by_id_mut(&mut page2.shapes, sid)?;
            for (n, v, _) in &plan.cells {
                conn.sheet.set_cell(n, vsdx::Cell::value(format!("{v:.6}")));
            }
            for (sec_ix, row_ix, name, cell) in &plan.row_cells {
                let _ = dc_core::commands::set_row_cell(
                    &mut conn.sheet,
                    "Geometry",
                    *sec_ix,
                    &dc_core::commands::RowSel::Ix(*row_ix),
                    name,
                    cell.clone(),
                );
            }
            // #303: the grow path replaces the route rows wholesale —
            // mirror the commit's SetSectionRows directly on the
            // snapshot; the per-row path keeps its cell writes.
            if let Some((sec_ix, rep)) = &plan.rows_replace {
                if let Some(sec) = conn.sheet.children.iter_mut().find_map(|c| match c {
                    vsdx::model::SheetChild::Section(sec)
                        if sec.name == "Geometry" && sec.ix == *sec_ix =>
                    {
                        Some(sec)
                    }
                    _ => None,
                }) {
                    sec.rows = rep.clone();
                }
            }
            for (ix, x, y) in &plan.rows {
                for (name, v) in [("X", *x), ("Y", *y)] {
                    let _ = dc_core::commands::set_row_cell(
                        &mut conn.sheet,
                        "Geometry",
                        0,
                        &dc_core::commands::RowSel::Ix(*ix),
                        name,
                        vsdx::Cell::value(format!("{v:.6}")),
                    );
                }
            }
        }
        // Raster the snapshot's connector through the page renderer —
        // the full styled ink (dash, arrow, weight, color), the same
        // pipeline the committed layer lands through.
        let page2 = ep.doc.pages.iter().find(|p| p.id == page_id)?;
        let bounds = dc::svg::shape_ink_bounds(&ep.doc, page2, sid)?;
        let svg = dc::svg::render_shape_layer(&ep.doc, page2, sid, &[], bounds).ok()?;
        let w_px = (bounds.2 * f64::from(self.view.px_per_in())).clamp(16.0, 2048.0);
        let tree = crate::svgimg::parse_svg(&svg, crate::perf::Kind::Page).ok()?;
        let img = crate::svgimg::render_tree(&tree, w_px as f32, crate::perf::Kind::Page).ok()?;
        ep.tex = Some(ctx.load_texture("endpoint-drag-preview", img, egui::TextureOptions::LINEAR));
        ep.bbox = bounds;
        ep.last_to = to;
        let tex = ep.tex.as_ref()?;
        Some(crate::layers::ShapeQuad {
            id: sid,
            tex: tex.id(),
            bbox: ep.bbox,
        })
    }

    pub(super) fn snap_endpoint_for(&self, dragged: i64, p: (f64, f64)) -> (f64, f64) {
        let page_id = self.page_id().unwrap_or(0);
        let targets = self.snap_targets(page_id, &[dragged]);
        let guide_list = self.snap_guides(&[dragged]);
        let ctx = self.snap_ctx_of(&targets, &guide_list);
        let mut s = self.snap;
        s.glue_shape_handles = false;
        s.glue_shape_vertices = false;
        crate::snap::point_snap(p, &ctx, &s, true).map_or(p, |(q, _)| q)
    }

    /// #160: per-axis corner snap for a resize drag — the SNAP
    /// settings (resize edges are snap targets, not glue targets),
    /// against the page minus the resized shape.
    pub(super) fn snap_corner_for(&self, dragged: i64, p: (f64, f64)) -> Option<(f64, f64)> {
        let page_id = self.page_id()?;
        let targets = self.snap_targets(page_id, &[dragged]);
        let guide_list = self.snap_guides(&[dragged]);
        let ctx = self.snap_ctx_of(&targets, &guide_list);
        crate::snap::corner_snap(p, &ctx, &self.snap)
    }
}

impl App {
    /// #113 (URS-1122): write the page's size (page inches, portrait
    /// orientation of the pair as given) as one undo Macro, then
    /// refit the view to the new page.
    /// #251: the page's Show-Line-Jumps toggle (Page Setup ▸ Layout
    /// and Routing) — the PageSheet's `LineJumpCode` (the
    /// PageLayout-section cell): 1 = horizontal lines jump (Visio's
    /// authored default), 6 = "no line gets jumps" (the off switch).
    /// One undoable command.
    pub(super) fn apply_line_jumps_shown(&mut self, show: bool) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        // #301: hiding jumps straightens the stored arcs in the same
        // step — the render already draws them straight, and the file
        // should agree.
        let cmds: Vec<Box<dyn dc_core::Command>> = vec![
            Box::new(SetCell::new(
                Scope::PageSheet { page: page_id },
                "LineJumpCode",
                // Off writes 0 (visPLOJumpNone, [MS-Learn]) — 6 was
                // never in the enumeration (#218 review).
                vsdx::Cell::value(if show { "1" } else { "0" }),
            )),
            Box::new(crate::cmd::ReconcileLineJumps::new(page_id)),
        ];
        self.apply(dc_core::Macro::new("set line jumps shown", cmds));
        self.status = if show {
            crate::i18n::strs().msg_line_jumps_shown()
        } else {
            crate::i18n::strs().msg_line_jumps_hidden()
        };
    }

    /// #251: the page's jump-size multiplier (`LineJumpFactorX`).
    /// #301: the stored jumps normalize with the edit — recognized at
    /// the PRE-edit radius (they were authored there) and rewritten
    /// at the new one, so the stored rows and the overlay never
    /// disagree after a factor change.
    pub(super) fn apply_line_jump_factor(&mut self, factor: f64) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let pre = self.ed.as_ref().and_then(|ed| {
            ed.doc
                .pages
                .iter()
                .find(|p| p.id == page_id)
                .map(|p| dc::geom::jump_radii_in(&ed.doc, p))
        });
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(SetCell::new(
            Scope::PageSheet { page: page_id },
            "LineJumpFactorX",
            vsdx::Cell::value(format!("{factor:.6}")),
        ))];
        if let Some(pre) = pre {
            cmds.push(Box::new(crate::cmd::ReconcileLineJumps::with_strip(page_id, pre)));
        }
        self.apply(dc_core::Macro::new("set line jump factor", cmds));
        // The factor steps in fractions (0.5 .. 3.0) — the i64 cast
        // truncated them to ×0/×1 in the status note (PR #269
        // review); FluentNumber carries the decimal through.
        self.status = crate::i18n::strs().msg_line_jump_factor_set(factor);
    }
}
