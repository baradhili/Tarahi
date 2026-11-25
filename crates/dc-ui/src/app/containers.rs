//! Containers and groups: membership, wrapping and unwrapping, the
//! style gallery, group/ungroup/disband, and drag nest targets.

use super::*;

impl App {
    /// Apply a pre-boxed command (the multi-shape move paths build
    /// commands per member); wrapped in a one-step Macro so the status
    /// and history behave like every other apply.
    /// Shapes whose `User.msvSDContainerShape` names `container_id`
    /// (Visio's membership convention, #27).
    pub(super) fn container_member_ids(&self, page_id: i64, container_id: i64) -> Vec<i64> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        page.shapes
            .iter()
            .filter(|s| {
                s.sheet.sections_named("User").iter().any(|sec| {
                    sec.named_row("msvSDContainerShape")
                        .and_then(|r| r.cell("Value"))
                        .and_then(|c| c.numeric())
                        == Some(container_id as f64)
                })
            })
            .map(|s| s.id)
            .collect()
    }

    /// #75: the topmost container under the selection's centroid —
    /// a candidate is a top-level shape that is the target of at
    /// least one member-side `User.msvSDContainerShape` reference or
    /// was created by the Add-to-New flow (its `Container.` name);
    /// the selection's own shapes never qualify. Z-order decides
    /// between overlapping containers (topmost wins).
    pub(super) fn underlying_container(&self, members: &[i64]) -> Option<i64> {
        let ed = self.ed()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let page_id = page.id;
        // The selection's centroid (page inches, y-up).
        let mut cx = 0.0;
        let mut cy = 0.0;
        let mut n = 0.0;
        for &sid in members {
            let sh = page.shapes.iter().find(|s| s.id == sid)?;
            let (x, y, w, h) = self.shape_rect(&ed.doc, page_id, sh)?;
            cx += x + w / 2.0;
            cy += y + h / 2.0;
            n += 1.0;
        }
        if n == 0.0 {
            return None;
        }
        let centroid = (cx / n, cy / n);
        let is_container = |s: &Shape, referenced: bool| {
            referenced
                || s.name_u
                    .as_deref()
                    .is_some_and(|n| n.starts_with("Container."))
        };
        // Which ids some member references (container identity by
        // membership convention).
        let referenced: std::collections::HashSet<i64> = page
            .shapes
            .iter()
            .flat_map(|s| s.sheet.sections_named("User"))
            .filter_map(|sec| {
                sec.named_row("msvSDContainerShape")
                    .and_then(|r| r.cell("Value"))
                    .and_then(|c| c.numeric())
                    .map(|v| v as i64)
            })
            .collect();
        let mut found: Option<i64> = None;
        for s in &page.shapes {
            if members.contains(&s.id) {
                continue;
            }
            if !is_container(s, referenced.contains(&s.id)) {
                continue;
            }
            let Some((x, y, w, h)) = self.shape_rect(&ed.doc, page_id, s) else {
                continue;
            };
            if centroid.0 >= x && centroid.0 <= x + w && centroid.1 >= y && centroid.1 <= y + h {
                found = Some(s.id); // later z-order wins (topmost)
            }
        }
        found
    }

    /// #75: Add to Underlying Container — write the membership row on
    /// each selected shape (the #27 convention, materialized on first
    /// edit), one Macro, one undo.
    pub(super) fn add_selection_to_underlying_container(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let members: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, s)| *s)
            .collect();
        let Some(container) = self.underlying_container(&members) else {
            return;
        };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = members
            .iter()
            .map(|&m| {
                Box::new(dc_core::SetRowCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: m,
                    },
                    "User",
                    0,
                    dc_core::commands::RowSel::Named("msvSDContainerShape".into()),
                    "Value",
                    vsdx::Cell::value(container.to_string()),
                )) as Box<dyn dc_core::Command>
            })
            .collect();
        // #138 §2: the heading band's member count re-derives from live
        // membership — "{base title} ({count})" through SetText.
        if let Some(ed) = self.ed() {
            let title = ed
                .doc
                .pages
                .iter()
                .find(|p| p.id == page_id)
                .and_then(|p| p.shapes.iter().find(|s| s.id == container))
                .and_then(|cont| {
                    // #191: master-based containers keep the title row on
                    // the master root — resolve through the chain.
                    ed.doc
                        .effective_user_row(cont, &[], "msvSDContainerTitle")
                        .and_then(|r| r.cell("Value"))
                        .and_then(|c| c.v.clone())
                })
                .unwrap_or_else(|| crate::i18n::strs().msg_container());
            // The UNION — already-members selected again must not
            // double-count (#75 branch review).
            let mut all = self.container_member_ids(page_id, container);
            for m in &members {
                if !all.contains(m) {
                    all.push(*m);
                }
            }
            let count = all.len();
            // Text edit as one undo step incl. text-derived cache
            // refresh (#147).
            cmds.push(Box::new(dc::textfunc::text_edit_macro(
                &ed.doc,
                &Scope::Shape {
                    page: page_id,
                    shape: container,
                },
                &format!("{title} ({count})"),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(
                crate::i18n::strs()
                    .msg_undo_add_n_to_container(members.len() as i64, container.to_string()),
                cmds,
            ));
        }
    }

    /// Context-menu Container (#27): wrap the current selection's
    /// union bbox (padded) in a plain container shape at the back of
    /// the z-order; membership rides on the members. One undo step.
    pub(super) fn wrap_selection_in_container(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let members: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, s)| *s)
            .collect();
        if members.is_empty() {
            return;
        }
        let Some(ed) = self.ed() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let mut x0 = f64::MAX;
        let mut y0 = f64::MAX;
        let mut x1 = f64::MIN;
        let mut y1 = f64::MIN;
        for sid in &members {
            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) else {
                continue;
            };
            let Ok(chain) = shape_chain(&ed.doc, page_id, *sid) else {
                continue;
            };
            let g = |n: &str| {
                ed.doc
                    .resolve_cell(sh, &chain, n)
                    .and_then(|c| c.numeric())
                    .unwrap_or(0.0)
            };
            let (px, py) = (g("PinX"), g("PinY"));
            let (w, h) = (g("Width"), g("Height"));
            let (lx, ly) = (g("LocPinX"), g("LocPinY"));
            x0 = x0.min(px - lx);
            y0 = y0.min(py - ly);
            x1 = x1.max(px - lx + w);
            y1 = y1.max(py - ly + h);
        }
        if x0 > x1 {
            return;
        }
        const PAD: f64 = 0.25;
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let rect = (x0 - PAD, y0 - PAD, x1 - x0 + 2.0 * PAD, y1 - y0 + 2.0 * PAD);
        self.apply(dc_core::commands::add_container(
            page_id,
            id,
            &members,
            rect,
            // heading text is shape CONTENT — stays English (#246)
            "Container",
        ));
        self.set_selection(Some((page_id, id)));
    }
}

