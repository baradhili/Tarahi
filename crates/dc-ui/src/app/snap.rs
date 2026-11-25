//! Snapping: settings, target collection, drag-snap application,
//! glue-release cleanup, and the guide set.

use super::*;

impl App {
    /// Reload the snap & glue toggles from the document (open / new /
    /// after any model-level settings change).
    pub(super) fn reload_snap_settings(&mut self) {
        self.snap = self
            .ed
            .as_ref()
            .and_then(|e| e.doc.doc_snap_glue())
            .map(crate::snap::SnapSettings::from_doc)
            .unwrap_or_default();
    }

    /// Write the current toggles into the document. Settings are
    /// document state, not drawing edits, so they never enter the
    /// undo history (Visio's behavior — Snap & Glue has no undo).
    pub(super) fn apply_snap_settings(&mut self) {
        if let Some(ed) = self.ed.as_mut() {
            ed.doc.set_doc_snap_glue(self.snap.to_doc());
            self.dirty = true;
        }
    }

    /// The snap-side description of every top-level shape except
    /// `excluded` (the dragged shape, its co-selected group, and any
    /// container members riding along): alignment box, own geometry
    /// vertices, and connection points — the shape's own rows first,
    /// the master's for instances (Visio stencils define them on the
    /// master, like the #137 geometry inheritance note).
    pub(super) fn snap_targets(&self, page_id: i64, excluded: &[i64]) -> Vec<crate::snap::SnapTarget> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        page.shapes
            .iter()
            .filter(|s| !excluded.contains(&s.id))
            .filter_map(|s| {
                let rect = self.shape_rect(&ed.doc, page_id, s)?;
                let mut t = crate::snap::SnapTarget {
                    id: s.id,
                    rect,
                    ..Default::default()
                };
                // Own geometry vertices (#137's mapping: shape-local
                // y-up onto the chain-mapped bbox origin).
                for geo in s.sheet.sections_named("Geometry") {
                    for row in &geo.rows {
                        let point_row = matches!(
                            row.t.as_deref().unwrap_or(""),
                            "MoveTo"
                                | "LineTo"
                                | "ArcTo"
                                | "EllipticalArcTo"
                                | "NURBSTo"
                                | "SplineKnot"
                        );
                        if !point_row {
                            continue;
                        }
                        if let (Some(x), Some(y)) = (
                            row.cell("X").and_then(|c| c.numeric()),
                            row.cell("Y").and_then(|c| c.numeric()),
                        ) {
                            t.vertices.push((rect.0 + x, rect.1 + y));
                        }
                    }
                }
                collect_connection_points(&mut t, &s.sheet);
                if t.connection_points.is_empty() {
                    if let Some(mid) = s.master {
                        if let Some(root) = ed.doc.master(mid).and_then(|m| m.shapes.first()) {
                            collect_connection_points(&mut t, &root.sheet);
                        }
                    }
                }
                Some(t)
            })
            .collect()
    }

    /// A snap context over prebuilt targets and guides (#113 now
    /// feeds them): the displayed grid and ruler-subdivision
    /// spacings, and the Visio-style screen snap strength converted
    /// through the zoom.
    pub(super) fn snap_ctx_of<'t>(
        &self,
        targets: &'t [crate::snap::SnapTarget],
        guides: &'t [crate::snap::Guide],
    ) -> crate::snap::SnapCtx<'t> {
        crate::snap::SnapCtx {
            targets,
            guides,
            grid_step_in: canvas::grid_step_in(&self.view),
            ruler_step_in: canvas::ruler_interval_in(&self.view),
            ruler_mirror_x_in: crate::i18n::rtl().then(|| self.page_geom().w_in),
            threshold_in: SNAP_STRENGTH_PX as f64 / self.view.px_per_in() as f64,
        }
    }

    /// Snap the live drag position (#109) and refresh the dynamic-grid
    /// hints. The model rect is still the press-time rect during the
    /// gesture, so the live box is the rect translated by the pin
    /// delta; the returned pin carries the snap translation, and the
    /// commit path (move_cmd_for) lands snapped by construction.
    pub(super) fn apply_drag_snap(&mut self, d: &DragState, pin: (f64, f64)) -> (f64, f64) {
        self.snap_live.clear();
        let Some(page_id) = self.page_id() else {
            return pin;
        };
        let Some(ed) = self.ed.as_ref() else {
            return pin;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return pin;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, d.shape) else {
            return pin;
        };
        let Some((x, y, w, h)) = self.shape_rect(&ed.doc, page_id, sh) else {
            return pin;
        };
        let mut excluded: Vec<i64> = Vec::with_capacity(d.group.len() + 3);
        excluded.push(d.shape);
        excluded.extend(d.group.iter().copied());
        excluded.extend(self.container_member_ids(page_id, d.shape));
        let targets = self.snap_targets(page_id, &excluded);
        let guide_list = self.snap_guides(&excluded);
        let ctx = self.snap_ctx_of(&targets, &guide_list);
        let live = (x + pin.0 - d.start_pin.0, y + pin.1 - d.start_pin.1, w, h);
        let res = crate::snap::snap_translation(live, &ctx, &self.snap);
        if self.snap.dynamic_grid {
            let snapped = (live.0 + res.dx, live.1 + res.dy, w, h);
            self.snap_live = crate::snap::dynamic_hints(snapped, &ctx);
        }
        // #113: remember which guide an axis snapped onto — the move
        // commit turns it into Visio's pin glue formula when
        // glue-to-guides is enabled.
        self.drag_glue = None;
        if self.snap.glue_active && self.snap.glue_guides {
            use crate::snap::SnapHit;
            let guide_of = |h: &Option<(f64, SnapHit)>| match h {
                Some((_, SnapHit::Guide(id))) => Some(*id),
                _ => None,
            };
            let (gx, gy) = (
                guide_of(&res.x_hit).is_some(),
                guide_of(&res.y_hit).is_some(),
            );
            if let Some(id) = guide_of(&res.x_hit).or(guide_of(&res.y_hit)) {
                self.drag_glue = Some((id, gx, gy));
            }
        }
        (pin.0 + res.dx, pin.1 + res.dy)
    }

    /// #108: commands removing every `<Connect>` that references a
    /// deleted shape (either side). Indices are collected descending —
    /// sequential `RemoveConnect`s inside one Macro shift the list.
    pub(super) fn connect_cleanup_cmds(
        &self,
        page_id: i64,
        deleted: &[i64],
    ) -> Vec<Box<dyn dc_core::Command>> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        let connects = page.connects();
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for idx in (0..connects.list.len()).rev() {
            let c = &connects.list[idx];
            if deleted.contains(&c.from_sheet) || deleted.contains(&c.to_sheet) {
                cmds.push(Box::new(dc_core::RemoveConnect::new(
                    dc_core::ConnectsOwner::Page(page_id),
                    idx,
                )));
            }
        }
        cmds
    }

    /// #113: commands that release every shape glued to `shape_id`
    /// (no-ops for non-guides): each dependent's pin formula
    /// referencing the guide is replaced by its cached value, so the
    /// shape keeps its position when the guide goes — Visio's
    /// release-on-delete.
    pub(super) fn guide_glue_release_cmds(
        &self,
        page_id: i64,
        shape_id: i64,
    ) -> Vec<Box<dyn dc_core::Command>> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return Vec::new();
        };
        if !crate::guides::is_guide(sh) {
            return Vec::new();
        }
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (sid, axes) in crate::guides::glue_dependents(&page.shapes, shape_id) {
            let Some(dep) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
                continue;
            };
            for (cell, _) in axes {
                let v = dep.sheet.cell(cell).and_then(|c| c.numeric());
                if let Some(v) = v {
                    cmds.push(Box::new(SetCell::new(
                        Scope::Shape {
                            page: page_id,
                            shape: sid,
                        },
                        cell,
                        vsdx::Cell::value(format!("{v:.6}")),
                    )));
                }
            }
        }
        cmds
    }

    // -- connector authoring (GitHub #108) -------------------------------
}

