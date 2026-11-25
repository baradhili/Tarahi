//! The Format pane and Shapesheet panel: effective-cell accessors,
//! cell writes, dropdowns, and the shared color palette window.

use super::*;

impl App {
    /// Effective cell value of the primary selection (instance →
    /// master) as a trimmed string (#31).
    pub(super) fn eff_cell_str(&self, name: &str) -> Option<String> {
        let (ed, (page_id, sid)) = (self.ed()?, self.sel?);
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
        let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
        ed.doc
            .resolve_cell(sh, &chain, name)
            .and_then(|c| c.v.clone())
            .map(|v| v.trim().to_string())
    }

    pub(super) fn eff_cell_num(&self, name: &str) -> Option<f64> {
        let (ed, (page_id, sid)) = (self.ed()?, self.sel?);
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
        let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
        ed.doc
            .resolve_cell(sh, &chain, name)
            .and_then(|c| c.numeric())
    }

    /// Effective first-row cell of a section (Character/Paragraph, #31).
    pub(super) fn eff_row_str(&self, section: &str, cell: &str) -> Option<String> {
        let (ed, (page_id, sid)) = (self.ed()?, self.sel?);
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
        let chain = shape_chain(&ed.doc, page_id, sid).ok()?;
        ed.doc
            .effective_sheet(sh, &chain)
            .sections_named(section)
            .first()
            .and_then(|sec| sec.rows.first())
            .and_then(|r| r.cell(cell))
            .and_then(|c| c.v.clone())
            .map(|v| v.trim().to_string())
    }

    /// Write one cell on every selected shape — one Macro, one undo
    /// step (#31 requirement 2).
    pub(super) fn fmt_set_cell(&mut self, name: &str, value: &str) {
        let targets = self.sel_set.clone();
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (page_id, sid) in targets {
            cmds.push(Box::new(SetCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                name,
                vsdx::Cell::value(value.to_owned()),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(
                crate::i18n::strs().msg_undo_set_cell(name),
                cmds,
            ));
        }
    }

    pub(super) fn fmt_set_row(&mut self, section: &str, cell: &str, value: &str) {
        let targets = self.sel_set.clone();
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (page_id, sid) in targets {
            cmds.push(Box::new(dc_core::SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                section,
                0,
                dc_core::commands::RowSel::Ix(0),
                cell,
                vsdx::Cell::value(value.to_owned()),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(
                crate::i18n::strs().msg_undo_set_section_cell(section, cell),
                cmds,
            ));
        }
    }

    /// A named-choice dropdown (the manual popup — the stock ComboBox
    /// glyph is missing from the pinned fonts). Returns the chosen
    /// value's key when the user picks one.
    pub(super) fn fmt_dropdown(
        ui: &mut Ui,
        current: &str,
        key_of: &dyn Fn(&str) -> String,
        choices: &[(&str, String)], // (key, display)
    ) -> Option<String> {
        let display = choices
            .iter()
            .find(|(k, _)| *k == current)
            .map(|(_, d)| d.clone())
            .unwrap_or_else(|| current.to_string());
        let r = ui.button(format!("{display}  ›"));
        let mut picked = None;
        egui::Popup::menu(&r)
            // Close-on-click would dismiss the menu on any scrollbar
            // interaction, so the menu closes only on outside clicks
            // and ui.close() after a pick (the arrow gallery's 46
            // entries need the scroll).
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(130.0);
                let max_h = (ui.input(|i| i.viewport_rect().height()) - 120.0).clamp(180.0, 520.0);
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .show(ui, |ui| {
                        for (k, d) in choices {
                            if ui.selectable_label(*k == current, d.clone()).clicked() {
                                picked = Some(key_of(k));
                                // CloseOnClickOutside keeps the menu
                                // open otherwise — close on a pick.
                                ui.close();
                            }
                        }
                    });
            });
        picked
    }