impl App {
    /// #156: is the primary selection a container — the target of a
    /// member-side `msvSDContainerShape` reference, a `Container.`
    /// synthetic (#138), or an effective `msvStructureType=Container`
    /// root (the master-based SDCONT family)?
    pub(super) fn sel_is_container(&self) -> bool {
        let Some((page_id, sid)) = self.sel else {
            return false;
        };
        let Some(ed) = self.ed() else {
            return false;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return false;
        };
        if !self.container_member_ids(page_id, sid).is_empty() {
            return true;
        }
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
            return false;
        };
        if sh
            .name_u
            .as_deref()
            .is_some_and(|n| n.starts_with("Container."))
        {
            return true;
        }
        let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
            return false;
        };
        // #191: the msvStructureType row lives on the MASTER root; a
        // real instance carries local sections, so effective_sheet's
        // either/or return hides it — resolve through the chain.
        ed.doc
            .effective_user_row(sh, &chain, "msvStructureType")
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.v.clone())
            .is_some_and(|v| v == "Container")
    }

    /// #156: member highlight reads the container's effective
    /// `msvSDContainerNoHighlight` row (Visio's switch); default on.
    pub(super) fn container_highlight_on(&self) -> bool {
        let Some((page_id, sid)) = self.sel else {
            return true;
        };
        let Some(ed) = self.ed() else {
            return true;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return true;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, sid) else {
            return true;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, sid) else {
            return true;
        };
        // #191: chain-aware like sel_is_container — the row lives on
        // the master root until an instance edit materializes it.
        ed.doc
            .effective_user_row(sh, &chain, "msvSDContainerNoHighlight")
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.numeric())
            .map(|v| v == 0.0)
            .unwrap_or(true)
    }

    /// #156: the Container Tools toggle — writes the
    /// `msvSDContainerNoHighlight` User row onto the container
    /// (materialized like Visio's first edit of an inherited row).
    pub(super) fn toggle_container_highlight(&mut self) {
        let Some((page_id, sid)) = self.sel else {
            return;
        };
        let on = self.container_highlight_on();
        self.apply(Macro::new(
            crate::i18n::strs().msg_toggle_member_highlight(),
            vec![Box::new(dc_core::SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                "User",
                0,
                dc_core::commands::RowSel::Named("msvSDContainerNoHighlight".into()),
                "Value",
                vsdx::Cell::value(if on { "1" } else { "0" }),
            ))],
        ));
        self.status = if on {
            crate::i18n::strs().msg_member_highlight_off()
        } else {
            crate::i18n::strs().msg_member_highlight_on()
        };
    }

    /// #156 (owner note 2026-11-01): which stencil variant matches the
    /// PAPER — an ISO A-size page is metric (the `-m` masters), an
    /// ANSI/Letter page is US units; a custom size falls back to the
    /// pane's "Metric Stencils" preference. The preset pairs mirror
    /// page_setup_window's SIZES table.
    pub(super) fn paper_is_metric(&self) -> bool {
        let geom = self.page_geom();
        let (lo, hi) = (geom.w_in.min(geom.h_in), geom.w_in.max(geom.h_in));
        const ISO: &[(f64, f64)] = &[
            (5.83, 8.27),   // A5
            (8.27, 11.69),  // A4
            (11.69, 16.54), // A3
            (16.54, 23.39), // A2
            (23.39, 33.11), // A1
            (33.11, 46.81), // A0
        ];
        const ANSI: &[(f64, f64)] = &[
            (8.5, 11.0),  // Letter (ANSI A)
            (8.5, 14.0),  // Legal
            (11.0, 17.0), // Tabloid (ANSI B)
            (17.0, 22.0), // ANSI C
            (22.0, 34.0), // ANSI D
            (34.0, 44.0), // ANSI E
        ];
        let near = |t: &(f64, f64)| (lo - t.0).abs() < 0.02 && (hi - t.1).abs() < 0.02;
        if ISO.iter().any(near) {
            return true;
        }
        if ANSI.iter().any(near) {
            return false;
        }
        self.stencil.metric_units
    }

    /// #156: apply an SDCONT style — the selection (or the selected
    /// container's members, a restyle) wraps in a master-based
    /// container of that style, one undo Macro.
    pub(super) fn apply_container_style(&mut self, style: &str) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        // The sdcont source: an open stencil (the session's explicit
        // variant choice), else the embedded catalog parsed in the
        // variant that matches the PAPER (owner note 2026-11-01: ISO
        // A-size → the metric twin sdcont-m, ANSI/Letter → US).
        let src_master: Option<vsdx::Master> = self
            .stencils
            .iter()
            .filter(|(_, st)| st.source.to_string_lossy().contains("sdcont"))
            .find_map(|(_, st)| find_master_by_name(&st.doc, style).cloned())
            .or_else(|| {
                let b = crate::stencil::builtin::by_slug("sdcont")?;
                let doc = vsdx::parse::parse_bytes(b.bytes_for(self.paper_is_metric())).ok()?;
                find_master_by_name(&doc, style).cloned()
            });
        let Some(src) = src_master else {
            self.status = crate::i18n::strs().msg_container_style_style_not_found(style);
            return;
        };
        // Members: the selected container re-wraps (a restyle);
        // anything else wraps the selection itself.
        let (members, old_container) = if self.sel_is_container() {
            let (_, sid) = self.sel.unwrap();
            (self.container_member_ids(page_id, sid), Some(sid))
        } else {
            (
                self.sel_set
                    .iter()
                    .filter(|(p, _)| *p == page_id)
                    .map(|(_, s)| *s)
                    .collect(),
                None,
            )
        };
        if members.is_empty() {
            self.status = crate::i18n::strs().msg_select_the_shapes_to_contain_fir();
            return;
        }
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let mut x0 = f64::MAX;
        let mut y0 = f64::MAX;
        let mut x1 = f64::MIN;
        let mut y1 = f64::MIN;
        for sid in &members {
            let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, *sid) else {
                continue;
            };
            let Ok(chain) = shape_chain(&ed.doc, page_id, *sid) else {
                continue;
            };
            let g = |n: &str| {
                ed.doc
                    .resolve_cell(sh, &chain, n)
                    .and_then(|c| c.numeric())
                    .unwrap_or(0.0)
            };
            let (px, py) = (g("PinX"), g("PinY"));
            let (w, h) = (g("Width"), g("Height"));
            let (lx, ly) = (g("LocPinX"), g("LocPinY"));
            x0 = x0.min(px - lx);
            y0 = y0.min(py - ly);
            x1 = x1.max(px - lx + w);
            y1 = y1.max(py - ly + h);
        }
        if x0 > x1 {
            return;
        }
        // The style's own member margin (the master's
        // msvSDContainerMargin; SDCONT runs 0.125–0.375).
        let margin = src
            .shapes
            .first()
            .and_then(|root| {
                root.sheet
                    .sections_named("User")
                    .iter()
                    .find_map(|sec| sec.named_row("msvSDContainerMargin"))
            })
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.numeric())
            .unwrap_or(0.25);
        let rect = (
            x0 - margin,
            y0 - margin,
            x1 - x0 + 2.0 * margin,
            y1 - y0 + 2.0 * margin,
        );
        // Place + size the instance, then move the members over.
        let new_id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let mut place =
            dc_core::place_master_from_stencil(&ed.doc, page_id, &src, (rect.0, rect.1));
        let mut cmds = std::mem::take(&mut place.cmds);
        for (n, v) in [
            ("PinX", rect.0),
            ("PinY", rect.1),
            ("Width", rect.2),
            ("Height", rect.3),
            ("LocPinX", 0.0),
            ("LocPinY", 0.0),
        ] {
            cmds.push(Box::new(SetCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: new_id,
                },
                n,
                vsdx::Cell::value(format!("{v:.6}")),
            )));
        }
        for m in &members {
            cmds.push(Box::new(dc_core::SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: *m,
                },
                "User",
                0,
                dc_core::commands::RowSel::Named("msvSDContainerShape".into()),
                "Value",
                vsdx::Cell::value(new_id.to_string()),
            )));
        }
        cmds.push(Box::new(dc_core::commands::ReorderShape::new(
            page_id, new_id, false,
        )));
        if let Some(old) = old_container {
            cmds.push(Box::new(dc_core::commands::RemoveShape::new(page_id, old)));
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_apply_container_style(style),
            cmds,
        ));
        self.set_selection(Some((page_id, new_id)));
        self.sel_set = vec![(page_id, new_id)];
        self.tab = Tab::ContainerTools;
        self.status = crate::i18n::strs().msg_container_style_style_applied(style);
    }
}