impl App {
    /// #113: a ruler drag released over the page becomes a guide
    /// (URS-1076): one undoable AddShape carrying the synthetic
    /// clean-room guide cells, then selected like any placement.
    pub(super) fn add_guide(&mut self, src: canvas::GuideSource, at: (f64, f64)) {
        use crate::guides::GuideKind;
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let geom = self.page_geom();
        let kind = match src {
            canvas::GuideSource::Vertical => GuideKind::Vertical,
            canvas::GuideSource::Horizontal => GuideKind::Horizontal,
            canvas::GuideSource::Point => GuideKind::Point,
        };
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let shape = crate::guides::guide_shape(id, kind, at, (geom.w_in, geom.h_in));
        let label = match kind {
            GuideKind::Vertical => "add vertical guide",
            GuideKind::Horizontal => "add horizontal guide",
            GuideKind::Point => "add guide point",
        };
        self.apply(dc_core::AddShape {
            page: page_id,
            parent: None,
            shape,
        });
        self.set_selection(Some((page_id, id)));
        self.sel_set = vec![(page_id, id)];
        self.status = match kind {
            GuideKind::Vertical => {
                crate::i18n::strs().msg_guide_added_vertical(self.view.unit.fmt(at.0))
            }
            GuideKind::Horizontal => {
                crate::i18n::strs().msg_guide_added_horizontal(self.view.unit.fmt(at.1))
            }
            GuideKind::Point => {
                crate::i18n::strs().msg_guide_added_point(self.view.unit.fmt_pt(at.0, at.1))
            }
        };
        let _ = label;
    }

