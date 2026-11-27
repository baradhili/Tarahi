//! Shell chrome: ribbon Action dispatch, window title, the tab
//! strip, ribbon content, page tabs, and the status bar.

use super::*;

impl App {
    pub(super) fn dispatch(&mut self, a: Action) {
        match a {
            Action::NewDoc => self.do_new(),
            Action::Open => self.do_open(),
            Action::Save => self.do_save(false),
            Action::SaveAs => self.do_save(true),
            Action::Undo => self.do_undo(),
            Action::Redo => self.do_redo(),
            Action::NewPage => {
                // #68: like the tab menu's Insert Page, the new sheet
                // copies the active page's setup.
                let name = format!(
                    "Page-{}",
                    self.ed().map(|e| e.doc.pages.len() + 1).unwrap_or(1)
                );
                let add = match self.page_id() {
                    Some(id) => AddPage::seeded_from(name, id),
                    None => AddPage::new(name),
                };
                self.apply(add);
                if let Some(ed) = self.ed() {
                    self.page_idx = ed.doc.pages.len() - 1;
                    self.view.fit_pending = true;
                    self.tab_scroll_to_active = true;
                }
            }
            Action::Delete => self.delete_selection(),
            // Manual zooms leave fit mode: window resizes must not
            // clobber the user's chosen view (Fit Page re-arms it).
            Action::ZoomIn => {
                self.view.zoom = (self.view.zoom * 1.25).clamp(0.1, 8.0);
                self.view.fit_mode = false;
            }
            Action::ZoomOut => {
                self.view.zoom = (self.view.zoom / 1.25).clamp(0.1, 8.0);
                self.view.fit_mode = false;
            }
            Action::Zoom100 => {
                self.view.zoom = 1.0;
                self.view.fit_mode = false;
            }
            Action::FitPage => self.view.fit_pending = true,
            // #209 step 7: Presentation Mode — hide the chrome; the
            // backstage screen is chrome too, so it closes with it.
            // #130 closes the footer-spec §3.4 variance: the mode is
            // FULL screen — entering and leaving drive the viewport
            // command with the chrome.
            Action::PresentationMode => {
                self.chrome_hidden = !self.chrome_hidden;
                if self.chrome_hidden {
                    self.backstage = false;
                }
                self.fullscreen_pending = Some(self.chrome_hidden);
            }
            // #130: the Slide Show — read-only, page-at-a-time
            // presenting; the module owns the entry/exit machinery.
            Action::Slideshow(from) => self.enter_slideshow(from),
            // #209 step 3: Page Width — the width fit is a zoom, not a
            // mode (a later manual zoom or a resize never re-fits).
            Action::FitPageWidth => self.apply_page_width_fit(),
            // The Zoom dialog opens with the current zoom preselected.
            Action::ZoomDialog => {
                self.zoom_dlg_open = !self.zoom_dlg_open;
                if self.zoom_dlg_open {
                    self.zoom_pick =
                        dialogs::ZoomPick::Preset((self.view.zoom * 100.0).round() as u32);
                }
            }
            Action::ZoomSet(pct) => {
                self.view.zoom = (pct as f32 / 100.0).clamp(0.1, 8.0);
                self.view.fit_mode = false;
            }
            Action::About => self.about_open = !self.about_open,
            // #237: zoom-to-selection defers to the canvas frame — it
            // needs the content area + selection rects the frame owns.
            Action::ZoomSelection => self.zoom_selection_pending = true,
            Action::ToggleGrid => self.view.show_grid = !self.view.show_grid,
            Action::ToggleRulers => self.view.show_rulers = !self.view.show_rulers,
            Action::ToggleComments => self.show_comments = !self.show_comments,
            Action::ToggleSizePosition => self.size_pos_open = !self.size_pos_open,
            Action::ToggleDynamicGrid => {
                self.snap.dynamic_grid = !self.snap.dynamic_grid;
                self.apply_snap_settings();
            }
            Action::SnapGlue => self.snap_dlg_open = !self.snap_dlg_open,
            // #209 step 2: AutoConnect is document state like Snap &
            // Glue — the toggle writes through, never into undo history.
            Action::ToggleAutoConnect => {
                self.snap.autoconnect = !self.snap.autoconnect;
                self.apply_snap_settings();
            }
            Action::SpellCheck => self.start_spellcheck(),
            Action::PointerTool => {
                self.tool = Tool::Pointer;
                self.connpt_sel = None;
            }
            Action::AlignLeft => self.align_selection(crate::align::AlignKind::Left),
            Action::AlignHCenter => self.align_selection(crate::align::AlignKind::HCenter),
            Action::AlignRight => self.align_selection(crate::align::AlignKind::Right),
            Action::AlignTop => self.align_selection(crate::align::AlignKind::Top),
            Action::AlignVCenter => self.align_selection(crate::align::AlignKind::VCenter),
            Action::AlignBottom => self.align_selection(crate::align::AlignKind::Bottom),
            Action::AlignToPage => {
                self.align_to_page = !self.align_to_page;
                self.status = if self.align_to_page {
                    crate::i18n::strs().msg_aligning_to_the_page()
                } else {
                    crate::i18n::strs().msg_aligning_to_the_first_selected_s()
                };
            }
            Action::DistributeH => self.distribute_selection(false),
            Action::DistributeV => self.distribute_selection(true),
            Action::SelectAll => self.select_all(),
            Action::SelectByType => {
                self.selby_open = !self.selby_open;
                if self.selby_open {
                    self.status = crate::i18n::strs().msg_select_by_type_pick_a_criterion();
                }
            }
            Action::FormatPainter => self.arm_format_painter(false),
            Action::FormatPainterSticky => self.arm_format_painter(true),
            Action::PageOrientation => self.toggle_page_orientation(),
            Action::PageSizeDialog => self.pagesetup_open = !self.pagesetup_open,
            Action::CopyAsPicture => self.copypic_open = !self.copypic_open,
            Action::InsertPicture => self.open_picture_dialog(),
            Action::PageBackgrounds => self.backgrounds_open = !self.backgrounds_open,
            Action::ConnectorTool => {
                self.tool = Tool::Connector;
                self.status = crate::i18n::strs().msg_connector_tool_drag_from_one_sha();
            }
            Action::ContainerStyle(name) => self.apply_container_style(name),
            Action::ContainerGallery => self.contgal_open = !self.contgal_open,
            Action::ContainerHighlight => self.toggle_container_highlight(),
            Action::ConnPointTool => {
                if self.tool == Tool::ConnectionPoint {
                    self.tool = Tool::Pointer;
                    self.connpt_sel = None;
                    self.status = crate::i18n::strs().msg_connection_point_tool_off();
                } else {
                    self.tool = Tool::ConnectionPoint;
                    self.connpt_sel = None;
                    self.status = crate::i18n::strs().msg_connpoint_tool_status();
                }
            }
            Action::FreeformTool => {
                // Toggle like the other tools; the gesture arms on an
                // empty-page press and the release builds the shape.
                if self.tool == Tool::Freeform {
                    self.tool = Tool::Pointer;
                    self.status = crate::i18n::strs().msg_freeform_off();
                } else {
                    self.tool = Tool::Freeform;
                    self.status = crate::i18n::strs().msg_freeform_drag_on_the_page_to_dra();
                }
            }
            Action::PanTool => {
                // #283: toggle like the other tools; a plain drag pans,
                // presses arm nothing, clicks change nothing.
                if self.tool == Tool::Pan {
                    self.tool = Tool::Pointer;
                    self.status = crate::i18n::strs().msg_pan_off();
                } else {
                    self.tool = Tool::Pan;
                    self.status = crate::i18n::strs().msg_pan_tip();
                }
            }
            Action::LassoTool => {
                // Toggle like Visio's Select ▸ Lasso Select menu item;
                // the Pointer Tool (or Esc) is Select Area.
                if self.tool == Tool::Lasso {
                    self.tool = Tool::Pointer;
                    self.status = crate::i18n::strs().msg_lasso_select_off();
                } else {
                    self.tool = Tool::Lasso;
                    self.status = crate::i18n::strs().msg_lasso_select_drag_a_freehand_tra();
                }
            }
            Action::ToggleUnits => {
                self.view.unit = if self.view.unit == crate::units::Unit::Mm {
                    crate::units::Unit::In
                } else {
                    crate::units::Unit::Mm
                };
            }
            Action::ShowShapeSheet => {
                // #192 spec: per-shape sheets — the button targets the
                // selection's sheet (toggling); without a selection it
                // closes every open sheet.
                if let Some((_, sid)) = self.sel {
                    self.toggle_shapesheet(crate::shapesheet::SheetTarget::Shape(sid));
                } else {
                    self.shapesheets.clear();
                }
            }
            // #272: Visio's Show ShapeSheet ▸ Page / Document.
            Action::ShowPageSheet => {
                if let Some(page_id) = self.page_id() {
                    self.toggle_shapesheet(crate::shapesheet::SheetTarget::Page(page_id));
                }
            }
            Action::ShowDocumentSheet => {
                self.toggle_shapesheet(crate::shapesheet::SheetTarget::Document);
            }
            Action::Macros => {
                self.status = crate::i18n::strs().msg_macros_arrive_with_m7_vba_vsdm_m()
            }
            Action::FillMenu | Action::LineMenu | Action::TextColorMenu => {
                self.color_pick = Some(match a {
                    Action::FillMenu => ColorTarget::Fill,
                    Action::LineMenu => ColorTarget::Line,
                    // #150: the ribbon Text color button rides #73's
                    // whole-selection Character row-0 write.
                    _ => ColorTarget::Text,
                });
            }
            Action::FontGrow => self.nudge_char_size(1.0),
            Action::FontShrink => self.nudge_char_size(-1.0),
            Action::RotateLeft => self.rotate_selection(90.0),
            Action::RotateRight => self.rotate_selection(-90.0),
            Action::Group => self.group_selection(),
            Action::ContainerDisband => self.disband_container(),
            Action::Ungroup => self.ungroup_selection(),
            Action::Hyperlink => {
                // #252 Insert ▸ Links: the Hyperlinks dialog (#28/#71)
                // on the primary selection; no selection notes it.
                match self.sel {
                    Some((_, sid)) => {
                        self.hyperlink_open = Some(sid);
                        self.hyperlink_draft = None;
                    }
                    None => {
                        self.status = crate::i18n::strs().msg_select_a_shape_to_edit_its_hyper()
                    }
                }
            }
            Action::InsertField => {
                // #267 Insert ▸ Text: the Insert Field dialog on the
                // primary selection — caret-precise when the in-place
                // text editor is open on it. The click that opened the
                // dialog also commits the edit (#22's click-away), so
                // the caret is captured NOW — the insertion lands
                // where the caret sat, in the committed raw.
                self.fields_caret = self
                    .text_edit
                    .as_ref()
                    .map(|s| (s.page, s.shape, s.caret))
                    .or_else(|| {
                        // The edit already committed (tab click): the
                        // last caret of THIS shape carries.
                        let sel = self.sel?;
                        let (p, s, caret) = self.last_edit_caret?;
                        (p == sel.0 && s == sel.1).then_some((p, s, caret))
                    });
                match self.sel {
                    Some((page_id, sid)) => {
                        self.fields_open = Some((page_id, sid));
                        self.fields_draft = None;
                    }
                    None => {
                        self.status = crate::i18n::strs().msg_select_a_shape_to_insert_a_field()
                    }
                }
            }
        }
    }