impl App {
    /// The Copy as Picture dialog (#113, URS-1200).
    /// #156: the Container Styles gallery (Design ▸ Containers…) —
    /// the 14 SDCONT styles; a click wraps the selection (Visio's
    /// gallery-with-a-selection flow) and selects the new container.
    pub(super) fn container_gallery_window(&mut self, ctx: &egui::Context) {
        if !self.contgal_open {
            return;
        }
        let mut open = true;
        let mut pick: Option<&'static str> = None;
        egui::Window::new(crate::i18n::strs().msg_container_styles())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.with_layout(dialog_dir(), |ui| {
                    ui.weak(crate::i18n::strs().msg_contgal_prompt());
                    ui.add_space(4.0);
                    egui::Grid::new("dc-contgal")
                        .num_columns(2)
                        .spacing(egui::vec2(8.0, 4.0))
                        .show(ui, |ui| {
                            // #257: an RTL grid fills its rows right-to-left.
                            let styles: Vec<&str> = if crate::i18n::rtl() {
                                CONTAINER_STYLES
                                    .chunks(2)
                                    .flat_map(|c| c.iter().rev().copied())
                                    .collect()
                            } else {
                                CONTAINER_STYLES.to_vec()
                            };
                            for name in styles {
                                if ui.button(name).clicked() {
                                    pick = Some(name);
                                }
                            }
                        });
                });
            });
        if let Some(name) = pick {
            self.contgal_open = false;
            self.apply_container_style(name);
        }
        if !open {
            self.contgal_open = false;
        }
    }
}