    /// The Format Shape pane (#31): Fill / Line / Text for the current
    /// selection, writing the same cells the renderer reads — WYSIWYG
    /// by construction. Values seed from the effective sheet; writes
    /// are instance-local overrides (never the master).
    pub(super) fn format_pane(&mut self, ui: &mut Ui) {
        let t = theme::shell_tokens(ui.visuals());
        ui.set_min_width(200.0);
        ui.heading(
            egui::RichText::new(crate::i18n::strs().msg_format_shape())
                .size(13.0)
                .color(t.label),
        );
        ui.add_space(4.0);
        let mut bufs = std::mem::take(&mut self.fmt_bufs);
        let mut color_pick: Option<ColorTarget> = None;
        let mut set_cell: Option<(String, String)> = None;
        let mut set_row: Option<(String, String, String)> = None;
        // #149 item 2: a targeted open (a mini-toolbar Format icon)
        // forces its section open and scrolls to it once, then the
        // focus is consumed.
        let focus = self.format_pane_focus;
        let mut scrolled = false;

        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut fill_hdr =
                egui::CollapsingHeader::new(crate::i18n::strs().msg_fill()).default_open(true);
            if focus == Some(FormatSection::Fill) {
                fill_hdr = fill_hdr.open(Some(true));
            }
            let fill_rsp = fill_hdr.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_foreground());
                    if ui
                        .button(
                            self.eff_cell_str("FillForegnd")
                                .unwrap_or_else(|| "—".into()),
                        )
                        .clicked()
                    {
                        color_pick = Some(ColorTarget::Fill);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_background());
                    if ui
                        .button(self.eff_cell_str("FillBkgnd").unwrap_or_else(|| "—".into()))
                        .clicked()
                    {
                        color_pick = Some(ColorTarget::FillBkgnd);
                    }
                });
                let cur = self
                    .eff_cell_str("FillPattern")
                    .unwrap_or_else(|| "1".into());
                if let Some(v) = Self::fmt_dropdown(
                    ui,
                    &cur,
                    &|k: &str| k.to_string(),
                    &[
                        ("0", crate::i18n::strs().msg_none()),
                        ("1", crate::i18n::strs().msg_solid()),
                        ("2", "5%".to_string()),
                        ("3", "10%".to_string()),
                        ("4", "20%".to_string()),
                        ("5", "30%".to_string()),
                        ("6", "40%".to_string()),
                        ("24", crate::i18n::strs().msg_gradient_horiz()),
                        ("25", crate::i18n::strs().msg_gradient_vert()),
                        ("28", crate::i18n::strs().msg_gradient_radial()),
                    ],
                ) {
                    set_cell = Some(("FillPattern".into(), v));
                }
            });
            if focus == Some(FormatSection::Fill) {
                fill_rsp
                    .header_response
                    .scroll_to_me(Some(egui::Align::Min));
                scrolled = true;
            }
            ui.add_space(2.0);
            let mut line_hdr =
                egui::CollapsingHeader::new(crate::i18n::strs().msg_line()).default_open(true);
            if focus == Some(FormatSection::Line) {
                line_hdr = line_hdr.open(Some(true));
            }
            let line_rsp = line_hdr.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_color());
                    if ui
                        .button(self.eff_cell_str("LineColor").unwrap_or_else(|| "—".into()))
                        .clicked()
                    {
                        color_pick = Some(ColorTarget::Line);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_weight());
                    let cur_pt = self.eff_cell_num("LineWeight").unwrap_or(0.0104) * 72.0;
                    let mut pt = cur_pt;
                    let r = ui.add(
                        egui::Slider::new(&mut pt, 0.25..=12.0)
                            .show_value(true)
                            .custom_formatter(|v, _| format!("{v:.2} pt")),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Slider,
                            true,
                            crate::i18n::strs().msg_line_weight(),
                        )
                    });
                    if r.drag_stopped() && (pt - cur_pt).abs() > 1e-6 {
                        set_cell = Some(("LineWeight".into(), format!("{:.6}", pt / 72.0)));
                    }
                });
                let cur = self
                    .eff_cell_str("LinePattern")
                    .unwrap_or_else(|| "1".into());
                if let Some(v) = Self::fmt_dropdown(
                    ui,
                    &cur,
                    &|k: &str| k.to_string(),
                    // #171: labels match Visio 2016's measured gallery
                    // (Analysis/line-patterns-visio2016.md) — 9 is the
                    // short 4-2 dash, 16 the long dash.
                    &[
                        ("0", crate::i18n::strs().msg_none()),
                        ("1", crate::i18n::strs().msg_solid()),
                        ("2", crate::i18n::strs().msg_dash()),
                        ("3", crate::i18n::strs().msg_dot()),
                        ("4", crate::i18n::strs().msg_dash_dot()),
                        ("5", crate::i18n::strs().msg_dash_dot_dot()),
                        ("9", crate::i18n::strs().msg_short_dash()),
                        ("10", crate::i18n::strs().msg_fine_dot()),
                        ("16", crate::i18n::strs().msg_long_dash()),
                        ("23", crate::i18n::strs().msg_fine_dash()),
                    ],
                ) {
                    set_cell = Some(("LinePattern".into(), v));
                }
                // Begin/End Arrow (#72, #169, #199): every value 0–45
                // carries its own glyph, pinned from the 600 dpi Visio
                // print (Analysis/arrow-gallery-visio2016.md, the
                // 2026-11-06 re-read). The picker lists the gallery
                // per value, Visio-style.
                let arrows: &[(&str, String)] = &[
                    ("0", crate::i18n::strs().msg_none()),
                    ("1", crate::i18n::strs().msg_open_2_prong()),
                    ("2", crate::i18n::strs().msg_small_arrow()),
                    ("3", crate::i18n::strs().msg_open_wide()),
                    ("4", crate::i18n::strs().msg_classic_arrow()),
                    ("5", crate::i18n::strs().msg_wide_arrow()),
                    ("6", crate::i18n::strs().msg_long_arrow()),
                    ("7", crate::i18n::strs().msg_open_fork()),
                    ("8", crate::i18n::strs().msg_stealth_dart()),
                    ("9", crate::i18n::strs().msg_slanted_tick()),
                    ("10", crate::i18n::strs().msg_filled_circle()),
                    ("11", crate::i18n::strs().msg_filled_square()),
                    ("12", crate::i18n::strs().msg_open_v_tall()),
                    ("13", crate::i18n::strs().msg_filled_arrow_plain()),
                    ("14", crate::i18n::strs().msg_open_triangle_large()),
                    ("15", crate::i18n::strs().msg_open_triangle_small()),
                    ("16", crate::i18n::strs().msg_open_triangle_medium_large()),
                    ("17", crate::i18n::strs().msg_open_triangle_medium()),
                    ("18", crate::i18n::strs().msg_open_triangle_convex_base()),
                    ("19", crate::i18n::strs().msg_open_barbed()),
                    ("20", crate::i18n::strs().msg_open_circle()),
                    ("21", crate::i18n::strs().msg_open_square()),
                    ("22", crate::i18n::strs().msg_open_diamond()),
                    ("23", crate::i18n::strs().msg_slanted_tick()),
                    ("24", crate::i18n::strs().msg_tick()),
                    ("25", crate::i18n::strs().msg_double_tick()),
                    ("26", crate::i18n::strs().msg_triple_tick()),
                    ("27", crate::i18n::strs().msg_crow_s_foot()),
                    ("28", crate::i18n::strs().msg_crow_s_foot_tick()),
                    ("29", crate::i18n::strs().msg_fork_ring()),
                    ("30", crate::i18n::strs().msg_ring_tick()),
                    ("31", crate::i18n::strs().msg_tick_ring()),
                    ("32", crate::i18n::strs().msg_double_tick_ring()),
                    ("33", crate::i18n::strs().msg_triple_tick_ring()),
                    ("34", crate::i18n::strs().msg_diamond_ring()),
                    ("35", crate::i18n::strs().msg_tick_dot()),
                    ("36", crate::i18n::strs().msg_double_tick_dot()),
                    ("37", crate::i18n::strs().msg_triple_tick_dot()),
                    ("38", crate::i18n::strs().msg_diamond_dot()),
                    ("39", crate::i18n::strs().msg_double_arrow_filled()),
                    ("40", crate::i18n::strs().msg_double_arrow_open()),
                    ("41", crate::i18n::strs().msg_ring()),
                    ("42", crate::i18n::strs().msg_dot()),
                    ("43", crate::i18n::strs().msg_open_double_chevron_large()),
                    ("44", crate::i18n::strs().msg_tick_chevron()),
                    ("45", crate::i18n::strs().msg_tick_double_chevron()),
                ];
                let cur = self
                    .eff_cell_str("BeginArrow")
                    .unwrap_or_else(|| "0".into());
                if let Some(v) = Self::fmt_dropdown(ui, &cur, &|k: &str| k.to_string(), arrows) {
                    set_cell = Some(("BeginArrow".into(), v));
                }
                let cur = self.eff_cell_str("EndArrow").unwrap_or_else(|| "0".into());
                if let Some(v) = Self::fmt_dropdown(ui, &cur, &|k: &str| k.to_string(), arrows) {
                    set_cell = Some(("EndArrow".into(), v));
                }
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_rounding());
                    let cur = self.eff_cell_str("Rounding").unwrap_or_else(|| "0".into());
                    let buf = bufs.entry("rounding".into()).or_insert(cur);
                    let r = ui.add(egui::TextEdit::singleline(buf).desired_width(70.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            crate::i18n::strs().msg_rounding(),
                        )
                    });
                    if r.lost_focus() {
                        if let Ok(v) = buf.trim().parse::<f64>() {
                            set_cell = Some(("Rounding".into(), format!("{v:.6}")));
                        }
                        bufs.remove("rounding");
                    }
                });
            });
            if focus == Some(FormatSection::Line) {
                line_rsp
                    .header_response
                    .scroll_to_me(Some(egui::Align::Min));
                scrolled = true;
            }
            ui.add_space(2.0);
            let mut text_hdr =
                egui::CollapsingHeader::new(crate::i18n::strs().msg_text()).default_open(true);
            if focus == Some(FormatSection::Text) {
                text_hdr = text_hdr.open(Some(true));
            }
            let text_rsp = text_hdr.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_font());
                    let cur = self.eff_row_str("Character", "Font").unwrap_or_default();
                    let buf = bufs.entry("font".into()).or_insert(cur);
                    let r = ui.add(egui::TextEdit::singleline(buf).desired_width(120.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            crate::i18n::strs().msg_font(),
                        )
                    });
                    if r.lost_focus() {
                        let v = buf.trim().to_string();
                        if !v.is_empty() {
                            set_row = Some(("Character".into(), "Font".into(), v));
                        }
                        bufs.remove("font");
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_size_pt());
                    let cur = self
                        .eff_row_str("Character", "Size")
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(10.0)
                        * 72.0;
                    let buf = bufs.entry("size".into()).or_insert(format!("{cur:.1}"));
                    let r = ui.add(egui::TextEdit::singleline(buf).desired_width(70.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            crate::i18n::strs().msg_size_pt(),
                        )
                    });
                    if r.lost_focus() {
                        if let Ok(pt) = buf.trim().parse::<f64>() {
                            set_row = Some((
                                "Character".into(),
                                "Size".into(),
                                format!("{:.6}", pt / 72.0),
                            ));
                        }
                        bufs.remove("size");
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_style());
                    let bits = self
                        .eff_row_str("Character", "Style")
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(0);
                    let mut bold = bits & 1 != 0;
                    let mut italic = bits & 2 != 0;
                    let rb = ui.checkbox(&mut bold, "B");
                    rb.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::CheckBox,
                            true,
                            crate::i18n::strs().msg_bold(),
                        )
                    });
                    if rb.changed() {
                        set_row = Some((
                            "Character".into(),
                            "Style".into(),
                            format!("{}", (bits & !1) | bold as i64),
                        ));
                    }
                    let ri = ui.checkbox(&mut italic, "I");
                    ri.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::CheckBox,
                            true,
                            crate::i18n::strs().msg_italic(),
                        )
                    });
                    if ri.changed() {
                        set_row = Some((
                            "Character".into(),
                            "Style".into(),
                            format!("{}", (bits & !2) | ((italic as i64) * 2)),
                        ));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::strs().msg_color());
                    if ui
                        .button(
                            self.eff_row_str("Character", "Color")
                                .unwrap_or_else(|| "—".into()),
                        )
                        .clicked()
                    {
                        color_pick = Some(ColorTarget::Text);
                    }
                });
                let cur = self
                    .eff_row_str("Paragraph", "HorzAlign")
                    .unwrap_or_else(|| "0".into());
                if let Some(v) = Self::fmt_dropdown(
                    ui,
                    &cur,
                    &|k: &str| k.to_string(),
                    &[
                        ("0", crate::i18n::strs().msg_align_left()),
                        ("1", crate::i18n::strs().msg_center()),
                        ("2", crate::i18n::strs().msg_align_right()),
                    ],
                ) {
                    set_row = Some(("Paragraph".into(), "HorzAlign".into(), v));
                }
            });
            if focus == Some(FormatSection::Text) {
                text_rsp
                    .header_response
                    .scroll_to_me(Some(egui::Align::Min));
                scrolled = true;
            }
        });
        if scrolled {
            self.format_pane_focus = None;
        }

        self.fmt_bufs = bufs;
        if let Some(target) = color_pick {
            self.color_pick = Some(target);
        }
        if let Some((name, v)) = set_cell {
            self.fmt_set_cell(&name, &v);
        }
        if let Some((sec, cell, v)) = set_row {
            self.fmt_set_row(&sec, &cell, &v);
        }
    }

    /// #192 spec: toggle one ShapeSheet window (open order kept;
    /// re-toggling removes). #272: shapes, pages and the document.
    pub(super) fn toggle_shapesheet(&mut self, target: crate::shapesheet::SheetTarget) {
        if let Some(i) = self.shapesheets.iter().position(|t| *t == target) {
            self.shapesheets.remove(i);
        } else {
            self.shapesheets.push(target);
        }
    }

    pub(super) fn shapesheet_panel(
        &mut self,
        ui: &mut Ui,
        target: crate::shapesheet::SheetTarget,
        max_w: f32,
    ) {
        // #190/#192: the window's title bar carries the close; this is
        // pure content, bound to the sheet's OWN shape (not the live
        // selection). #189: the body itself lives in `shapesheet.rs`
        // (app decomposition) — the resolved, Visio-2016-style view.
        ui.vertical(|ui| {
            ui.separator();
            let Some(page_id) = self.page_id() else {
                return;
            };
            let Some(ed) = self.ed() else { return };
            crate::shapesheet::panel_for(ui, &ed.doc, page_id, target, max_w);
        });
    }
}