    /// #113: commit a guide drag — the guide moves through the normal
    /// 1D/pin move path, and every shape glued to it (pin formulas
    /// naming this guide) follows: cached values rewritten in the
    /// same undo step, formulas kept for the file's sake (a real
    /// Visio re-evaluates them live).
    pub(super) fn apply_guide_move(&mut self, page_id: i64, guide_id: i64, to_pin: (f64, f64)) {
        let mut cmds: Vec<Box<dyn dc_core::Command>> =
            vec![self.move_cmd_for(page_id, guide_id, to_pin)];
        if let Some(ed) = self.ed() {
            let page = ed.doc.pages.iter().find(|p| p.id == page_id);
            if let Some(page) = page {
                for (sid, axes) in crate::guides::glue_dependents(&page.shapes, guide_id) {
                    for (cell, guide_axis) in axes {
                        let v = if guide_axis == "PinX" {
                            to_pin.0
                        } else {
                            to_pin.1
                        };
                        cmds.push(Box::new(SetCell::new(
                            Scope::Shape {
                                page: page_id,
                                shape: sid,
                            },
                            cell,
                            vsdx::Cell {
                                v: Some(format!("{v:.6}")),
                                f: Some(crate::guides::glue_formula(guide_id, guide_axis)),
                                ..vsdx::Cell::default()
                            },
                        )));
                    }
                }
            }
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_move_guide(guide_id),
            cmds,
        ));
    }
}

impl App {
    /// The page's guides as UI info (#113), resolved through the
    /// inheritance chain.
    pub(super) fn page_guides(&self) -> Vec<crate::guides::GuideInfo> {
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
            .filter(|s| crate::guides::is_guide(s))
            .filter_map(|s| {
                let Ok(chain) = shape_chain(&ed.doc, page_id, s.id) else {
                    return None;
                };
                let get = |n: &str| ed.doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
                crate::guides::guide_info(s.id, s, get)
            })
            .collect()
    }

    /// The guide list as engine targets (#113 → #109's Guide seam),
    /// minus `excluded` (the dragged shape's set — a guide being
    /// dragged never targets itself).
    pub(super) fn snap_guides(&self, excluded: &[i64]) -> Vec<crate::snap::Guide> {
        self.page_guides()
            .iter()
            .filter(|g| !excluded.contains(&g.id))
            .map(|g| match g.kind {
                crate::guides::GuideKind::Vertical => crate::snap::Guide::Line {
                    id: g.id,
                    horizontal: false,
                    at: g.x,
                },
                crate::guides::GuideKind::Horizontal => crate::snap::Guide::Line {
                    id: g.id,
                    horizontal: true,
                    at: g.y,
                },
                crate::guides::GuideKind::Point => crate::snap::Guide::Point {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                },
            })
            .collect()
    }
}