impl App {
    /// #95's UI guard: when `members` contains exactly one top-level
    /// Group (plus at least one other shape), grouping nests into that
    /// group — returns the parent id and the members' page-space bbox
    /// origin converted into the parent's local frame.
    pub(super) fn nest_group_target(&self, members: &[i64]) -> Option<(i64, (f64, f64))> {
        if members.len() < 2 {
            return None;
        }
        let ed = self.ed()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let page_id = page.id;
        let mut groups = Vec::new();
        for &id in members {
            let s = page.shapes.iter().find(|s| s.id == id)?;
            if s.type_.as_deref() == Some("Group") {
                groups.push(id);
            }
        }
        if groups.len() != 1 {
            return None;
        }
        let parent = groups[0];
        // The members' page-space bbox origin (the group-to-be's pin
        // before conversion): union of the non-parent members' rects.
        let mut x0 = f64::MAX;
        let mut y0 = f64::MAX;
        for &id in members {
            if id == parent {
                continue;
            }
            let s = page.shapes.iter().find(|s| s.id == id)?;
            let chain = shape_chain(&ed.doc, page_id, id).ok()?;
            let get = |n: &str| ed.doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
            let (px, py) = (get("PinX")?, get("PinY")?);
            let lx = get("LocPinX").unwrap_or(0.0);
            let ly = get("LocPinY").unwrap_or(0.0);
            x0 = x0.min(px - lx);
            y0 = y0.min(py - ly);
        }
        let page_h = Self::page_height(ed, page_id);
        // The conversion needs the PARENT as an ancestor — the affine
        // maps the chain's last entry's parent-local frame, so append
        // the new group's own slot (the parent twice).
        let mut chain = shape_chain(&ed.doc, page_id, parent).ok()?;
        chain.push(chain[0]);
        let local = dc::svg::page_point_to_parent_local(&ed.doc, &chain, page_h, (x0, y0));
        Some((parent, local))
    }
}

