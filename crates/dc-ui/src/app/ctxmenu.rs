//! Context menus: the quick-shapes menu and the shape context menu,
//! plus clipboard copy/paste and inline text-edit sessions.

use super::*;

impl App {
    /// The Quick Shapes membership menu (#143), opened by
    /// right-clicking a master cell — in its source entry or in Quick
    /// Shapes (a quick cell's key IS its source key). One item whose
    /// label follows live membership; Visio persists membership
    /// app-side and so does this (never the .vsdx, #88's rule).
    pub(super) fn quick_shapes_menu(&mut self, ctx: &egui::Context) {
        let Some(((src_id, master_id), anchor)) = self.quick_menu else {
            return;
        };
        let identity = crate::stencil::quick_source_id(src_id, &self.stencils);
        let in_default = match src_id {
            StencilId::Document => self
                .ed
                .as_ref()
                .map(|e| crate::stencil::quick_default_ids(&e.doc).contains(&master_id)),
            StencilId::Open(ix) => self
                .stencils
                .get(ix)
                .map(|st| crate::stencil::quick_default_ids(&st.doc).contains(&master_id)),
        };
        // A closed source whose pane slot shifted has no stable
        // identity for the menu — drop it.
        let (Some(identity), Some(in_default)) = (identity, in_default) else {
            self.quick_menu = None;
            return;
        };
        let member = self
            .quick_shapes
            .is_member(&identity, master_id, in_default);
        let mut act: Option<bool> = None; // true = add, false = remove
        let menu = egui::Popup::new(
            egui::Id::unique("dc-quick-shapes-menu"),
            ctx.clone(),
            anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup = menu.show(|ui| {
            ui.set_min_width(214.0);
            let label = if member {
                crate::i18n::strs().msg_remove_from_quick_shapes()
            } else {
                crate::i18n::strs().msg_add_to_quick_shapes()
            };
            if ui.button(label).clicked() {
                act = Some(!member);
            }
        });
        if let Some(add) = act {
            if add {
                self.quick_shapes.add(&identity, master_id);
                self.status = crate::i18n::strs().msg_added_to_quick_shapes();
            } else {
                self.quick_shapes.remove(&identity, master_id);
                self.status = crate::i18n::strs().msg_removed_from_quick_shapes();
            }
            self.quick_menu = None;
            return;
        }
        // Close on an outside PRIMARY click (the shape menu's
        // idiom): the opening right-click's own release is a
        // secondary click and must not dismiss the menu it opened.
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.quick_menu = None;
            }
        }
    }

    pub(super) fn shape_context_menu(&mut self, ctx: &egui::Context) {
        let Some((shape_id, pointer_anchor, _page_pos)) = self.ctx_menu else {
            self.ctx_menu_rect = None;
            return;
        };
        let has_clip = self.clipboard.is_some();
        // #75: the underlying-container membership item enables when a
        // container sits under the selection AND the selection is not
        // already entirely its member set (a pure no-op stays gray).
        let sel_members: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| Some(p) == self.page_id().as_ref())
            .map(|(_, s)| *s)
            .collect();
        let underlying = self.underlying_container(&sel_members);
        let can_underlying = match underlying {
            Some(c) => {
                let current = self.container_member_ids(self.page_id().unwrap_or(-1), c);
                // Enables only when a selected shape is not yet a
                // member — a pure re-add is a no-op and stays gray
                // (#75 branch review).
                sel_members.iter().any(|m| !current.contains(m))
            }
            None => false,
        };
        // Master-defined actions (the ShapeSheet Actions section) head the
        // menu in Visio — e.g. the Capability master's "Shape Layout"
        // flyout. Entries render with their flyout children indented;
        // executing SETF actions arrives with a later milestone.
        let actions = self.master_actions(shape_id);

        #[derive(Clone, Copy, PartialEq)]
        enum Act {
            Master(usize),
            Group,
            Ungroup,
            /// Enter the selected group's edit mode (#78).
            OpenGroup(i64),
            Container,
            /// #75: membership into the container under the selection.
            AddToUnderlying,
            Hyperlink,
            AddComment,
            ShapeData,
            FormatShape,
            /// #149 item 2: the mini-toolbar's three Format icons open
            /// the pane focused on their section.
            FormatFill,
            FormatLine,
            FormatText,
            Cut,
            Copy,
            Paste,
            Duplicate,
            Delete,
            Front,
            Back,
            Rot(f64),
            FlipV,
            FlipH,
            EditText,
            ShapeSheet,
            /// #108: per-connector route style (URS-1114).
            RouteStraight,
            RouteRightAngle,
            RouteCurved,
            /// #113/URS-1200: render the selection to a picture.
            CopyAsPicture,
            /// #257 text-direction: toggle the shape's paragraph base
            /// direction (explicit U+202B/U+202A marks — VSDX has no
            /// RTLText cell; Analysis/rtl-text-direction.md).
            ToggleRtlText,
        }
        let mut act: Option<Act> = None;
        // Min menu width incl. frame, until the popup reports its own
        // measured size (used only for the right-edge flip decision).
        const MENU_MIN_W: f32 = 244.0;
        let mut menu = egui::Popup::new(
            egui::Id::unique("dc-shape-ctx-menu"), // stable per-menu id is intended
            ctx.clone(),
            pointer_anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        // Cap the sizing pass: menu rows stretch to available width, so
        // without this the first frame balloons the menu to the viewport
        // width and it covers the page (the pre-#49 symptom).
        .width(232.0);
        // Visio anchors the menu just outside the shape's top-right
        // corner, mirroring to the top-left corner when the right screen
        // edge is too close (issue #49). #257: an RTL chrome mirrors
        // the anchor — the menu opens to the LEFT of the shape when the
        // left screen edge has room, right edge otherwise. Anchoring to
        // the shape's live rect keeps the menu glued to it through
        // zoom/pan; if the rect cannot be resolved, fall back to the
        // pointer position like a classic context menu (egui defaults).
        // Alternatives stay empty so the corner anchor is deterministic.
        if let Some(r) = self.shape_screen_rect(shape_id) {
            let menu_w = menu
                .get_expected_size()
                .map_or(MENU_MIN_W, |s| s.x.max(MENU_MIN_W));
            // The popup Area is constrained into the content rect, so
            // measure room against the same rect.
            let rtl = crate::i18n::rtl();
            let room_preferred = if rtl {
                r.left() - ctx.content_rect().left() >= menu_w
            } else {
                ctx.content_rect().right() - r.right() >= menu_w
            };
            let (preferred, other) = if rtl {
                (egui::RectAlign::LEFT_START, egui::RectAlign::RIGHT_START)
            } else {
                (egui::RectAlign::RIGHT_START, egui::RectAlign::LEFT_START)
            };
            menu = menu
                .anchor(r)
                .gap(2.0)
                .align(if room_preferred { preferred } else { other })
                .align_alternatives(&[]);
        }
        let popup = menu.show(|ui| {
            ui.set_min_width(232.0);
            // Menu colors resolve from the live visuals via the theme
            // mapping, so the popup follows a light/dark switch (GitHub
            // #48) instead of assuming Visio-light surroundings.
            let tokens = theme::menu_tokens(ui.visuals());
            ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = tokens.hover_fill;
            // separators ride the token too (egui's dark default is too
            // faint on the dark menu fill)
            ui.visuals_mut().widgets.noninteractive.bg_stroke.color = tokens.separator;
            // The mini-toolbar (#77 landed a flat icon bar; #149
            // restyles it to the real Visio 2010/2016 menu): a
            // two-row ribbon-chrome panel above the text rows, with
            // the Flip flyout (#149 item 1) and the section-targeted
            // Format icons (#149 item 2). The seven #77 commands stay
            // the working set until #149 item 3's oracle pass
            // confirms the real slot map.
            {
                let can_group = self.sel_set.len() >= 2;
                if let Some(bar) = crate::mini_toolbar::show(ui, can_group) {
                    act = Some(match bar {
                        crate::mini_toolbar::Act::EditText => Act::EditText,
                        crate::mini_toolbar::Act::FormatFill => Act::FormatFill,
                        crate::mini_toolbar::Act::FormatLine => Act::FormatLine,
                        crate::mini_toolbar::Act::FormatText => Act::FormatText,
                        crate::mini_toolbar::Act::Front => Act::Front,
                        // The text row's sign: page coordinates are
                        // y-up, so clockwise (right) is NEGATIVE
                        // radians (#77 branch review — the bar had
                        // it backwards).
                        crate::mini_toolbar::Act::RotateRight => {
                            Act::Rot(-std::f64::consts::FRAC_PI_2)
                        }
                        crate::mini_toolbar::Act::FlipVertical => Act::FlipV,
                        crate::mini_toolbar::Act::FlipHorizontal => Act::FlipH,
                        crate::mini_toolbar::Act::Group => Act::Group,
                        // The Styles tile's stand-in until a styles
                        // feature: the pane, generically.
                        crate::mini_toolbar::Act::Styles => Act::FormatShape,
                    });
                }
                ui.add_space(2.0);
            }
            // Visio order: master actions, Show ShapeSheet, clipboard,
            // object commands, then the grayed future milestones.
            // Actions rows are live (#25): evaluated checkmarks and
            // disabled state, BeginGroup separators, FlyoutChild
            // indentation, and SETF execution on click.
            for (ai, row) in actions.iter().enumerate() {
                if row.begin_group && ai > 0 {
                    ui.separator();
                }
                // Flyout arrow: › (the pinned font set has no ▸);
                // #257: the RTL menu's flyouts open leftward, so the
                // arrow mirrors with the chrome.
                let label = if row.flyout_child {
                    format!("     {}", row.menu)
                } else if crate::i18n::rtl() {
                    format!("‹  {}", row.menu)
                } else {
                    format!("{}  ›", row.menu)
                };
                // A display-only row (No Formula) stays grayed like
                // Visio's flyout headers; Disabled is the formula's.
                let enabled = !row.disabled && !row.display_only && !row.action.trim().is_empty();
                let icon = row.checked.then_some(crate::ribbon::Icon::Check);
                if ctx_item(ui, icon, &label, "", enabled) && enabled {
                    act = Some(Act::Master(ai));
                }
            }
            if !actions.is_empty() {
                ui.separator();
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::ShapeSheet),
                &crate::i18n::strs().msg_show_shapesheet(),
                "",
                true,
            ) {
                act = Some(Act::ShapeSheet);
            }
            ui.separator();
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Cut),
                &crate::i18n::strs().msg_cut(),
                "Ctrl+X",
                true,
            ) {
                act = Some(Act::Cut);
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Copy),
                &crate::i18n::strs().msg_copy(),
                "Ctrl+C",
                true,
            ) {
                act = Some(Act::Copy);
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Paste),
                &crate::i18n::strs().msg_paste(),
                "Ctrl+V",
                has_clip,
            ) {
                act = Some(Act::Paste);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_duplicate(),
                "Ctrl+D",
                true,
            ) {
                act = Some(Act::Duplicate);
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Delete),
                &crate::i18n::strs().msg_delete(),
                "Del",
                true,
            ) {
                act = Some(Act::Delete);
            }
            ui.separator();
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::BringFront),
                &crate::i18n::strs().msg_bring_to_front(),
                "",
                true,
            ) {
                act = Some(Act::Front);
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::SendBack),
                &crate::i18n::strs().msg_send_to_back(),
                "",
                true,
            ) {
                act = Some(Act::Back);
            }
            ui.separator();
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Rotate),
                &crate::i18n::strs().msg_rotate_right_90(),
                "",
                true,
            ) {
                act = Some(Act::Rot(-std::f64::consts::FRAC_PI_2));
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Rotate),
                &crate::i18n::strs().msg_rotate_left_90(),
                "",
                true,
            ) {
                act = Some(Act::Rot(std::f64::consts::FRAC_PI_2));
            }
            if ctx_item(ui, None, &crate::i18n::strs().msg_flip_vertical(), "", true) {
                act = Some(Act::FlipV);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_flip_horizontal(),
                "",
                true,
            ) {
                act = Some(Act::FlipH);
            }
            ui.separator();
            // Group ▸ is a flyout (Visio 2016, #78): Group/Ungroup live
            // inside it, plus Open <name> to enter group-edit mode.
            // Group needs a multi-selection; Ungroup/Open need a
            // selected group (#26).
            let can_group = self.sel_set.len() >= 2;
            let selected_group = self
                .ed()
                .and_then(|ed| ed.doc.pages.get(self.page_idx))
                .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id))
                .filter(|s| s.type_.as_deref() == Some("Group") && !s.sub_shapes.is_empty());
            let is_group = selected_group.is_some();
            let group_label = selected_group
                .and_then(|s| s.name_u.clone())
                .unwrap_or_else(|| format!("Group.{shape_id}"));
            // The flyout parent row: drawn directly so we own its
            // Response for Popup::menu (the Container ▸ pattern).
            let tokens = theme::menu_tokens(ui.visuals());
            // #257: the RTL row right-aligns (halign on the job) and
            // its flyout arrow opens leftward; the same mirrored row
            // routes the label through the catalog.
            let mut group_job = egui::text::LayoutJob {
                halign: if crate::i18n::rtl() {
                    egui::Align::RIGHT
                } else {
                    egui::Align::LEFT
                },
                ..egui::text::LayoutJob::default()
            };
            group_job.append(
                &crate::i18n::strs().msg_group_flyout(),
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(12.5), tokens.label),
            );
            let group_row = ui.add_enabled(
                true,
                egui::Button::new(group_job)
                    .min_size(Vec2::new(ui.available_width().max(150.0), 21.0)),
            );
            egui::Popup::menu(&group_row)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(210.0);
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_group(),
                        "Ctrl+G",
                        can_group,
                    ) {
                        act = Some(Act::Group);
                    }
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_ungroup(),
                        "Ctrl+Shift+G",
                        is_group,
                    ) {
                        act = Some(Act::Ungroup);
                    }
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_open_group_label(group_label),
                        "",
                        is_group,
                    ) {
                        act = Some(Act::OpenGroup(shape_id));
                    }
                });
            // Container is a flyout, not an action (Visio 2016): the
            // parent row only opens the submenu. Anchor: draw the row
            // directly so we own its Response for Popup::menu.
            let mut job = egui::text::LayoutJob {
                halign: if crate::i18n::rtl() {
                    egui::Align::RIGHT
                } else {
                    egui::Align::LEFT
                },
                ..egui::text::LayoutJob::default()
            };
            job.append(
                &crate::i18n::strs().msg_container_flyout(),
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(12.5), tokens.label),
            );
            let cont = ui.add_enabled(
                !self.sel_set.is_empty(),
                egui::Button::new(job).min_size(Vec2::new(ui.available_width().max(150.0), 21.0)),
            );
            let mut sub_action = false;
            let mut underlying_action = false;
            egui::Popup::menu(&cont)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| {
                    ui.set_min_width(210.0);
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_add_to_new_container(),
                        "",
                        true,
                    ) {
                        sub_action = true;
                    }
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_add_to_underlying_container(),
                        "",
                        can_underlying,
                    ) && can_underlying
                    {
                        underlying_action = true;
                    }
                });
            if sub_action {
                act = Some(Act::Container);
            }
            if underlying_action {
                act = Some(Act::AddToUnderlying);
            }
            // #108: connector route styles (Visio 2016 puts
            // Straight/Right-Angle Connector on the connector's menu).
            {
                let is_conn = self.page_id().is_some_and(|pid| {
                    self.ed.as_ref().is_some_and(|ed| {
                        let page = ed.doc.pages.iter().find(|p| p.id == pid);
                        page.and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id))
                            .is_some_and(crate::connectors::is_connector)
                    })
                });
                if is_conn {
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_straight_connector(),
                        "",
                        true,
                    ) {
                        act = Some(Act::RouteStraight);
                    }
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_right_angle_connector(),
                        "",
                        true,
                    ) {
                        act = Some(Act::RouteRightAngle);
                    }
                    if ctx_item(
                        ui,
                        None,
                        &crate::i18n::strs().msg_curved_connector(),
                        "",
                        true,
                    ) {
                        act = Some(Act::RouteCurved);
                    }
                    ui.separator();
                }
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_copy_as_picture_menu(),
                "",
                self.sel.is_some(),
            ) {
                act = Some(Act::CopyAsPicture);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_hyperlink_menu(),
                "Ctrl+K",
                true,
            ) {
                act = Some(Act::Hyperlink);
            }
            if ctx_item(ui, None, &crate::i18n::strs().msg_comment_add(), "", true) {
                act = Some(Act::AddComment);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_shape_data_menu(),
                "",
                true,
            ) {
                act = Some(Act::ShapeData);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_format_shape_menu(),
                "",
                true,
            ) {
                act = Some(Act::FormatShape);
            }
            ui.separator();
            if ctx_item(ui, None, &crate::i18n::strs().msg_edit_text(), "F2", true) {
                act = Some(Act::EditText);
            }
            if ctx_item(ui, None, &crate::i18n::strs().msg_rtl_text_menu(), "", true) {
                act = Some(Act::ToggleRtlText);
            }
        });
        self.ctx_menu_rect = popup.as_ref().map(|ir| ir.response.rect);
        // Close on an outside primary click; a right-click elsewhere
        // re-anchors through central()'s handler.
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.ctx_menu = None;
            }
        }
        let Some(a) = act else { return };
        self.ctx_menu = None;
        let Some((page_id, _)) = self.sel else { return };
        match a {
            Act::Master(ai) => {
                if let Some(row) = actions.get(ai).cloned() {
                    self.run_master_action(shape_id, &row);
                }
            }
            Act::Group => {
                // #252: shared with Home ▸ Arrange's Group button.
                self.group_selection();
            }
            Act::Container => self.wrap_selection_in_container(),
            Act::AddToUnderlying => self.add_selection_to_underlying_container(),
            Act::Hyperlink => {
                self.hyperlink_open = Some(shape_id);
                self.hyperlink_draft = None;
            }
            Act::AddComment => {
                self.comment_target = Some(shape_id);
                self.comment_body.clear();
                self.comment_edit = None;
            }
            Act::ShapeData => {
                self.shape_data_open = Some(shape_id);
                self.prop_bufs.clear();
                self.prop_add = None;
            }
            Act::FormatShape => {
                self.format_pane_open = true;
                self.fmt_bufs.clear();
                self.format_pane_focus = None;
            }
            // #149 item 2: the mini-toolbar's three Format icons open
            // the pane FOCUSED on their section — the reference maps
            // the Fill/Line/Text slots to those pane areas.
            Act::FormatFill | Act::FormatLine | Act::FormatText => {
                self.format_pane_open = true;
                self.fmt_bufs.clear();
                let section = match a {
                    Act::FormatFill => FormatSection::Fill,
                    Act::FormatLine => FormatSection::Line,
                    _ => FormatSection::Text,
                };
                self.format_pane_focus = Some(section);
                self.format_pane_last_focus = Some(section);
            }
            Act::Ungroup => {
                let first_member = self.ed().and_then(|ed| {
                    ed.doc.pages.get(self.page_idx).and_then(|p| {
                        vsdx::model::find_shape_by_id(&p.shapes, shape_id)
                            .and_then(|g| g.sub_shapes.first().map(|m| m.id))
                    })
                });
                self.apply(dc_core::commands::UngroupShape::new(page_id, shape_id));
                if let Some(m) = first_member {
                    self.set_selection(Some((page_id, m)));
                }
            }
            Act::OpenGroup(gid) => {
                // Enter (or deepen) group-edit mode (#78): view state
                // only — members keep editing through the same commands,
                // one undo each, as on the page.
                if !self.group_edit.contains(&gid) {
                    self.group_edit.push(gid);
                }
                let name = self.ed().and_then(|ed| {
                    ed.doc.pages.get(self.page_idx).and_then(|p| {
                        vsdx::model::find_shape_by_id(&p.shapes, gid).and_then(|s| s.name_u.clone())
                    })
                });
                self.status = crate::i18n::strs()
                    .msg_opened_group(name.unwrap_or_else(|| format!("Group.{gid}")));
            }
            Act::Cut | Act::Copy => {
                self.copy_selection();
                if a == Act::Cut {
                    self.delete_one(page_id, shape_id);
                    self.set_selection(None);
                }
            }
            Act::Paste => {
                self.paste_clipboard(page_id);
            }
            Act::Duplicate => {
                let new_id = self
                    .ed
                    .as_ref()
                    .map(|ed| dc_core::commands::next_shape_id(&ed.doc, page_id));
                self.apply(DuplicateShape::new(page_id, shape_id, (0.25, -0.25)));
                if let Some(nid) = new_id {
                    self.set_selection(Some((page_id, nid)));
                }
            }
            Act::Delete => self.delete_selection(),
            Act::Front => self.apply(ReorderShape::new(page_id, shape_id, true)),
            Act::Back => self.apply(ReorderShape::new(page_id, shape_id, false)),
            Act::Rot(delta) => {
                let cur = self.selected_cell(shape_id, "Angle").unwrap_or(0.0);
                self.apply(SetCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: shape_id,
                    },
                    "Angle",
                    vsdx::Cell::value(format!("{:.10}", cur + delta)),
                ));
            }
            Act::FlipV | Act::FlipH => {
                let name = if a == Act::FlipV { "FlipY" } else { "FlipX" };
                let cur = self.selected_cell(shape_id, name).unwrap_or(0.0);
                self.apply(SetCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: shape_id,
                    },
                    name,
                    vsdx::Cell::value(if cur != 0.0 { "0" } else { "1" }),
                ));
            }
            Act::EditText => {
                let draft = self.selected_text(shape_id).unwrap_or_default();
                self.begin_text_edit(shape_id, draft);
                self.status = crate::i18n::strs().msg_editing_text_of_shape_shape_id(shape_id);
            }
            // #257 text-direction: prepend U+202B (RLE) to every
            // paragraph — or strip leading U+202A/U+202B back — through
            // the ordinary SetText path (one undo step, texture
            // invalidation). VSDX carries no direction cell, so the
            // explicit embedding marks are how the toggle persists
            // (Analysis/rtl-text-direction.md); the renderer's
            // explicit-override rule reads them.
            Act::ToggleRtlText => {
                // A shape with NO text yet can still pre-set its
                // direction — the empty text takes the bare mark (PR
                // #269 review).
                let text = self.selected_text(shape_id).unwrap_or_default();
                // The direction test reads the mark BEFORE any
                // stripping — trim-then-ask could never see the
                // U+202B and the toggle only ever added marks; off
                // now pins still-RTL paragraphs with an explicit
                // U+202A (PR #269 review — see svg::toggle_rtl_marks).
                let any_rtl = text
                    .replace("\r\n", "\n")
                    .lines()
                    .any(|l| l.starts_with('\u{202B}'));
                let toggled = dc::svg::toggle_rtl_marks(&text, !any_rtl);
                let scope = Scope::Shape {
                    page: page_id,
                    shape: shape_id,
                };
                let doc = self.ed().map(|ed| &ed.doc);
                if let Some(doc) = doc {
                    self.apply(dc::textfunc::text_edit_macro(doc, &scope, &toggled));
                }
                self.status = if any_rtl {
                    crate::i18n::strs().msg_rtl_text_off()
                } else {
                    crate::i18n::strs().msg_rtl_text_on()
                };
            }
            // #190/#192: a per-shape toggle for the CLICKED shape —
            // the one-way open left no reachable close; the spec made
            // sheets per-shape.
            Act::ShapeSheet => {
                self.toggle_shapesheet(crate::shapesheet::SheetTarget::Shape(shape_id))
            }
            Act::RouteStraight => self.apply_connector_route_style(
                page_id,
                shape_id,
                crate::connectors::ROUTE_STRAIGHT,
            ),
            Act::RouteRightAngle => self.apply_connector_route_style(
                page_id,
                shape_id,
                crate::connectors::ROUTE_RIGHT_ANGLE,
            ),
            Act::RouteCurved => {
                self.apply_connector_route_style(page_id, shape_id, crate::connectors::ROUTE_CURVED)
            }
            Act::CopyAsPicture => {
                self.copypic_open = true;
            }
        }
    }

    /// Deep-clone the selected shape into the in-app clipboard.
    pub(super) fn copy_selection(&mut self) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        if let Some(s) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) {
            self.clipboard = Some(s.clone());
            // #113 (URS-1113): the system clipboard receives the
            // shape's text so other apps get meaningful content (the
            // internal shape clipboard handles in-app paste).
            if let Some(txt) = s.sheet.text.as_deref().filter(|t| !t.is_empty()) {
                self.os_clipboard_text = Some(txt.to_string());
            }
        }
    }

    /// Paste the clipboard shape as a fresh instance (new id, nudged pin),
    /// top of the z-order, and select it.
    pub(super) fn paste_clipboard(&mut self, page_id: i64) {
        let Some(clip) = self.clipboard.clone() else {
            // #113 (URS-1113): nothing internal — an OS clipboard
            // IMAGE pastes as a picture (another app's graphic, or
            // this app's own Copy as Picture). Probed off-thread:
            // arboard opens its own connection and the paste path
            // runs on the UI thread.
            self.paste_os_image();
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let mut pasted = clip;
        pasted.id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        for (name, dv) in [("PinX", 0.25), ("PinY", -0.25)] {
            let v = pasted
                .sheet
                .cell(name)
                .and_then(|c| c.numeric())
                .unwrap_or(1.0)
                + dv;
            pasted
                .sheet
                .set_cell(name, vsdx::Cell::value(format!("{v}")));
        }
        let new_id = pasted.id;
        self.apply(AddShape {
            page: page_id,
            parent: None,
            shape: pasted,
        });
        self.set_selection(Some((page_id, new_id)));
    }

    /// One resolved numeric cell of a shape on the current page.
    pub(super) fn selected_cell(&self, shape_id: i64, name: &str) -> Option<f64> {
        let page_id = self.page_id()?;
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let s = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        ed.doc
            .resolve_cell(s, &chain, name)
            .and_then(|c| c.numeric())
    }

    /// Current text of a shape (draft seed for the in-place editor).
    pub(super) fn selected_text(&self, shape_id: i64) -> Option<String> {
        let page_id = self.page_id()?;
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let s = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        s.sheet.text.clone()
    }

    /// Open the in-place WYSIWYG editor on a shape (GitHub #22): the
    /// session captures the view (restored on exit) and the central
    /// panel applies the entry zoom, lays the editor over the text
    /// block, and suppresses the shape's text in its canvas layer.
    pub(super) fn begin_text_edit(&mut self, shape_id: i64, draft: String) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        self.text_edit = Some(crate::textedit::TextEditSession::new(
            page_id, shape_id, draft, &self.view,
        ));
    }

    /// End the in-place editor: restore the pre-edit view, and commit
    /// the draft through `SetText` (one undo step, dirty, texture
    /// invalidation — the retired dialog's commit path). A no-change
    /// exit (open, close, type nothing) applies nothing — Visio does
    /// not record an undo step for it.
    pub(super) fn end_text_edit(&mut self, commit: bool) {
        let Some(sess) = self.text_edit.take() else {
            return;
        };
        sess.restore_view(&mut self.view);
        if !commit {
            return;
        }
        // The click that OPENED the Insert Field dialog commits the
        // edit through this path — and dispatch may run after the
        // overlay in some frame orderings, so the caret is captured
        // HERE too (Greptile round 1: dispatch's own capture can see
        // an already-empty session). The LAST-edit caret is kept
        // unconditionally: the tab click commits before the field
        // button even opens the dialog (CodeRabbit PR review).
        self.last_edit_caret = Some((sess.page, sess.shape, sess.caret));
        if self.fields_open.is_some() && self.fields_caret.is_none() {
            self.fields_caret = Some((sess.page, sess.shape, sess.caret));
        }
        // The shape must still exist to receive the commit — the edit
        // can outlive it (Ctrl+Z of its placement is not keyboard-
        // gated). Resolve first and drop the draft when the page or
        // shape is gone instead of applying SetText at a dead scope
        // (#22 review).
        let current = self
            .ed
            .as_ref()
            .and_then(|ed| ed.doc.pages.iter().find(|p| p.id == sess.page))
            .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, sess.shape))
            .map(|s| s.sheet.text.clone().unwrap_or_default());
        let Some(current) = current else {
            return;
        };
        // #267: parked field insertions commit with the text — the
        // marker lands where each display chunk sits (caret-precise),
        // in the SAME undo step as any typing. With no typing beyond
        // the parked displays, the raw text is untouched and each
        // marker inserts at its plain offset, preserving every
        // existing formatting marker.
        let strip_marks = |draft: &str, marks: &[crate::textedit::FieldMark]| -> String {
            let mut spans: Vec<(usize, usize)> = marks
                .iter()
                .map(|m| (m.offset, m.offset + m.display.chars().count()))
                .collect();
            spans.sort();
            draft
                .chars()
                .enumerate()
                .filter(|(pos, _)| !spans.iter().any(|(a, b)| pos >= a && pos < b))
                .map(|(_, c)| c)
                .collect()
        };
        let typed = strip_marks(&sess.draft, &sess.field_marks) != current;
        if !sess.field_marks.is_empty() || sess.draft != current {
            let scope = Scope::Shape {
                page: sess.page,
                shape: sess.shape,
            };
            let doc = self.ed.as_ref().map(|ed| &ed.doc);
            if let Some(doc) = doc {
                let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
                if typed {
                    // The typed text first: raw = escaped draft (the
                    // parked display chunks are part of it), with the
                    // #147 cache refreshes.
                    cmds.push(Box::new(dc::textfunc::text_edit_macro(
                        doc,
                        &scope,
                        &sess.draft,
                    )));
                }
                // ASCENDING offset order: mark offsets live in DRAFT
                // space (displays included), and each insertion grows
                // the raw's plain projection toward the draft —
                // applying in insertion order would land a later-
                // parked EARLIER field against a plain projection
                // that hasn't caught up (CodeRabbit round 4: X at 2,
                // then Y at 0, must commit "YABXCD" from "ABCD").
                let mut marks = sess.field_marks.clone();
                marks.sort_by_key(|m| m.offset);
                for m in &marks {
                    let len = if typed {
                        m.display.chars().count()
                    } else {
                        // The raw never carried the display — insert
                        // at the plain offset, replacing nothing.
                        0
                    };
                    let mut cmd = dc_core::InsertField::new(
                        scope.clone(),
                        m.value_f.clone(),
                        dc_core::FieldInsert::ReplacePlain {
                            offset: m.offset,
                            len,
                        },
                    )
                    .with_display(m.display.clone());
                    if let Some(f) = &m.format_f {
                        cmd = cmd.with_format(f.clone());
                    }
                    cmds.push(Box::new(cmd));
                }
                if !typed {
                    // No SetText ran, so nothing refreshed the
                    // text-derived caches — the parked displays
                    // changed what renders. The draft IS the post
                    // text (original plain + displays, no typing).
                    cmds.extend(dc::textfunc::text_cache_refreshes(doc, &scope, &sess.draft));
                }
                if cmds.len() == 1 && !typed {
                    // A single field, no typing: the InsertField alone
                    // is the undo step.
                    let cmd = cmds.remove(0);
                    self.apply_boxed(&crate::i18n::strs().msg_insert_field(), cmd);
                } else {
                    let mac = dc_core::Macro::new(
                        crate::i18n::strs().msg_insert_field().to_string(),
                        cmds,
                    );
                    self.apply(mac);
                }
            }
        }
    }

    // -------------------------------------------------------------------
    // Check Spelling (#121) — the domain lives in `spellcheck.rs`; this
    // is the state field's glue: entry, the dialog frame, and the
    // outcome commands.
    // -------------------------------------------------------------------
}