    /// #209 step 3: Visio's Page Width — zoom to the page's width
    /// against the current content area (the last canvas frame's
    /// layout; a no-op before the first frame or without a document).
    pub(super) fn apply_page_width_fit(&mut self) {
        if self.ed.is_none() || self.canvas_rect.width() <= 0.0 {
            return;
        }
        let (origin, content) = canvas::content_area(self.canvas_rect, self.view.show_rulers);
        let geom = self.page_geom();
        self.view.apply_fit_width(content, origin.to_vec2(), &geom);
    }

    // -- shell panels -----------------------------------------------------

    /// #67 (option D, owner 2026-10-28): the OS window draws the
    /// title — the app band that duplicated it is gone, and the
    /// document title (name + dirty marker) is pushed to the OS
    /// window on change instead. Headless tests assert through the
    /// tracked field; the customize chevron is dropped with the band
    /// (Visio's opens a real menu ours never had).
    pub(super) fn sync_window_title(&mut self, ctx: &egui::Context) {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::strs().msg_drawing_n(1));
        // The OS title bar is a NATIVE surface (#280): the WM's font
        // has no glyphs for Fluent's bidi-isolation marks and paints
        // them as tofu — strip them at the boundary
        // (`i18n::to_native_surface`).
        let dirty_mark = if self.dirty { " *" } else { "" };
        let title = crate::i18n::to_native_surface(&format!("{name} - Tarahi{dirty_mark}"));
        if self.window_title != title {
            self.window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    pub(super) fn tab_strip(&mut self, ui: &mut Ui) {
        // Visio 2016 "Colorful" shell: a white strip where the File tab
        // is a green block running flush up into the green title band
        // (square top, rounded bottom corners — taller than the text
        // tabs), the active tab carries the accent underline, and
        // Office's "Tell me" search box sits at the strip's right end.
        const STRIP_H: f32 = 28.0;
        // `tk`: the for-loop below binds `t` for each Tab.
        let tk = theme::shell_tokens(ui.visuals());
        egui::Panel::top("tab_strip")
            .frame(
                egui::Frame::new()
                    .fill(tk.ribbon_content)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ui, |ui| {
                // #257 item 3: RTL mirrors the strip — File lands at
                // the right edge and the tabs flow leftward.
                let strip_dir = if crate::i18n::rtl() {
                    egui::Layout::right_to_left(egui::Align::Center)
                } else {
                    egui::Layout::left_to_right(egui::Align::Center)
                };
                ui.with_layout(strip_dir, |ui| {
                    ui.set_min_height(STRIP_H);
                    // File: a green block spanning the full strip height
                    // — the green title band above makes it read as one
                    // tall tab, like the Office File tab.
                    let (fr, fresp) =
                        ui.allocate_exact_size(Vec2::new(54.0, STRIP_H), Sense::click());
                    // The painted text never reaches the accessibility
                    // tree on its own (GitHub #56).
                    fresp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            crate::i18n::strs().msg_file(),
                        )
                    });
                    ui.painter().rect_filled(
                        fr,
                        egui::CornerRadius {
                            nw: 0,
                            ne: 0,
                            se: 2,
                            sw: 2,
                        },
                        tk.accent_band,
                    );
                    ui.painter().text(
                        fr.center(),
                        egui::Align2::CENTER_CENTER,
                        crate::i18n::strs().msg_file(),
                        egui::FontId::proportional(12.5),
                        Color32::WHITE,
                    );
                    if fresp.clicked() {
                        self.open_file_backstage();
                    }
                    // #184: the File badge (Office letter F).
                    self.keytips.badges.push(crate::keytips::Badge {
                        rect: fr,
                        key: "F".into(),
                        hit: crate::keytips::Hit::File,
                        anchor: None,
                    });
                    ui.add_space(8.0);
                    // #156: the contextual Container Tools tab rides
                    // after the fixed strip while a container is the
                    // primary selection (Visio 2016).
                    let sel_is_container = self.sel_is_container();
                    for t in TABS
                        .iter()
                        .chain(sel_is_container.then_some(&Tab::ContainerTools))
                    {
                        if *t == Tab::File {
                            continue;
                        }
                        let active = *t == self.tab;
                        let txt = egui::RichText::new(t_title(*t))
                            .size(12.0)
                            .color(if active {
                                tk.accent_dark
                            } else {
                                tk.tab_strip_text
                            });
                        let b = egui::Button::new(txt).small().frame(false);
                        let resp = ui.add_enabled(true, b);
                        if resp.clicked() {
                            self.tab = *t;
                        }
                        let r = resp.rect;
                        // #184: every strip tab badges its Office
                        // letter while keytips are armed.
                        if let Some(k) = crate::keytips::tab_keytip(*t) {
                            self.keytips.badges.push(crate::keytips::Badge {
                                rect: r,
                                key: k.into(),
                                hit: crate::keytips::Hit::Tab(*t),
                                anchor: None,
                            });
                        }
                        if active {
                            ui.painter().hline(
                                r.left() + 2.0..=r.right() - 2.0,
                                r.bottom(),
                                Stroke::new(2.0, tk.accent),
                            );
                        }
                        ui.add_space(6.0);
                    }
                    // #192 4a: the Window switcher (classic MDI
                    // position) — every open document, the active one
                    // marked; clicking activates.
                    let n_docs = self.docs.len() + self.ed.is_some() as usize;
                    if n_docs > 0 {
                        ui.add_space(8.0);
                        let label = crate::i18n::strs().msg_window_n(n_docs);
                        // (no widget_info override — the Button's own
                        // text is the accessible label, #56's concern is
                        // painted text only)
                        let wresp = ui.add(
                            egui::Button::new(egui::RichText::new(label).size(12.0)).frame(false),
                        );
                        if wresp.clicked() {
                            self.winmenu_open = !self.winmenu_open;
                        }
                        if self.winmenu_open {
                            let mut close_menu = false;
                            egui::Popup::menu(&wresp).show(|ui| {
                                ui.set_min_width(220.0);
                                // The ACTIVE document first (marked).
                                if self.ed.is_some() {
                                    let name = self
                                        .path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
                                    let mark = if self.dirty { " •" } else { "" };
                                    ui.add_enabled(
                                        false,
                                        egui::Label::new(
                                            // (› — Carlito has no ✓ glyph;
                                            // the breadcrumb's arrow)
                                            egui::RichText::new(format!("› {name}{mark}")).strong(),
                                        ),
                                    );
                                }
                                for (ix, d) in self.docs.iter().enumerate() {
                                    let name = d
                                        .path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
                                    let mark = if d.dirty { " •" } else { "" };
                                    if ui.button(format!("  {name}{mark}")).clicked() {
                                        self.pending_activate = Some(ix);
                                        close_menu = true;
                                    }
                                }
                            });
                            if close_menu {
                                self.winmenu_open = false;
                            }
                        }
                    }
                    // Office's "Tell me what you want to do" search box.                    // Office's "Tell me what you want to do" search
                    // box. The inner layout flips with the strip so it
                    // hugs the mirrored far end.
                    let tellme_dir = if crate::i18n::rtl() {
                        egui::Layout::left_to_right(egui::Align::Center)
                    } else {
                        egui::Layout::right_to_left(egui::Align::Center)
                    };
                    ui.with_layout(tellme_dir, |ui| {
                        ui.add_space(6.0);
                        let (pr, presp) =
                            ui.allocate_exact_size(Vec2::new(196.0, 20.0), Sense::hover());
                        let hot = presp.hovered();
                        ui.painter().rect_filled(pr, 2, tk.tellme_bg);
                        ui.painter().rect_stroke(
                            pr,
                            2,
                            Stroke::new(1.0, if hot { tk.ruler_tick } else { tk.ribbon_edge }),
                            egui::StrokeKind::Middle,
                        );
                        Icon::TellMe.paint(
                            ui,
                            Rect::from_min_size(
                                pr.left_top() + Vec2::new(5.0, 3.0),
                                Vec2::splat(14.0),
                            ),
                            true,
                        );
                        // The QAT cluster lands LEFT of Tell Me (this is
                        // a right-to-left layout): Save/Undo/Redo as
                        // window-level document controls on the menu
                        // strip's right (#67 option D — with no menu
                        // bar they are the only always-visible command
                        // surface; the customize chevron is dropped
                        // with the old title band).
                        ui.add_space(10.0);
                        // #184: the digit badges ride the QAT buttons
                        // themselves (1..=9 by position, Office's
                        // rule).
                        qat_button(
                            ui,
                            Icon::Save,
                            &crate::i18n::strs().msg_save_ctrl_s(),
                            || Action::Save,
                            self,
                            1,
                        );
                        qat_button(
                            ui,
                            Icon::Undo,
                            &crate::i18n::strs().msg_undo_ctrl_z(),
                            || Action::Undo,
                            self,
                            2,
                        );
                        qat_button(
                            ui,
                            Icon::Redo,
                            &crate::i18n::strs().msg_redo_ctrl_y(),
                            || Action::Redo,
                            self,
                            3,
                        );
                        // #242: Help lives in the QAT (the owner's
                        // placement) — opens the About window.
                        qat_button(
                            ui,
                            Icon::Help,
                            &crate::i18n::strs().msg_help_about(),
                            || Action::About,
                            self,
                            4,
                        );
                        // #204 step 4: a REAL query field in the same
                        // chrome — typing feeds the live search,
                        // gaining focus opens the panel, and Alt+Q /
                        // the Q keytip land here from the input path.
                        // The placeholder stays PAINTED (not the
                        // TextEdit's own hint): egui's hint-text
                        // rendering varies per OS at the anti-alias
                        // level, and the exact-match i18n goldens
                        // would not survive it (the macOS CI round of
                        // PR #409); the painter galley is the
                        // deterministic path the old box used.
                        let field_r = Rect::from_min_max(
                            Pos2::new(pr.left() + 22.0, pr.top()),
                            Pos2::new(pr.right() - 4.0, pr.bottom()),
                        );
                        let editor = egui::TextEdit::singleline(&mut self.tellme_query)
                            .id(crate::tellme::field_id())
                            .frame(egui::Frame::NONE)
                            .font(egui::FontId::proportional(11.0))
                            .desired_width(field_r.width());
                        // `put` advances the mirrored layout's cursor
                        // past the field rect — the field therefore
                        // lives at the END of this closure, after the
                        // QAT, so that advance has nothing left to
                        // crowd (the QAT shift it caused was caught by
                        // the macOS CI golden round of PR #409).
                        let fresp = ui.put(field_r, editor);
                        if self.tellme_query.is_empty() {
                            ui.painter().text(
                                Pos2::new(pr.left() + 24.0, pr.center().y),
                                egui::Align2::LEFT_CENTER,
                                crate::i18n::strs().msg_tell_me_what_you_want_to_do(),
                                egui::FontId::proportional(11.0),
                                tk.hint_text,
                            );
                        }
                        // Name the field for screen readers — a real
                        // TextEdit keeps its own role; the label keeps
                        // the a11y lookups (#56) keying on it.
                        fresp.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::TextInput,
                                true,
                                crate::i18n::strs().msg_tell_me(),
                            )
                        });
                        let fresp =
                            fresp.on_hover_text(crate::i18n::strs().msg_tell_me_tip());
                        if fresp.gained_focus() {
                            self.tellme_open = true;
                            self.tellme_sel = 0;
                        }
                        // #204 (CodeRabbit PR review): an Enter seen by
                        // the input path is CAPTURED there (before this
                        // field updates the query) and resolved HERE,
                        // against the query this frame's typing just
                        // produced — replace-text-and-Enter in one
                        // frame must run the NEW query's top hit, not
                        // the stale one.
                        if self.tellme_enter {
                            self.tellme_enter = false;
                            let rows = self.tellme_rows();
                            self.tellme_sel = self.tellme_sel.min(rows.len().saturating_sub(1));
                            if let Some(row) = rows.get(self.tellme_sel) {
                                match row {
                                    crate::tellme::PanelRow::Result(e) if e.enabled => {
                                        self.tellme_open = false;
                                        self.tellme_query.clear();
                                        self.tellme_sel = 0;
                                        ui.ctx().memory_mut(|m| {
                                            m.surrender_focus(crate::tellme::field_id())
                                        });
                                        self.dispatch(e.action);
                                    }
                                    crate::tellme::PanelRow::Try { query, .. } => {
                                        self.tellme_query = query.to_string();
                                        self.tellme_sel = 0;
                                    }
                                    // The Get-Help footer: open the
                                    // stub with the query (#204 step
                                    // 7) and close the panel.
                                    crate::tellme::PanelRow::Help { .. } => {
                                        self.tellme_help = Some(self.tellme_query.clone());
                                        self.tellme_open = false;
                                        self.tellme_query.clear();
                                        self.tellme_sel = 0;
                                        ui.ctx().memory_mut(|m| {
                                            m.surrender_focus(crate::tellme::field_id())
                                        });
                                    }
                                    // A faded row does nothing (the
                                    // acceptance rule).
                                    crate::tellme::PanelRow::Result(_) => {}
                                }
                            }
                        }
                        // #184: Tell Me badges Q (Office's Alt+Q).
                        self.keytips.badges.push(crate::keytips::Badge {
                            rect: pr,
                            key: "Q".into(),
                            hit: crate::keytips::Hit::TellMe,
                            anchor: None,
                        });
                        // #204 step 3: the results panel — a menu
                        // popup anchored under the box, rebuilt per
                        // frame from the same snapshot the ribbon
                        // renders from. Rows carry the button's own
                        // icon + label in the display language;
                        // disabled commands render faded and never
                        // dispatch. The keyboard machine (arrows /
                        // Enter / Esc) lives in `handle_shortcuts`;
                        // the row layout mirrors under RTL like the
                        // box's own inner layout does.
                        if self.tellme_open {
                            let rows = self.tellme_rows();
                            let idle = self.tellme_query.trim().is_empty();
                            // #204 step 7: an unmatched query keeps the
                            // explicit "No matching commands" row above
                            // the help footer (the acceptance's rule).
                            let no_results = !idle
                                && rows.iter().all(|r| {
                                    !matches!(r, crate::tellme::PanelRow::Result(_))
                                });
                            self.tellme_sel = self.tellme_sel.min(rows.len().saturating_sub(1));
                            let sel = self.tellme_sel;
                            let rtl = crate::i18n::rtl();
                            let mut run: Option<Action> = None;
                            let mut drill: Option<&'static str> = None;
                            // #204 step 7: the Get-Help footer was
                            // picked — open the stub with the query.
                            let mut help = false;
                            let mut close = false;
                            let mut hover: Option<usize> = None;
                            // A click on a faded row keeps the session
                            // editable (CodeRabbit PR review): egui
                            // surrenders the field's focus on pointer
                            // clicks elsewhere, so an explicit
                            // re-request follows the no-dispatch.
                            let mut refocus = false;
                            // `open_bool` drives egui's popup memory
                            // from OUR flag (the field, not a button,
                            // anchors the panel) and writes egui's
                            // outside-click close back into it. The
                            // close behavior is OUTSIDE-clicks only:
                            // a menu's default closes on ANY click,
                            // which would kill the Try drill-down
                            // (result rows close themselves on run).
                            let mut open = self.tellme_open;
                            egui::Popup::menu(&fresp)
                                .open_bool(&mut open)
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show(|ui| {
                                ui.set_min_width(240.0);
                                let t = theme::menu_tokens(ui.visuals());
                                if idle {
                                    // The idle category header (spec
                                    // 3.1's styled "Try" header) — a
                                    // painted label, so name it (#56).
                                    let (hr, hresp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), 18.0),
                                        Sense::hover(),
                                    );
                                    hresp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Label,
                                            true,
                                            crate::i18n::strs().msg_try_header(),
                                        )
                                    });
                                    let hx = if rtl {
                                        hr.left() + 4.0
                                    } else {
                                        hr.right() - 4.0
                                    };
                                    ui.painter().text(
                                        Pos2::new(hx, hr.center().y),
                                        if rtl {
                                            egui::Align2::LEFT_CENTER
                                        } else {
                                            egui::Align2::RIGHT_CENTER
                                        },
                                        crate::i18n::strs().msg_try_header(),
                                        egui::FontId::proportional(10.0),
                                        t.hint,
                                    );
                                }
                                if no_results {
                                    // The explicit empty state (spec
                                    // 3.2's results view with nothing
                                    // to show) — informational, not a
                                    // pick; named for screen readers.
                                    let (nr, nresp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), 20.0),
                                        Sense::hover(),
                                    );
                                    nresp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Label,
                                            false,
                                            crate::i18n::strs().msg_no_matching_commands(),
                                        )
                                    });
                                    let nx = if rtl {
                                        nr.right() - 6.0
                                    } else {
                                        nr.left() + 6.0
                                    };
                                    ui.painter().text(
                                        Pos2::new(nx, nr.center().y),
                                        if rtl {
                                            egui::Align2::RIGHT_CENTER
                                        } else {
                                            egui::Align2::LEFT_CENTER
                                        },
                                        crate::i18n::strs().msg_no_matching_commands(),
                                        egui::FontId::proportional(11.5),
                                        t.label_disabled,
                                    );
                                }
                                for (i, row) in rows.iter().enumerate() {
                                    // Spec 4.2's divider: the help
                                    // footer separates from the
                                    // actionable commands above it.
                                    if matches!(row, crate::tellme::PanelRow::Help { .. }) {
                                        let (dr, _) = ui.allocate_exact_size(
                                            Vec2::new(ui.available_width(), 1.0),
                                            Sense::hover(),
                                        );
                                        ui.painter().hline(
                                            dr.left() + 4.0..=dr.right() - 4.0,
                                            dr.center().y,
                                            Stroke::new(1.0, t.separator),
                                        );
                                    }
                                    let (r, resp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), 22.0),
                                        Sense::click(),
                                    );
                                    // Painted galley rows — name each
                                    // for screen readers (#56; the
                                    // issue's step-9 a11y walk).
                                    resp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            matches!(row, crate::tellme::PanelRow::Try { .. })
                                                || matches!(row, crate::tellme::PanelRow::Help { .. })
                                                || matches!(row, crate::tellme::PanelRow::Result(e) if e.enabled),
                                            row_label(row),
                                        )
                                    });
                                    if resp.hovered() {
                                        hover = Some(i);
                                    }
                                    let selected = i == sel || resp.hovered();
                                    if selected {
                                        ui.painter().rect_filled(r, 2.0, t.hover_fill);
                                    }
                                    let label_color = match row {
                                        crate::tellme::PanelRow::Result(e) if e.enabled => t.label,
                                        crate::tellme::PanelRow::Result(_) => t.label_disabled,
                                        // Try rows are plain prompts —
                                        // always actionable — and the
                                        // help footer always opens its
                                        // stub.
                                        crate::tellme::PanelRow::Try { .. } => t.label,
                                        crate::tellme::PanelRow::Help { .. } => t.label,
                                    };
                                    match row {
                                        crate::tellme::PanelRow::Result(e) => {
                                            // The mirrored row: icon at
                                            // the reading start,
                                            // shortcut hint at the
                                            // reading end.
                                            let icon_r = Rect::from_min_size(
                                                if rtl {
                                                    r.right_top() + Vec2::new(-20.0, 2.0)
                                                } else {
                                                    r.left_top() + Vec2::new(2.0, 2.0)
                                                },
                                                Vec2::splat(18.0),
                                            );
                                            e.icon.paint(ui, icon_r, e.enabled);
                                            let (tx, ta) = if rtl {
                                                (
                                                    Pos2::new(icon_r.left() - 4.0, r.center().y),
                                                    egui::Align2::RIGHT_CENTER,
                                                )
                                            } else {
                                                (
                                                    Pos2::new(icon_r.right() + 4.0, r.center().y),
                                                    egui::Align2::LEFT_CENTER,
                                                )
                                            };
                                            ui.painter().text(
                                                tx,
                                                ta,
                                                &e.label,
                                                egui::FontId::proportional(11.5),
                                                label_color,
                                            );
                                            if let Some(sc) = e.shortcut {
                                                let (hx, ha) = if rtl {
                                                    (
                                                        Pos2::new(r.left() + 4.0, r.center().y),
                                                        egui::Align2::LEFT_CENTER,
                                                    )
                                                } else {
                                                    (
                                                        Pos2::new(r.right() - 4.0, r.center().y),
                                                        egui::Align2::RIGHT_CENTER,
                                                    )
                                                };
                                                ui.painter().text(
                                                    hx,
                                                    ha,
                                                    sc,
                                                    egui::FontId::proportional(10.0),
                                                    t.hint,
                                                );
                                            }
                                            if resp.clicked() && e.enabled {
                                                run = Some(e.action);
                                                close = true;
                                            } else if resp.clicked() {
                                                refocus = true;
                                            }
                                        }
                                        crate::tellme::PanelRow::Try { label, query } => {
                                            // A plain prompt row: text at
                                            // the reading start, no icon
                                            // (Office's Try items).
                                            let (tx, ta) = if rtl {
                                                (
                                                    Pos2::new(r.right() - 6.0, r.center().y),
                                                    egui::Align2::RIGHT_CENTER,
                                                )
                                            } else {
                                                (
                                                    Pos2::new(r.left() + 6.0, r.center().y),
                                                    egui::Align2::LEFT_CENTER,
                                                )
                                            };
                                            ui.painter().text(
                                                tx,
                                                ta,
                                                label,
                                                egui::FontId::proportional(11.5),
                                                label_color,
                                            );
                                            if resp.clicked() {
                                                drill = Some(query);
                                            }
                                        }
                                        crate::tellme::PanelRow::Help { label } => {
                                            // The footer row: our own
                                            // help glyph + the
                                            // localized "Get Help on
                                            // '<query>'" text, laid out
                                            // like a result row.
                                            let icon_r = Rect::from_min_size(
                                                if rtl {
                                                    r.right_top() + Vec2::new(-20.0, 2.0)
                                                } else {
                                                    r.left_top() + Vec2::new(2.0, 2.0)
                                                },
                                                Vec2::splat(18.0),
                                            );
                                            Icon::Help.paint(ui, icon_r, true);
                                            let (tx, ta) = if rtl {
                                                (
                                                    Pos2::new(icon_r.left() - 4.0, r.center().y),
                                                    egui::Align2::RIGHT_CENTER,
                                                )
                                            } else {
                                                (
                                                    Pos2::new(icon_r.right() + 4.0, r.center().y),
                                                    egui::Align2::LEFT_CENTER,
                                                )
                                            };
                                            ui.painter().text(
                                                tx,
                                                ta,
                                                label,
                                                egui::FontId::proportional(11.5),
                                                label_color,
                                            );
                                            if resp.clicked() {
                                                help = true;
                                            }
                                        }
                                    }
                                }
                            });
                            // A caret-positioning click in the query is
                            // NOT a dismissal (CodeRabbit review): the
                            // field is part of the panel, so a click
                            // egui counted as "outside the popup body"
                            // keeps it open — only clicks outside BOTH
                            // close it.
                            if !open && fresp.clicked() {
                                open = true;
                            }
                            self.tellme_open = open;
                            if let Some(i) = hover {
                                self.tellme_sel = i;
                            }
                            if let Some(q) = drill {
                                // A Try pick drills down: the query
                                // populates and the results show (#204
                                // step 6, spec 3.1). Re-focus the
                                // field (CodeRabbit PR review): egui
                                // surrenders focus on a pointer click
                                // outside the focused widget, and the
                                // populated query must stay editable.
                                self.tellme_query = q.to_string();
                                self.tellme_sel = 0;
                                ui.ctx().memory_mut(|m| {
                                    m.request_focus(crate::tellme::field_id())
                                });
                            }
                            if refocus {
                                // A faded-row click dispatched nothing
                                // and kept the panel open — put the
                                // keyboard back in the query.
                                ui.ctx().memory_mut(|m| {
                                    m.request_focus(crate::tellme::field_id())
                                });
                            }
                            if help {
                                // The Get-Help footer was picked: the
                                // panel closes and the stub window
                                // opens pre-filled with the query
                                // (#204 step 7, spec 3.3.3/5).
                                self.tellme_help = Some(self.tellme_query.clone());
                                self.tellme_open = false;
                                self.tellme_query.clear();
                                self.tellme_sel = 0;
                                ui.ctx().memory_mut(|m| {
                                    m.surrender_focus(crate::tellme::field_id())
                                });
                            }
                            if close || !open {
                                self.tellme_open = false;
                                self.tellme_query.clear();
                                self.tellme_sel = 0;
                                // Running a command (or clicking away)
                                // returns focus to the canvas (#204
                                // steps 3–4).
                                ui.ctx().memory_mut(|m| {
                                    m.surrender_focus(crate::tellme::field_id())
                                });
                            }
                            if let Some(a) = run {
                                self.dispatch(a);
                            }
                        }

                    });
                });
                // #184: the armed veil covers the strip's rect
                // (unioned with the content row's at paint time).
                self.keytips.strip_rect = Some(ui.min_rect());
            });
        // #192 4a: a Window-menu pick activates after the strip's
        // mutable borrows end.
        if let Some(ix) = self.pending_activate.take() {
            self.activate_doc(ix);
        }
    }

    pub(super) fn ribbon_content(&mut self, ui: &mut Ui) {
        let mut act: Option<Action> = None;
        let dispatch = |a: Action, act: &mut Option<Action>| {
            *act = Some(a);
        };
        // The contextual tab leaves when its context does (#156).
        if self.tab == Tab::ContainerTools && !self.sel_is_container() {
            self.tab = Tab::Home;
        }
        // One snapshot per frame; the per-tab modules (#252) own the
        // layouts and never borrow the app.
        let ctx = self.ribbon_ctx();
        let groups = crate::ribbon::groups(self.tab, &ctx);
        let mut launcher: Option<String> = None;
        // #184: every content-row control captures its rect for the
        // keytip badges (letters assigned at paint time). #257: under
        // an RTL language the badges also carry the control's ENGLISH
        // anchor label — the Latin name its keytip letter keys off
        // (the RTL Office scheme; the localized label has no ASCII
        // candidates) — zipped in content_row's traversal order.
        let anchors = if crate::i18n::rtl() {
            crate::ribbon::anchor_labels(self.tab, &ctx)
        } else {
            Vec::new()
        };
        let mut anchor_ix = 0usize;
        let badges = &mut self.keytips.badges;
        crate::ribbon::content_row(
            ui,
            &groups,
            &mut |a| dispatch(a, &mut act),
            &mut |label: &str| launcher = Some(label.to_string()),
            &mut |b, r| {
                let anchor = anchors.get(anchor_ix).cloned();
                anchor_ix += 1;
                badges.push(crate::keytips::Badge {
                    rect: r,
                    key: String::new(),
                    hit: crate::keytips::Hit::Action {
                        action: b.action,
                        enabled: b.enabled,
                        label: b.a11y_label().into(),
                    },
                    anchor,
                });
            },
        );
        self.keytips.content_rect = Some(ui.max_rect());
        if let Some(label) = launcher {
            self.status = crate::i18n::strs().msg_label_dialog_planned_for_a_later(label);
        }
        if let Some(a) = act {
            self.dispatch(a);
        }
        ui.add_space(2.0);
    }

    /// #204 step 3: the Tell Me panel's rows — the flattened registry
    /// searched with the live query, or the idle Try list while the
    /// query is empty (step 6). Rebuilt per frame while the panel is
    /// open (the same snapshot cost class as the keytips path), never
    /// cached.
    pub(super) fn tellme_rows(&self) -> Vec<crate::tellme::PanelRow> {
        crate::tellme::rows(&self.ribbon_ctx(), &self.tellme_query)
    }

    /// The per-frame ribbon snapshot (#252): every fact a tab's group
    /// builder needs, computed once so the per-tab modules stay pure
    /// layout.
    pub(super) fn ribbon_ctx(&self) -> crate::ribbon::TabCtx {
        let sel_count = self
            .sel_set
            .iter()
            .filter(|(p, _)| Some(*p) == self.page_id())
            .count();
        crate::ribbon::TabCtx {
            has_doc: self.ed.is_some(),
            has_sel: self.sel.is_some(),
            multi_sel: sel_count >= 2,
            tri_sel: sel_count >= 3,
            tool: self.tool,
            painter_armed: self.painter_armed.is_some(),
            selby_open: self.selby_open,
            align_to_page: self.align_to_page,
            pagesetup_open: self.pagesetup_open,
            backgrounds_open: self.backgrounds_open,
            contgal_open: self.contgal_open,
            show_rulers: self.view.show_rulers,
            show_grid: self.view.show_grid,
            show_comments: self.show_comments,
            size_pos_open: self.size_pos_open,
            dynamic_grid: self.snap.dynamic_grid,
            snap_dlg_open: self.snap_dlg_open,
            autoconnect: self.snap.autoconnect,
            chrome_hidden: self.chrome_hidden,
            shapesheet_open: self.sel.is_some_and(|(_, sid)| {
                self.shapesheets
                    .contains(&crate::shapesheet::SheetTarget::Shape(sid))
            }),
            page_sheet_open: self.page_id().is_some_and(|p| {
                self.shapesheets
                    .contains(&crate::shapesheet::SheetTarget::Page(p))
            }),
            doc_sheet_open: self
                .shapesheets
                .contains(&crate::shapesheet::SheetTarget::Document),
            sel_group: self.selection_has_group(),
            sel_container: self.sel_is_container(),
            container_highlight: self.container_highlight_on(),
        }
    }

    /// Whether any selected shape on the current page is a group
    /// (#252 Home ▸ Arrange Ungroup enablement).
    pub(super) fn selection_has_group(&self) -> bool {
        let Some(page_id) = self.page_id() else {
            return false;
        };
        let Some(ed) = self.ed.as_ref() else {
            return false;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return false;
        };
        self.sel_set.iter().any(|(p, sid)| {
            *p == page_id
                && vsdx::model::find_shape_by_id(&page.shapes, *sid)
                    .is_some_and(|sh| !sh.sub_shapes.is_empty())
        })
    }

    pub(super) fn page_tabs(&mut self, ui: &mut Ui) {
        let t = theme::shell_tokens(ui.visuals());
        // #257 item 3: RTL mirrors the strip — the navigation cluster
        // hugs the right edge and the tabs flow leftward.
        let dir = if crate::i18n::rtl() {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        ui.with_layout(dir, |ui| {
            ui.add_space(4.0);
            let tabs: Vec<(String, i64)> = self
                .ed
                .as_ref()
                .map(|ed| {
                    ed.doc
                        .pages
                        .iter()
                        .map(|p| (p.name.clone(), p.id))
                        .collect()
                })
                .unwrap_or_default();
            if tabs.is_empty() {
                return; // no document: Visio shows no strip at all
            }
            // The Visio 2010 navigation cluster at the strip's left end:
            // << first, < previous, > next, >> last (page navigation, not
            // strip scrolling — owner-confirmed against the shipped
            // reference, #54).
            let n = tabs.len();
            let page_idx = self.page_idx.min(n.saturating_sub(1));
            let mut nav = |ui: &mut Ui, glyph: &str, tip: &str, enabled: bool, to: usize| {
                let b =
                    egui::Button::new(egui::RichText::new(glyph).size(10.0).color(if enabled {
                        t.tab_text_active
                    } else {
                        t.tab_text_inactive
                    }))
                    .small()
                    .fill(egui::Color32::TRANSPARENT)
                    .frame(false);
                let clicked = ui.add_enabled(enabled, b).on_hover_text(tip).clicked();
                if clicked {
                    self.switch_page(to);
                }
            };
            nav(
                ui,
                "<<",
                &crate::i18n::strs().msg_first_page(),
                page_idx > 0,
                0,
            );
            nav(
                ui,
                "<",
                &crate::i18n::strs().msg_previous_page(),
                page_idx > 0,
                page_idx.saturating_sub(1),
            );
            nav(
                ui,
                ">",
                &crate::i18n::strs().msg_next_page(),
                page_idx + 1 < n,
                (page_idx + 1).min(n - 1),
            );
            nav(
                ui,
                ">>",
                &crate::i18n::strs().msg_last_page(),
                page_idx + 1 < n,
                n - 1,
            );
            ui.separator();
            // Tabs fill the strip; overflow clips and the nav cluster /
            // wheel scrolling reach the rest (Visio shows no scrollbar).
            let scroll_to_active = self.tab_scroll_to_active;
            self.tab_scroll_to_active = false;
            egui::ScrollArea::horizontal()
                .id_salt("dc-page-tabs")
                .max_height(24.0)
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, (name, id)) in tabs.iter().enumerate() {
                            let active = i == self.page_idx;
                            // Inline rename (#54): the tab becomes an
                            // editor on double-click / Rename; Enter or
                            // focus loss commits, Escape cancels.
                            let resp = if self.tab_rename.as_ref().is_some_and(|(rid, _)| rid == id)
                            {
                                let r = ui.add_sized(
                                    [96.0, 18.0],
                                    egui::TextEdit::singleline(
                                        &mut self.tab_rename.as_mut().unwrap().1,
                                    )
                                    .font(egui::TextStyle::Small)
                                    .hint_text(crate::i18n::strs().msg_page_name()),
                                );
                                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if escaped {
                                    self.tab_rename = None;
                                } else if entered || r.lost_focus() {
                                    let new = self
                                        .tab_rename
                                        .take()
                                        .map(|(_, d)| d.trim().to_string())
                                        .unwrap_or_default();
                                    if !new.is_empty() && new != *name {
                                        self.apply(RenamePage::new(*id, new));
                                    }
                                }
                                r
                            } else {
                                let txt = egui::RichText::new(name).size(11.5).color(if active {
                                    t.tab_text_active
                                } else {
                                    t.tab_text_inactive
                                });
                                let b = egui::Button::new(txt)
                                    .small()
                                    .fill(if active { t.tab_active } else { t.tabs_bg })
                                    .frame(true);
                                let resp = ui.add(b).on_hover_text(
                                    crate::i18n::strs().msg_page_tab_tooltip(i + 1, id),
                                );
                                if active && scroll_to_active {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                resp
                            };
                            if resp.clicked() {
                                self.switch_page(i);
                            }
                            if resp.double_clicked() {
                                self.tab_rename = Some((*id, name.clone()));
                            }
                            if resp.secondary_clicked() {
                                self.tab_menu = Some((
                                    i,
                                    resp.interact_pointer_pos().unwrap_or(resp.rect.left_top()),
                                ));
                            }
                        }
                    });
                });
            // No "+" button: it duplicated the tab context menu's
            // Insert Page… and its rect overflowed the strip band so
            // clicks could miss (#70 — removed by owner decision
            // 2026-10-24). The ribbon's New Page button remains, as
            // does right-click ▸ Insert Page….
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        let t = theme::shell_tokens(ui.visuals());
        // #257 item 3: RTL mirrors the bar — the page/shape readout
        // hugs the right edge and the zoom cluster the left.
        let dir = if crate::i18n::rtl() {
            egui::Layout::right_to_left(egui::Align::Center)
        } else {
            egui::Layout::left_to_right(egui::Align::Center)
        };
        let rev = if crate::i18n::rtl() {
            egui::Layout::left_to_right(egui::Align::Center)
        } else {
            egui::Layout::right_to_left(egui::Align::Center)
        };
        ui.with_layout(dir, |ui| {
            let mut left;
            if let Some(ed) = self.ed() {
                let geom = self.page_geom();
                // Visio names the active page in the status bar (#54),
                // e.g. "Page-2 of 4".
                let page_name = ed
                    .doc
                    .pages
                    .get(self.page_idx)
                    .map(|p| p.name.as_str())
                    .unwrap_or("Page");
                left = format!(
                    "{} of {}    {}",
                    page_name,
                    ed.doc.pages.len(),
                    self.view.unit.fmt(geom.w_in)
                        + " × "
                        + &self
                            .view
                            .unit
                            .fmt(geom.h_in)
                            .replace(" mm", "")
                            .replace(" in", "")
                        + " "
                        + self.view.unit.suffix(),
                );
                if let Some((_, sid)) = self.sel {
                    if let Some(page) = ed.doc.pages.get(self.page_idx) {
                        if let Some(s) = vsdx::model::find_shape_by_id(&page.shapes, sid) {
                            if let Ok(chain) = shape_chain(&ed.doc, page.id, sid) {
                                let g = |n: &str| {
                                    ed.doc
                                        .resolve_cell(s, &chain, n)
                                        .and_then(|c| c.numeric())
                                        .unwrap_or(0.0)
                                };
                                left.push_str(&format!(
                                    "    shape {}: {} × {} at {}",
                                    sid,
                                    self.view.unit.fmt(g("Width")),
                                    self.view.unit.fmt(g("Height")),
                                    self.view.unit.fmt_pt(g("PinX"), g("PinY")),
                                ));
                            }
                        }
                    }
                }
            } else {
                left = "no document".into();
            }
            ui.label(egui::RichText::new(left).color(t.status_text).size(11.0));
            // #209 step 5 (spec §3.2): the proofing language's native
            // name, read-only — dictionary vocabulary, never translated
            // (dc's display_name rule), and NOT the UI language.
            let lang = crate::spellcheck::lang_display_name(&self.spell.proofing_stem());
            ui.label(egui::RichText::new(lang).color(t.status_text).size(11.0));
            if self.ed.is_some() {
                // #209 step 6 (spec §3.3): the macro-record chip in its
                // resting state — disabled, the spec's tooltip. No macro
                // engine exists yet (M7); the active-recording visual
                // waits for that feature's own issue.
                status_chip(
                    ui,
                    crate::ribbon::Icon::Macro,
                    crate::i18n::strs().msg_macro_record_tip(),
                    false,
                    false,
                );
            }
            ui.with_layout(rev, |ui| {
                // Visio status bar right end: − [zoom slider] + N%
                if self.ed.is_some() {
                    // #209 step 4: the Fit Page chip (spec §3.5.5) —
                    // rightmost in the zoom cluster, dispatching the
                    // existing Whole Page fit.
                    if status_chip(
                        ui,
                        crate::ribbon::Icon::Fit,
                        crate::i18n::strs().msg_fit_page_window_tip(),
                        self.view.fit_mode,
                        true,
                    ) {
                        self.dispatch(Action::FitPage);
                    }
                    let pct = (self.view.zoom * 100.0).round() as u32;
                    // #209 step 3: the % is a button opening the Zoom
                    // dialog (Visio's status-bar zoom level).
                    if ui
                        .button(format!("{pct}%"))
                        .on_hover_text(crate::i18n::strs().msg_zoom_level_tip())
                        .clicked()
                    {
                        self.dispatch(Action::ZoomDialog);
                    }
                    let mut z = self.view.zoom;
                    let slider = egui::Slider::new(&mut z, 0.1..=8.0).show_value(false);
                    let sresp = ui
                        .add_sized([110.0, 14.0], slider)
                        .on_hover_text(crate::i18n::strs().msg_zoom());
                    if sresp.changed() {
                        self.view.zoom = z.clamp(0.1, 8.0);
                        self.view.fit_mode = false;
                    }
                    if ui
                        .button("−")
                        .on_hover_text(crate::i18n::strs().msg_zoom_out())
                        .clicked()
                    {
                        self.dispatch(Action::ZoomOut);
                    }
                    if ui
                        .button("+")
                        .on_hover_text(crate::i18n::strs().msg_zoom_in())
                        .clicked()
                    {
                        self.dispatch(Action::ZoomIn);
                    }
                    // #209 step 7 (spec §3.4): Presentation Mode —
                    // leftmost of the zoom cluster; clicking again
                    // (or Esc) restores the chrome.
                    if status_chip(
                        ui,
                        crate::ribbon::Icon::Presentation,
                        crate::i18n::strs().msg_presentation_mode(),
                        self.chrome_hidden,
                        true,
                    ) {
                        self.dispatch(Action::PresentationMode);
                    }
                    ui.separator();
                }
                if let Some((x, y)) = self.mouse_page {
                    ui.label(
                        egui::RichText::new(self.view.unit.fmt_pt(x, y))
                            .color(t.status_text)
                            .size(11.0),
                    );
                } else {
                    ui.label(egui::RichText::new("").size(11.0));
                }
            });
        });
    }
}