impl App {
    /// Rotate every selected shape ±90° (degrees, Visio CCW positive).
    /// #252 Container Tools ▸ Membership: disband the selected
    /// container — every member's `msvSDContainerShape` reference is
    /// released and the container shape removed, in one undo step.
    pub(super) fn disband_container(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(cont) = self.sel_container_id() else {
            self.status = crate::i18n::strs().msg_select_a_container_to_disband();
            return;
        };
        let members = self.container_member_ids(page_id, cont);
        // The same cleanup delete_one runs (#218 review): the
        // membership ROWS come out (#138 treats leftovers as
        // dangling), and Connects pointing at the container go with
        // it — writing empty Values would leave both in the file.
        let mut cmds: Vec<Box<dyn dc_core::Command>> =
            vec![Box::new(dc_core::commands::RemoveShape::new(page_id, cont))];
        if let Some(ed) = self.ed() {
            cmds.extend(dc_core::container_membership_cleanup(
                &ed.doc,
                page_id,
                &[cont],
            ));
        }
        cmds.extend(self.connect_cleanup_cmds(page_id, &[cont]));
        self.apply(dc_core::Macro::new(
            crate::i18n::strs().msg_undo_disband_container(members.len() as i64),
            cmds,
        ));
        self.set_selection(None);
    }