impl App {
    /// Bump the selected shapes' Character.Size by whole points.
    pub(super) fn nudge_char_size(&mut self, delta_pt: f64) {
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
            let sheet = ed.doc.effective_sheet(sh, &chain);
            let cur = sheet
                .sections_named("Character")
                .first()
                .and_then(|sec| sec.rows.first())
                .and_then(|r| r.cell("Size"))
                .and_then(|c| c.numeric())
                .unwrap_or(0.13888);
            let next = (cur * 72.0 + delta_pt).clamp(4.0, 200.0) / 72.0;
            cmds.push(Box::new(SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                "Character",
                0,
                dc_core::commands::RowSel::Ix(0),
                "Size",
                vsdx::Cell::value(format!("{next:.6}")),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(crate::i18n::strs().msg_font_size(), cmds));
        }
    }
}

impl App {
    /// Apply a picked color to FillForegnd or LineColor on the selection.
    pub(super) fn apply_color(&mut self, target: ColorTarget, color: &str) {
        // Text color lives in the Character section's row 0 (#73): the
        // whole-selection SetRowCell macro the pane's other text
        // fields use (first-edit materialization seeds the row with
        // the inherited cells).
        if matches!(target, ColorTarget::Text) {
            self.fmt_set_row("Character", "Color", color);
            return;
        }
        let cell_name = match target {
            ColorTarget::Fill => "FillForegnd",
            ColorTarget::FillBkgnd => "FillBkgnd",
            ColorTarget::Line => "LineColor",
            // Handled by the SetRowCell path above.
            ColorTarget::Text => unreachable!("text color writes a row cell"),
        };
        let targets = self.sel_set.clone();
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        for (page_id, sid) in targets {
            cmds.push(Box::new(SetCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: sid,
                },
                cell_name,
                vsdx::Cell::value(color.to_owned()),
            )));
        }
        if !cmds.is_empty() {
            self.apply(Macro::new(format!("set {cell_name}"), cmds));
        }
    }
}