/// A status-bar toggle chip (#209): the MDI icon in a compact hit box,
/// Visio's chip treatment — a selected fill when on, the hover fill
/// otherwise — and one tooltip carrying the accessible name. A
/// disabled chip (the macro placeholder) paints the muted icon and
/// never clicks. Returns whether it was clicked.
fn status_chip(
    ui: &mut Ui,
    icon: crate::ribbon::Icon,
    tip: String,
    selected: bool,
    enabled: bool,
) -> bool {
    let t = theme::shell_tokens(ui.visuals());
    let (r, resp) = ui.allocate_exact_size(egui::vec2(20.0, 16.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &tip));
    let resp = resp.on_hover_text(tip);
    if enabled && (selected || resp.hovered()) {
        let (fill, edge) = if selected {
            (t.ribbon_selected, t.ribbon_selected_edge)
        } else {
            (t.ribbon_hover, t.ribbon_hover_edge)
        };
        ui.painter().rect_filled(r, 2.0, fill);
        ui.painter()
            .rect_stroke(r, 2.0, Stroke::new(1.0, edge), egui::StrokeKind::Middle);
    }
    icon.paint(ui, r.shrink(2.0), enabled);
    enabled && resp.clicked()
}

/// The a11y label of a Tell Me panel row (#204 step 6): a result
/// carries its command's name, a Try row its intent text, the help
/// footer its "Get Help on '<query>'" text.
fn row_label(row: &crate::tellme::PanelRow) -> &str {
    match row {
        crate::tellme::PanelRow::Result(e) => e.label.as_str(),
        crate::tellme::PanelRow::Try { label, .. } => label,
        crate::tellme::PanelRow::Help { label } => label,
    }
}