    /// The selected container's id, if the selection is a container
    /// (membership points at it — the same test the contextual tab
    /// uses).
    pub(super) fn sel_container_id(&self) -> Option<i64> {
        let page_id = self.page_id()?;
        let (_, sid) = self.sel?;
        let has_members = !self.container_member_ids(page_id, sid).is_empty();
        if has_members || self.sel_is_container() {
            Some(sid)
        } else {
            None
        }
    }

    /// #252 Home ▸ Arrange: group the current multi-selection — the
    /// same command the ctx menu fires, including #95's
    /// group-into-group nesting.
    pub(super) fn group_selection(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let members: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, sid)| *sid)
            .collect();
        if members.len() < 2 {
            self.status = crate::i18n::strs().msg_group_needs_2_shapes_on_the_page();
            return;
        }
        let new_id = self
            .ed()
            .map(|ed| dc_core::commands::next_shape_id(&ed.doc, page_id));
        let nest = self.nest_group_target(&members);
        let cmd = match nest {
            Some((parent, local_pin)) => {
                let nested: Vec<i64> = members.iter().copied().filter(|id| *id != parent).collect();
                dc_core::commands::GroupShapes::into_group(page_id, nested, parent, local_pin)
            }
            None => dc_core::commands::GroupShapes::new(page_id, members),
        };
        self.apply(cmd);
        if let Some(nid) = new_id {
            self.set_selection(Some((page_id, nid)));
        }
    }

    /// #252 Home ▸ Arrange: ungroup every selected group — one Macro
    /// (one undo step); the first ex-member becomes the selection.
    pub(super) fn ungroup_selection(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let groups: Vec<(i64, Option<i64>)> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, sid)| {
                let first_member = vsdx::model::find_shape_by_id(&page.shapes, *sid)
                    .and_then(|g| g.sub_shapes.first().map(|m| m.id));
                (*sid, first_member)
            })
            .collect();
        let cmds: Vec<Box<dyn dc_core::Command>> = groups
            .iter()
            .filter(|(_, m)| m.is_some())
            .map(|(sid, _)| Box::new(dc_core::commands::UngroupShape::new(page_id, *sid)) as _)
            .collect();
        if cmds.is_empty() {
            self.status = crate::i18n::strs().msg_select_a_group_to_ungroup();
            return;
        }
        let n = cmds.len();
        self.apply(dc_core::Macro::new(
            crate::i18n::strs().msg_undo_ungroup_n(n),
            cmds,
        ));
        // The selection becomes the first EX-MEMBER of a group that
        // was actually ungrouped — `groups.first()` can be a plain
        // shape (None member), which left sel_set holding the removed
        // group ids and rolled back a following Delete (#218 review
        // round). Reset either way: the old ids are gone.
        let first = groups.iter().find_map(|(_, m)| *m);
        self.set_selection(first.map(|m| (page_id, m)));
    }
}