impl App {
    /// The Visio theme palette shown by the ribbon Fill/Line/Text
    /// color buttons and the Format pane's color rows.
    pub(super) fn color_palette_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.color_pick else {
            return;
        };
        let mut open = true;
        let title = match target {
            ColorTarget::Fill => crate::i18n::strs().msg_color_fill_window(),
            ColorTarget::FillBkgnd => crate::i18n::strs().msg_color_fill_bkgnd_window(),
            ColorTarget::Line => crate::i18n::strs().msg_color_line_window(),
            ColorTarget::Text => crate::i18n::strs().msg_color_text_window(),
        };
        egui::Window::new(title)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window (the
                // swatch grid itself is symmetric — the palette sweep
                // stays LTR either way).
                ui.with_layout(dialog_dir(), |ui| {
                    let theme_colors = [
                        "#000000", "#FFFFFF", "#EEEEEE", "#969696", "#4472C4", "#5B9BD5",
                        "#70AD47", "#ED7D31", "#FFC000", "#FF0000", "#7030A0", "#00B0F0",
                        "#FFF2CC", "#DEEAF6", "#E2EFDA", "#FCE4D6",
                    ];
                    egui::Grid::new("theme-palette")
                        .num_columns(8)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            for (i, c) in theme_colors.iter().enumerate() {
                                // from_hex requires the leading '#'.
                                let color =
                                    egui::Color32::from_hex(c).unwrap_or(egui::Color32::GRAY);
                                let (r, resp) =
                                    ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::click());
                                resp.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::Button, true, c)
                                });
                                ui.painter().rect_filled(r, 2.0, color);
                                ui.painter().rect_stroke(
                                    r,
                                    2.0,
                                    Stroke::new(1.0, Color32::from_gray(120)),
                                    egui::StrokeKind::Middle,
                                );
                                if resp.clicked() {
                                    self.apply_color(target, c);
                                    self.color_pick = None;
                                }
                                if i % 8 == 7 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            });
        if !open {
            self.color_pick = None;
        }
    }
}
