//! The dialog windows: Shape Data, Select by Type, Snap & Glue,
//! Insert Field, Hyperlinks, page reorder, and master Actions.

use super::*;

impl App {
    /// The shape's Prop rows for the Shape Data dialog (#30):
    /// instance rows, then the master's (effective view); edits always
    /// target the instance scope by row name.
    pub(super) fn prop_rows(&self, shape_id: i64) -> Vec<(String, vsdx::model::Row)> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.get(self.page_idx) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return Vec::new();
        };
        let collect = |shapes: &[Shape]| {
            shapes
                .iter()
                .flat_map(|s| s.sheet.sections_named("Prop"))
                .flat_map(|sec| sec.rows.iter())
                .filter_map(|r| r.n.as_deref().map(|n| (n.to_string(), r.clone())))
                .collect::<Vec<_>>()
        };
        let mut out = collect(std::slice::from_ref(sh));
        if let Some(m) = sh.master.and_then(|id| ed.doc.master(id)) {
            out.extend(collect(&m.shapes));
        }
        out
    }

    /// Shape Data… (#30): one field per visible Prop row, ordered by
    /// SortKey; the editor follows the row's Type/Format. Edits write
    /// the row's Value through SetRowCell (one undo per change).
    pub(super) fn shape_data_window(&mut self, ctx: &egui::Context) {
        fn write_value(app: &mut App, page_id: i64, shape_id: i64, name: &str, val: &str) {
            app.apply(dc_core::SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: shape_id,
                },
                "Prop",
                0,
                dc_core::commands::RowSel::Named(name.to_string()),
                "Value",
                vsdx::model::Cell::value(val.to_string()),
            ));
        }
        let Some(shape_id) = self.shape_data_open else {
            return;
        };
        let alive = self
            .ed()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .is_some_and(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id).is_some());
        if !alive {
            self.shape_data_open = None;
            self.prop_bufs.clear();
            self.prop_add = None;
            return;
        }
        let Some(page_id) = self.page_id() else {
            return;
        };
        let mut rows = self.prop_rows(shape_id);
        rows.sort_by(|a, b| crate::props::cmp_sort_key(&a.1, &b.1));
        let mut bufs = std::mem::take(&mut self.prop_bufs);
        // Closable (live report on #30): the title-bar X AND a labeled
        // Close button in an always-visible action bar — the same
        // elementary/Apple-HIG cancel path the Hyperlinks window uses.
        // Default position: top-right of the viewport, where Visio
        // pins its Shape Data pane.
        let mut open = true;
        let mut close_btn = false;
        let top_right = ctx
            .input(|i| i.viewport().inner_rect)
            .map_or(egui::pos2(900.0, 60.0), |r| {
                r.right_top() + egui::vec2(-360.0, 60.0)
            });
        egui::Window::new(crate::i18n::strs().msg_shape_data())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(340.0)
            .default_pos(top_right)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window; the
                // property grid swaps its columns under RTL (field
                // visual-left, label visual-right).
                ui.with_layout(dialog_dir(), |ui| {
                    let rtl_grid = crate::i18n::rtl();
                    let emit_label = |ui: &mut egui::Ui, label: &str, tip: &str| {
                        ui.label(label).on_hover_text(tip);
                    };
                    egui::Grid::new("dc-shape-data")
                        .num_columns(2)
                        .spacing(egui::vec2(8.0, 6.0))
                        .show(ui, |ui| {
                            for (name, row) in &rows {
                                if crate::props::invisible(row) {
                                    continue;
                                }
                                let label = crate::props::label(row, name);
                                let current = row
                                    .cell("Value")
                                    .and_then(|c| c.v.clone())
                                    .unwrap_or_default();
                                let tip = row
                                    .cell("Prompt")
                                    .and_then(|c| c.v.clone())
                                    .unwrap_or_default();
                                if !rtl_grid {
                                    emit_label(ui, &label, &tip);
                                }
                                let _field_id = ();
                                match crate::props::field_kind(row) {
                                    crate::props::FieldKind::Bool => {
                                        let mut on = current.trim() == "1";
                                        let r = ui.checkbox(&mut on, "");
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::CheckBox,
                                                true,
                                                &label,
                                            )
                                        });
                                        if r.changed() {
                                            write_value(
                                                self,
                                                page_id,
                                                shape_id,
                                                name,
                                                if on { "1" } else { "0" },
                                            );
                                        }
                                    }
                                    crate::props::FieldKind::List(choices) => {
                                        // A manual dropdown: the stock
                                        // ComboBox's arrow glyph is missing
                                        // from the pinned font set.
                                        let r =
                                            ui.button(format!("{current}  ›")).on_hover_text(&tip);
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::ComboBox,
                                                true,
                                                &label,
                                            )
                                        });
                                        let mut picked: Option<String> = None;
                                        egui::Popup::menu(&r)
                                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                                            .show(|ui| {
                                                ui.set_min_width(120.0);
                                                for c in &choices {
                                                    if ui
                                                        .selectable_label(*c == current, c)
                                                        .clicked()
                                                    {
                                                        picked = Some(c.clone());
                                                    }
                                                }
                                            });
                                        if let Some(v) = picked {
                                            write_value(self, page_id, shape_id, name, &v);
                                        }
                                    }
                                    crate::props::FieldKind::VarList(choices) => {
                                        // Variable list (Type 4): the same
                                        // dropdown over the Format choices,
                                        // plus "＋" to reveal an add row —
                                        // adding a choice writes the row's
                                        // Value AND appends to Format (one
                                        // undo step), so the list grows
                                        // the way Visio's variable list
                                        // does.
                                        let mut picked: Option<String> = None;
                                        let mut open_add = false;
                                        ui.horizontal(|ui| {
                                            let r = ui
                                                .button(format!("{current}  ›"))
                                                .on_hover_text(&tip);
                                            r.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::Role::ComboBox,
                                                    true,
                                                    &label,
                                                )
                                            });
                                            egui::Popup::menu(&r)
                                                .close_behavior(
                                                    egui::PopupCloseBehavior::CloseOnClick,
                                                )
                                                .show(|ui| {
                                                    ui.set_min_width(120.0);
                                                    for c in &choices {
                                                        if ui
                                                            .selectable_label(*c == current, c)
                                                            .clicked()
                                                        {
                                                            picked = Some(c.clone());
                                                        }
                                                    }
                                                });
                                            if ui.small_button("+").clicked() {
                                                open_add = true;
                                            }
                                        });
                                        if open_add {
                                            self.prop_add = Some(name.clone());
                                        }
                                        if let Some(v) = picked {
                                            write_value(self, page_id, shape_id, name, &v);
                                        }
                                        // The add row (label column empty —
                                        // mirrored with the grid under RTL).
                                        if self.prop_add.as_deref() == Some(name.as_str()) {
                                            if rtl_grid {
                                                // The PROPERTY row's second cell —
                                                // emitted before the row closes, or
                                                // the label would land in the add
                                                // row below it (PR #269 review).
                                                emit_label(ui, &label, &tip);
                                            }
                                            ui.end_row();
                                            if rtl_grid {
                                                // the editor lands visual-left
                                            } else {
                                                ui.label("");
                                            }
                                            let add_key = format!("{name}#add");
                                            let mut commit = false;
                                            let mut cancel = false;
                                            {
                                                let buf = bufs.entry(add_key.clone()).or_default();
                                                ui.horizontal(|ui| {
                                                    let edit = ui.add(
                                                        egui::TextEdit::singleline(buf).hint_text(
                                                            crate::i18n::strs().msg_new_choice(),
                                                        ),
                                                    );
                                                    edit.widget_info(|| {
                                                        egui::WidgetInfo::labeled(
                                                            egui::Role::TextInput,
                                                            true,
                                                            &label,
                                                        )
                                                    });
                                                    if ui
                                                        .button(crate::i18n::strs().msg_add())
                                                        .clicked()
                                                        || (edit.lost_focus()
                                                            && ui.input(|i| {
                                                                i.key_pressed(egui::Key::Enter)
                                                            }))
                                                    {
                                                        commit = true;
                                                    }
                                                    if ui
                                                        .button(crate::i18n::strs().msg_cancel())
                                                        .clicked()
                                                    {
                                                        cancel = true;
                                                    }
                                                });
                                            }
                                            if rtl_grid {
                                                ui.label("");
                                            }
                                            if cancel {
                                                bufs.remove(&add_key);
                                                self.prop_add = None;
                                            } else if commit {
                                                let v = bufs
                                                    .get(&add_key)
                                                    .map(|b| b.trim().to_string())
                                                    .unwrap_or_default();
                                                if v.contains(';') {
                                                    // A choice is one ";"-separated
                                                    // Format entry — an embedded
                                                    // separator would split into
                                                    // phantom choices. Keep the add
                                                    // row open for correction.
                                                    self.status = crate::i18n::strs()
                                                        .msg_label_a_choice_cannot_contain(
                                                            label.clone(),
                                                        );
                                                } else if !v.is_empty() {
                                                    bufs.remove(&add_key);
                                                    self.prop_add = None;
                                                    let fmt = row
                                                        .cell("Format")
                                                        .and_then(|c| c.v.clone())
                                                        .unwrap_or_default();
                                                    if fmt.split(';').any(|c| c.trim() == v) {
                                                        // Already a choice: select it,
                                                        // don't duplicate it in Format.
                                                        write_value(
                                                            self, page_id, shape_id, name, &v,
                                                        );
                                                    } else {
                                                        let new_fmt = if fmt.trim().is_empty() {
                                                            v.clone()
                                                        } else {
                                                            format!("{};{}", fmt.trim_end(), v)
                                                        };
                                                        let scope = Scope::Shape {
                                                            page: page_id,
                                                            shape: shape_id,
                                                        };
                                                        let row_sel =
                                                            dc_core::commands::RowSel::Named(
                                                                name.clone(),
                                                            );
                                                        self.apply(dc_core::Macro::new(
                                                            crate::i18n::strs()
                                                                .msg_undo_add_choice(
                                                                    name.clone(),
                                                                    format!("{v:?}"),
                                                                ),
                                                            vec![
                                                                Box::new(dc_core::SetRowCell::new(
                                                                    scope.clone(),
                                                                    "Prop",
                                                                    0,
                                                                    row_sel.clone(),
                                                                    "Value",
                                                                    vsdx::model::Cell::value(v),
                                                                )),
                                                                Box::new(dc_core::SetRowCell::new(
                                                                    scope,
                                                                    "Prop",
                                                                    0,
                                                                    row_sel,
                                                                    "Format",
                                                                    vsdx::model::Cell::value(
                                                                        new_fmt,
                                                                    ),
                                                                )),
                                                            ],
                                                        ));
                                                    }
                                                } else {
                                                    bufs.remove(&add_key);
                                                    self.prop_add = None;
                                                }
                                            }
                                        }
                                    }
                                    crate::props::FieldKind::Number => {
                                        let buf = bufs
                                            .entry(name.clone())
                                            .or_insert_with(|| current.clone());
                                        let r = ui
                                            .add(egui::TextEdit::singleline(buf).hint_text("0.0"));
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::TextInput,
                                                true,
                                                &label,
                                            )
                                        });
                                        if r.lost_focus() && buf.as_str() != current {
                                            match buf.trim().parse::<f64>() {
                                                Ok(_) => {
                                                    let v = buf.trim().to_string();
                                                    write_value(self, page_id, shape_id, name, &v);
                                                }
                                                Err(_) => {
                                                    self.status = crate::i18n::strs()
                                                        .msg_label_not_a_number(label.clone());
                                                }
                                            }
                                            bufs.remove(name);
                                        }
                                    }
                                    crate::props::FieldKind::Str
                                    | crate::props::FieldKind::Formatted(_) => {
                                        let buf = bufs
                                            .entry(name.clone())
                                            .or_insert_with(|| current.clone());
                                        let r = ui.add(egui::TextEdit::singleline(buf));
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::TextInput,
                                                true,
                                                &label,
                                            )
                                        });
                                        if r.lost_focus() && buf.as_str() != current {
                                            let v = buf.trim().to_string();
                                            write_value(self, page_id, shape_id, name, &v);
                                            bufs.remove(name);
                                        }
                                    }
                                }
                                if rtl_grid && self.prop_add.as_deref() != Some(name.as_str()) {
                                    // The mirrored row's second cell (the
                                    // add-row variant emitted its own above,
                                    // before its row closed).
                                    emit_label(ui, &label, &tip);
                                }
                                ui.end_row();
                            }
                        });
                    // Always-visible action bar: Close bottom-right —
                    // #257: mirrored to the bottom-left under RTL.
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        row_rev(ui, |ui| {
                            if ui.button(crate::i18n::strs().msg_close()).clicked() {
                                close_btn = true;
                            }
                        });
                    });
                });
            });
        if open && !close_btn {
            self.prop_bufs = bufs;
        } else {
            self.shape_data_open = None;
            self.prop_bufs.clear();
            self.prop_add = None;
        }
    }

    /// The Select by Type dialog (#110, URS-1160, Visio's Home ▸
    /// Editing ▸ Select ▸ Select by Type): pick a criterion — shape
    /// type, master, or layer — and a value from the page, and the
    /// matching shapes become the selection.
    pub(super) fn select_by_type_window(&mut self, ctx: &egui::Context) {
        if !self.selby_open {
            return;
        }
        // The value list per criterion, straight off the page.
        let mut pairs: Vec<(String, String, Vec<i64>)> = Vec::new();
        if let Some(ed) = self.ed.as_ref() {
            if let Some(page) = ed.doc.pages.get(self.page_idx) {
                // (key shown to the user, dedup key, matching ids)
                let mut push = |label: String, ids: Vec<i64>| {
                    if let Some(slot) = pairs.iter_mut().find(|(l, _, _)| *l == label) {
                        slot.2.extend(ids);
                    } else {
                        pairs.push((label.clone(), label, ids));
                    }
                };
                match self.selby_criterion {
                    0 => {
                        for sh in &page.shapes {
                            let t = sh.type_.clone().unwrap_or_else(|| "Shape".into());
                            push(t, vec![sh.id]);
                        }
                    }
                    1 => {
                        for sh in &page.shapes {
                            if let Some(mid) = sh.master {
                                let name = ed
                                    .doc
                                    .master(mid)
                                    .and_then(|m| m.name_u.clone())
                                    .unwrap_or_else(|| crate::i18n::strs().msg_master_mid(mid));
                                push(name, vec![sh.id]);
                            }
                        }
                    }
                    _ => {
                        let layer_names: Vec<(i64, String)> = page
                            .sheet()
                            .map(|sheet| {
                                sheet
                                    .sections_named("Layer")
                                    .into_iter()
                                    .flat_map(|sec| {
                                        sec.rows.iter().map(|row| {
                                            (
                                                row.ix,
                                                row.cell("Name")
                                                    .and_then(|c| c.v.clone())
                                                    .unwrap_or_else(|| {
                                                        crate::i18n::strs()
                                                            .msg_layer_ix_fallback(row.ix)
                                                    }),
                                            )
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        for (ix, name) in &layer_names {
                            let ids: Vec<i64> = page
                                .shapes
                                .iter()
                                .filter(|sh| {
                                    sh.sheet
                                        .cell("LayerMember")
                                        .and_then(|c| c.numeric())
                                        .is_some_and(|m| m as i64 == *ix)
                                })
                                .map(|sh| sh.id)
                                .collect();
                            if !ids.is_empty() {
                                push(name.clone(), ids);
                            }
                        }
                    }
                }
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        if !pairs.iter().any(|(l, _, _)| *l == self.selby_value) {
            self.selby_value = pairs.first().map(|(l, _, _)| l.clone()).unwrap_or_default();
        }
        let mut open = true;
        let mut apply = false;
        egui::Window::new(crate::i18n::strs().msg_select_by_type())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window.
                ui.with_layout(dialog_dir(), |ui| {
                    row(ui, |ui| {
                        ui.label(crate::i18n::strs().msg_select_by());
                        for (i, label) in [
                            crate::i18n::strs().msg_shape_type(),
                            crate::i18n::strs().msg_master(),
                            crate::i18n::strs().msg_layer(),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if ui
                                .selectable_label(self.selby_criterion == i as u8, label)
                                .clicked()
                            {
                                self.selby_criterion = i as u8;
                                self.selby_value.clear();
                            }
                        }
                    });
                    ui.separator();
                    if pairs.is_empty() {
                        ui.weak(crate::i18n::strs().msg_nothing_on_this_page_matches_the());
                    } else {
                        ui.label(crate::i18n::strs().msg_value());
                        for (label, _, ids) in &pairs {
                            if ui
                                .selectable_label(
                                    self.selby_value == *label,
                                    crate::i18n::strs()
                                        .msg_select_value_n_shapes(label.clone(), ids.len() as i64),
                                )
                                .clicked()
                            {
                                self.selby_value = label.clone();
                            }
                        }
                    }
                    ui.add_space(6.0);
                    let can = !self.selby_value.is_empty();
                    let btn = egui::Button::new(crate::i18n::strs().msg_select());
                    let r = ui.add_enabled(can, btn);
                    if r.clicked() {
                        apply = true;
                    }
                });
            });
        if apply {
            if let Some((_, _, ids)) = pairs.iter().find(|(l, _, _)| *l == self.selby_value) {
                let ids = ids.clone();
                if let Some(page_id) = self.page_id() {
                    let mut all: Vec<(i64, i64)> = ids.iter().map(|sid| (page_id, *sid)).collect();
                    all.sort();
                    all.dedup();
                    if let Some(first) = all.first().copied() {
                        self.sel = Some(first);
                    }
                    let n = all.len();
                    self.sel_set = all;
                    let crit = match self.selby_criterion {
                        0 => crate::i18n::strs().msg_shape_type(),
                        1 => crate::i18n::strs().msg_master(),
                        _ => crate::i18n::strs().msg_layer(),
                    };
                    self.status = crate::i18n::strs().msg_selected_n_by_criterion(n as i64, crit);
                }
            }
        }
        if !open {
            self.selby_open = false;
        }
    }

    /// The Snap & Glue dialog (#109, Visio 2016's View ▸ Visual Aids):
    /// per-target toggles for snap (URS-1100) and glue (URS-1112)
    /// plus the dynamic-grid aid (URS-1088). Every change writes
    /// through to the document's DocumentSettings — settings are
    /// document state, not undoable drawing edits.
    pub(super) fn snap_glue_window(&mut self, ctx: &egui::Context) {
        if !self.snap_dlg_open {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new(crate::i18n::strs().msg_snap_glue())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(400.0)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window; the
                // two columns swap (Glue column reads first under RTL).
                ui.with_layout(dialog_dir(), |ui| {
                    let s = &mut self.snap;
                    let snap_col = |ui: &mut egui::Ui,
                                    s: &mut crate::snap::SnapSettings,
                                    changed: &mut bool| {
                        ui.vertical(|ui| {
                            ui.strong(crate::i18n::strs().msg_snap());
                            if ui
                                .checkbox(&mut s.snap_active, crate::i18n::strs().msg_active())
                                .changed()
                            {
                                *changed = true;
                            }
                            ui.add_enabled_ui(s.snap_active, |ui| {
                                ui.label(crate::i18n::strs().msg_snap_to());
                                for (v, label) in [
                                    (
                                        &mut s.snap_ruler_subdivisions,
                                        crate::i18n::strs().msg_ruler_subdivisions(),
                                    ),
                                    (&mut s.snap_grid, crate::i18n::strs().msg_grid()),
                                    (&mut s.snap_guides, crate::i18n::strs().msg_guides()),
                                    (
                                        &mut s.snap_alignment_boxes,
                                        crate::i18n::strs().msg_alignment_boxes(),
                                    ),
                                    (
                                        &mut s.snap_shape_extensions,
                                        crate::i18n::strs().msg_shape_extensions(),
                                    ),
                                    (
                                        &mut s.snap_shape_intersections,
                                        crate::i18n::strs().msg_shape_intersections(),
                                    ),
                                    (
                                        &mut s.snap_connection_points,
                                        crate::i18n::strs().msg_snap_conn_points(),
                                    ),
                                ] {
                                    if ui.checkbox(v, label).changed() {
                                        *changed = true;
                                    }
                                }
                            });
                        });
                    };
                    let glue_col = |ui: &mut egui::Ui,
                                    s: &mut crate::snap::SnapSettings,
                                    changed: &mut bool| {
                        ui.vertical(|ui| {
                            ui.strong(crate::i18n::strs().msg_glue());
                            if ui
                                .checkbox(&mut s.glue_active, crate::i18n::strs().msg_active())
                                .changed()
                            {
                                *changed = true;
                            }
                            ui.add_enabled_ui(s.glue_active, |ui| {
                                ui.label(crate::i18n::strs().msg_glue_to());
                                for (v, label) in [
                                    (&mut s.glue_guides, crate::i18n::strs().msg_guides()),
                                    (
                                        &mut s.glue_shape_handles,
                                        crate::i18n::strs().msg_shape_handles(),
                                    ),
                                    (
                                        &mut s.glue_shape_vertices,
                                        crate::i18n::strs().msg_shape_vertices(),
                                    ),
                                    (
                                        &mut s.glue_connection_points,
                                        crate::i18n::strs().msg_snap_conn_points(),
                                    ),
                                ] {
                                    if ui.checkbox(v, label).changed() {
                                        *changed = true;
                                    }
                                }
                            });
                        });
                    };
                    // #257: explicit emission order in a plain horizontal
                    // (the generic `row` flip reserves too little wrap
                    // width for the vertical columns under RTL).
                    ui.horizontal(|ui| {
                        if crate::i18n::rtl() {
                            glue_col(ui, s, &mut changed);
                            ui.separator();
                            snap_col(ui, s, &mut changed);
                        } else {
                            snap_col(ui, s, &mut changed);
                            ui.separator();
                            glue_col(ui, s, &mut changed);
                        }
                    });
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_drawing_aids());
                    if ui
                        .checkbox(
                            &mut s.dynamic_grid,
                            crate::i18n::strs().msg_snap_dynamic_grid(),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.add_space(3.0);
                    ui.weak(crate::i18n::strs().msg_snapglue_note());
                });
            });
        if !open {
            self.snap_dlg_open = false;
        }
        if changed {
            self.apply_snap_settings();
        }
    }

    /// The Insert Field dialog (#267): Category/Field/Format over the
    /// formula engine's field set, with a live preview. OK inserts
    /// the Field row AND the `<fld>` marker in one undo step — at the
    /// caret when the in-place editor is open on the shape (the
    /// display value parks in the draft), appended to the raw text
    /// otherwise.
    pub(super) fn fields_insert_window(&mut self, ctx: &egui::Context) {
        use crate::fields_dlg::{self, FieldsOutcome};
        let Some((dialog_page, shape_id)) = self.fields_open else {
            return;
        };
        // Close automatically when the shape goes away — pinned to
        // the PAGE the dialog opened on: shape IDs collide across
        // pages and documents, so an open dialog never resolves
        // against a different page's same-id shape (CodeRabbit
        // rounds 2–3).
        let alive = self.ed().is_some_and(|ed| {
            ed.doc
                .pages
                .iter()
                .find(|p| p.id == dialog_page)
                .is_some_and(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id).is_some())
        }) && Some(dialog_page) == self.page_id();
        if !alive {
            self.fields_open = None;
            self.fields_caret = None;
            self.fields_draft = None;
            return;
        }
        let Some(page_id) = self.page_id() else {
            return;
        };
        // The shape's Prop row names — Shape Data entries (model
        // vocabulary, never translated), INCLUDING the rows a
        // master-backed shape only inherits: a fresh instance keeps
        // all its Shape Data on the master, and the picker must not
        // come up empty (Greptile round 1). Local rows take
        // precedence (the evaluator layers them the same way).
        let shape_data: Vec<String> = self
            .ed()
            .and_then(|ed| {
                let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
                let s = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
                let mut names: Vec<String> = s
                    .sheet
                    .sections_named("Prop")
                    .iter()
                    .flat_map(|sec| sec.rows.iter().filter_map(|r| r.n.clone()))
                    .collect();
                if let Ok(chain) = dc_core::scope::shape_chain(&ed.doc, page_id, shape_id) {
                    let ancestors: Vec<_> =
                        chain.into_iter().filter(|c| c.id != shape_id).collect();
                    if let Some(src) = ed.doc.instance_source(s, &ancestors) {
                        for sec in src.sheet.sections_named("Prop") {
                            names.extend(sec.rows.iter().filter_map(|r| r.n.clone()));
                        }
                    }
                }
                names.sort();
                names.dedup();
                Some(names)
            })
            .unwrap_or_default();
        let mut draft = self.fields_draft.take().unwrap_or_default();
        let value_f = draft.value_f(&shape_data);
        let formats = fields_dlg::format_entries();
        let format_f = formats
            .get(draft.fmt)
            .map(|f| f.format_f.clone())
            .unwrap_or_else(|| "FIELDPICTURE(0)".to_string());

        // The preview: evaluated against the shape's field
        // environment (page facts included).
        let preview = self
            .ed()
            .and_then(|ed| {
                let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
                let s = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
                let chain = dc_core::scope::shape_chain(&ed.doc, page_id, shape_id).ok()?;
                let fctx = dc::fields::FieldsCtx::for_page(&ed.doc, page);
                Some(dc::fields::preview_field(
                    &ed.doc,
                    s,
                    &chain,
                    Some(&fctx),
                    &value_f,
                    Some(&format_f),
                ))
            })
            .unwrap_or_default();

        let mut open = true;
        let outcome =
            fields_dlg::window(ctx, &mut open, &mut draft, &shape_data, &preview, &value_f);
        self.fields_draft = Some(draft);
        if !open || outcome == FieldsOutcome::Cancel {
            self.fields_open = None;
            self.fields_draft = None;
            return;
        }
        if outcome != FieldsOutcome::Ok {
            return;
        }
        // The insertion.
        if value_f.is_empty() {
            self.status = crate::i18n::strs().msg_enter_a_field_formula_first();
            return;
        }
        self.fields_open = None;
        self.fields_draft = None;
        // Caret-precise, in priority order: a LIVE editor session on
        // this shape parks the display at the caret (the keytip path
        // — a keyboard action never commits the edit); else the caret
        // CAPTURED when the dialog opened (the opening click already
        // committed the edit, #22); else append at the raw end.
        let session_live = self
            .text_edit
            .as_ref()
            .map(|s| (s.page, s.shape))
            .is_some_and(|(p, s)| p == page_id && s == shape_id);
        if session_live {
            let mark = crate::textedit::FieldMark {
                offset: 0, // park_field places it at the caret
                display: preview.clone(),
                value_f: value_f.clone(),
                format_f: Some(format_f),
            };
            if let Some(sess) = self.text_edit.as_mut() {
                sess.park_field(mark);
            }
            return;
        }
        let mut caret_used: Option<usize> = None;
        let insert = match self.fields_caret.take() {
            Some((p, s, caret)) if p == page_id && s == shape_id => {
                caret_used = Some(caret);
                dc_core::FieldInsert::ReplacePlain {
                    offset: caret,
                    len: 0,
                }
            }
            _ => dc_core::FieldInsert::End,
        };
        // One undo step: the field AND the `TEXTWIDTH`/`TEXTHEIGHT`
        // cache refreshes for the text the shape will now render —
        // the SUBSTITUTED text with the new display spliced in
        // (measurement sees what emit_text paints; #267, CodeRabbit
        // round 2).
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let (cmd, refreshes) = self
            .ed()
            .map(|ed| {
                let page =
                    ed.doc.pages.iter().find(|p| p.id == page_id).and_then(|p| {
                        vsdx::model::find_shape_by_id(&p.shapes, shape_id).map(|_| p)
                    });
                let post_text = match page {
                    Some(page) => {
                        let sub = (|| -> Option<String> {
                            let s = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
                            let chain =
                                dc_core::scope::shape_chain(&ed.doc, page_id, shape_id).ok()?;
                            let fctx = dc::fields::FieldsCtx::for_page(&ed.doc, page);
                            // What renders now (substituted), with the
                            // new display at the insertion point.
                            let cur = dc::fields::substituted_text(&ed.doc, s, &chain, Some(&fctx))
                                .or_else(|| s.sheet.text.clone())
                                .unwrap_or_default();
                            let chars: Vec<char> = cur.chars().collect();
                            Some(match insert {
                                dc_core::FieldInsert::End => format!("{cur}{}", preview),
                                dc_core::FieldInsert::ReplacePlain { offset, len } => {
                                    let off = offset.min(chars.len());
                                    let end = (offset + len).min(chars.len());
                                    let head: String = chars[..off].iter().collect();
                                    let tail: String = chars[end..].iter().collect();
                                    format!("{head}{}{tail}", preview)
                                }
                                dc_core::FieldInsert::RawOffset(_) => cur,
                            })
                        })();
                        sub
                    }
                    None => None,
                };
                let cmd = dc_core::InsertField::new(scope.clone(), value_f.clone(), insert)
                    .with_format(format_f.clone())
                    .with_display(preview.clone());
                let refreshes = dc::textfunc::text_cache_refreshes(
                    &ed.doc,
                    &scope,
                    &post_text.unwrap_or_default(),
                );
                (cmd, refreshes)
            })
            .unwrap_or_else(|| {
                (
                    dc_core::InsertField::new(scope.clone(), value_f.clone(), insert)
                        .with_format(format_f.clone())
                        .with_display(preview.clone()),
                    Vec::new(),
                )
            });
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::with_capacity(1 + refreshes.len());
        cmds.push(Box::new(cmd));
        cmds.extend(refreshes);
        self.apply(dc_core::Macro::new(
            crate::i18n::strs().msg_insert_field(),
            cmds,
        ));
        // A caret-based insertion ADVANCES the retained caret past
        // the inserted display (plain characters): the next dialog
        // on this shape inserts AFTER the field, not before it —
        // two insertions at one caret used to read `AYXB` instead
        // of `AXYB` (CodeRabbit/Greptile round 5). Applies only to
        // the page/shape that received the field; an append (`End`)
        // keeps whatever caret the shape last had.
        if let Some(caret) = caret_used {
            self.last_edit_caret =
                Some((page_id, shape_id, caret + preview.chars().count()));
        }
        if preview.is_empty() {
            self.status = crate::i18n::strs().msg_field_inserted_no_preview();
        }
    }

    pub(super) fn hyperlink_window(&mut self, ctx: &egui::Context) {
        let Some(shape_id) = self.hyperlink_open else {
            return;
        };
        // Close automatically when the shape or page goes away.
        let alive = self
            .ed()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .is_some_and(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id).is_some());
        if !alive {
            self.hyperlink_open = None;
            self.hyperlink_draft = None;
            return;
        }
        let Some(page_id) = self.page_id() else {
            return;
        };
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        // Instance rows plus the master's, inherited-origin visible
        // (#139) — `hyperlink_rows` keeps instance-first order and
        // drops master rows an instance row already shadows.
        let rows = self.hyperlink_rows(shape_id);
        let mut draft = self.hyperlink_draft.take();
        // #71: the 2016 dialog's storage toggle. Visio keeps it
        // app-level (observed); ours persists app-side keyed by the
        // document's path — never inside the .vsdx, whose
        // DocumentSettings schema is closed.
        let mut relative_paths = self.relative_paths.get(&self.path);
        let doc_dir = crate::hyperlink::doc_dir(&self.path).map(|p| p.to_path_buf());
        // Page names for the Sub-address picker (#71): a SubAddress
        // names a page (the follow path), so the dropdown lists them.
        let page_names: Vec<String> = self
            .ed()
            .map(|e| e.doc.pages.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        // Cancel mechanism (live report + the 2016 dialog's Cancel):
        // the title-bar X and an explicit Close button both discard
        // any draft and dismiss the window.
        let mut open = true;
        // The Close BUTTON flips this (the X writes `open` via the
        // builder; the two can't share a borrow). Esc-no-draft does
        // too (#110 audit).
        let mut close_btn = false;
        let mut esc_close = false;
        egui::Window::new(crate::i18n::strs().msg_hyperlinks())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                // #110 audit: this window manages its own Esc — a
                // draft discards first (a window close on the same key
                // would lose the layering); Enter commits the draft
                // (the dialog's default OK button). Skipped while the
                // in-place text editor owns the keyboard (#22).
                if self.text_edit.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if draft.is_some() {
                        draft = None;
                    } else {
                        esc_close = true;
                    }
                }
                // The 2016 storage toggle (#71) sits above the list.
                if ui
                    .checkbox(
                        &mut relative_paths,
                        crate::i18n::strs().msg_use_relative_paths_for_hyperlink(),
                    )
                    .changed()
                {
                    self.relative_paths.set(&self.path, relative_paths);
                }
                if rows.is_empty() && draft.is_none() {
                    ui.weak(crate::i18n::strs().msg_no_hyperlinks_on_this_shape());
                }
                for row in &rows {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(if row.desc.is_empty() {
                                row.addr.clone()
                            } else {
                                format!("{} — {}", row.desc, row.addr)
                            })
                            .size(12.0),
                        );
                        if row.inherited {
                            // Origin marker + read-only Remove: the row
                            // belongs to the master (Visio keeps
                            // inherited row sets read-only until
                            // overridden); Edit materializes the
                            // override (#139).
                            ui.weak(crate::i18n::strs().msg_inherited());
                        }
                        if ui.button(crate::i18n::strs().msg_go()).clicked() {
                            let (a, s) = (row.addr.clone(), row.sub.clone());
                            self.follow_hyperlink(&a, &s);
                        }
                        if ui.button(crate::i18n::strs().msg_edit()).clicked() {
                            draft = Some(HyperlinkDraft {
                                edit_row: Some(row.ix),
                                from_master: row.inherited,
                                address: row.addr.clone(),
                                sub: row.sub.clone(),
                                desc: row.desc.clone(),
                            });
                        }
                        let remove = ui.add_enabled(
                            !row.inherited,
                            egui::Button::new(crate::i18n::strs().msg_remove()),
                        );
                        if row.inherited {
                            remove.on_disabled_hover_text(
                                crate::i18n::strs().msg_inherited_from_the_shape_s_maste(),
                            );
                        } else if remove.clicked() {
                            self.apply(dc_core::commands::RemoveSectionRow::new(
                                scope.clone(),
                                "Hyperlink",
                                0,
                                dc_core::commands::RowSel::Ix(row.ix),
                            ));
                        }
                    });
                }
                ui.separator();
                if let Some(d) = draft.as_mut() {
                    // Accessible names on the inputs (#56 pattern);
                    // the grid labels alone don't label the fields.
                    // #257: the RTL grid mirrors its rows — the label
                    // cell lands last (visual right), the browse/combo
                    // control first (visual left).
                    let rtl_grid = crate::i18n::rtl();
                    let input_cell = |ui: &mut Ui, label: &str, buf: &mut String| {
                        let r = ui.add(egui::TextEdit::singleline(buf).hint_text(label));
                        r.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, label)
                        });
                    };
                    let field = |ui: &mut Ui, label: &str, buf: &mut String| {
                        ui.label(label);
                        input_cell(ui, label, buf);
                    };
                    egui::Grid::new("dc-hyperlink-draft")
                        .num_columns(3)
                        .spacing(egui::vec2(8.0, 4.0))
                        .show(ui, |ui| {
                            if rtl_grid {
                                // Browse… (#71) first (visual left).
                                if ui.button(crate::i18n::strs().msg_browse()).clicked() {
                                    self.begin_dialog(PendingKind::HyperlinkAddress);
                                }
                                input_cell(
                                    ui,
                                    crate::i18n::strs().msg_hl_address().as_str(),
                                    &mut d.address,
                                );
                                ui.label(crate::i18n::strs().msg_hl_address());
                                ui.end_row();
                            } else {
                                field(
                                    ui,
                                    crate::i18n::strs().msg_hl_address().as_str(),
                                    &mut d.address,
                                );
                                // Browse… (#71): the native file picker (the
                                // Open/Save As worker-thread pattern); the
                                // pick lands in the Address field once the
                                // dialog resolves.
                                if ui.button(crate::i18n::strs().msg_browse()).clicked() {
                                    self.begin_dialog(PendingKind::HyperlinkAddress);
                                }
                                ui.end_row();
                            }
                            if rtl_grid {
                                let combo = egui::ComboBox::from_id_salt("dc-hyperlink-sub-page")
                                    .selected_text(if d.sub.is_empty() {
                                        crate::i18n::strs().msg_hyperlink_pages()
                                    } else {
                                        d.sub.clone()
                                    })
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        for name in &page_names {
                                            ui.selectable_value(&mut d.sub, name.clone(), name);
                                        }
                                    });
                                combo.response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::ComboBox,
                                        true,
                                        crate::i18n::strs().msg_sub_address_page(),
                                    )
                                });
                                input_cell(
                                    ui,
                                    crate::i18n::strs().msg_hl_subaddress().as_str(),
                                    &mut d.sub,
                                );
                                ui.label(crate::i18n::strs().msg_hl_subaddress());
                                ui.end_row();
                                input_cell(
                                    ui,
                                    crate::i18n::strs().msg_hl_description().as_str(),
                                    &mut d.desc,
                                );
                                ui.label(crate::i18n::strs().msg_hl_description());
                                ui.end_row();
                            } else {
                                field(
                                    ui,
                                    crate::i18n::strs().msg_hl_subaddress().as_str(),
                                    &mut d.sub,
                                );
                                // Page picker (#71): a SubAddress names a
                                // page — the dropdown offers the document's.
                                // The accessible name satisfies the dialog
                                // label requirement (#56 pattern).
                                let combo = egui::ComboBox::from_id_salt("dc-hyperlink-sub-page")
                                    .selected_text(if d.sub.is_empty() {
                                        crate::i18n::strs().msg_hyperlink_pages()
                                    } else {
                                        d.sub.clone()
                                    })
                                    .width(110.0)
                                    .show_ui(ui, |ui| {
                                        for name in &page_names {
                                            ui.selectable_value(&mut d.sub, name.clone(), name);
                                        }
                                    });
                                combo.response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::Role::ComboBox,
                                        true,
                                        crate::i18n::strs().msg_sub_address_page(),
                                    )
                                });
                                ui.end_row();
                                field(
                                    ui,
                                    crate::i18n::strs().msg_hl_description().as_str(),
                                    &mut d.desc,
                                );
                                ui.end_row();
                            }
                        });
                    let (edit_row, from_master, address, sub, desc) = (
                        d.edit_row,
                        d.from_master,
                        d.address.clone(),
                        d.sub.clone(),
                        d.desc.clone(),
                    );
                    // Commit stores the address the way the toggle
                    // wants it (#71): inside the document's directory
                    // and relative-paths on → relative form. Picked
                    // paths already arrived relativized; this covers
                    // hand-typed ones too (idempotent).
                    let address = crate::hyperlink::store_address(
                        &address,
                        doc_dir.as_deref(),
                        relative_paths,
                    );
                    let mut close_draft = false;
                    // OK / Apply / Cancel (#71): Apply commits without
                    // closing the draft — the 2016 triad. One undo per
                    // commit either way.
                    // Returns the committed row ix (None when the
                    // draft was all-empty and wrote nothing) — Apply
                    // feeds it back so a second commit EDITS the row
                    // instead of duplicating it (CodeRabbit).
                    let commit = |self_: &mut Self| -> Option<i64> {
                        let ix = edit_row
                            .unwrap_or_else(|| rows.iter().map(|r| r.ix).max().unwrap_or(-1) + 1);
                        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
                        if let Some(old) = edit_row {
                            // An instance edit replaces its row. A
                            // master-inherited row has no local row
                            // to replace: SetRowCell's first-edit
                            // materialization seeds the local
                            // section+row (inherited values
                            // included) — the master stays
                            // untouched (#139).
                            if !from_master {
                                cmds.push(Box::new(dc_core::commands::RemoveSectionRow::new(
                                    scope.clone(),
                                    "Hyperlink",
                                    0,
                                    dc_core::commands::RowSel::Ix(old),
                                )));
                            }
                        }
                        let all_empty = address.trim().is_empty()
                            && sub.trim().is_empty()
                            && desc.trim().is_empty();
                        for (name, val) in [
                            ("Address", &address),
                            ("SubAddress", &sub),
                            ("Description", &desc),
                        ] {
                            // A materializing or REPLACING edit writes
                            // every field, cleared ones as explicit
                            // empty values: the materialized row is
                            // seeded with the inherited cells, so a
                            // skipped field would silently keep its
                            // master value (#139 review; #196 extended
                            // the seeding to the remove-then-rewrite
                            // path, where it resurrects cleared fields
                            // the same way — CodeRabbit on the #239
                            // batch). An all-empty draft keeps the pure
                            // row deletion (no write, nothing
                            // re-seeds); a brand-new row keeps the
                            // non-empty-only writes.
                            if from_master
                                || !val.trim().is_empty()
                                || (edit_row.is_some() && !all_empty)
                            {
                                cmds.push(Box::new(dc_core::SetRowCell::new(
                                    scope.clone(),
                                    "Hyperlink",
                                    0,
                                    dc_core::commands::RowSel::Ix(ix),
                                    name,
                                    vsdx::model::Cell::value(val.trim()),
                                )));
                            }
                        }
                        // An all-empty draft writes nothing (and
                        // must not record a no-op undo step).
                        if cmds.is_empty() {
                            return None;
                        }
                        self_.apply(dc_core::Macro::new(
                            crate::i18n::strs().msg_edit_hyperlink(),
                            cmds,
                        ));
                        Some(ix)
                    };
                    row(ui, |ui| {
                        if ui.button(crate::i18n::strs().msg_ok()).clicked() {
                            commit(self);
                            close_draft = true;
                        }
                        if ui.button(crate::i18n::strs().msg_apply()).clicked() {
                            if let Some(ix) = commit(self) {
                                // The draft now edits the row it just
                                // created (a second Apply updates it,
                                // never duplicates).
                                d.edit_row = Some(ix);
                                d.from_master = false;
                            }
                        }
                        if ui.button(crate::i18n::strs().msg_cancel()).clicked() {
                            close_draft = true;
                        }
                    });
                    // #110 audit: Enter is the dialog's OK (Visio's
                    // default button) — commits and closes the draft.
                    if ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                        commit(self);
                        close_draft = true;
                    }
                    if close_draft {
                        draft = None;
                    }
                }
                // The window's action bar (always visible — elementary/
                // Apple HIG: the dismiss control must be obvious, not
                // hidden behind editor state): New… left, Close right.
                ui.add_space(4.0);
                row(ui, |ui| {
                    if draft.is_none()
                        && ui.button(crate::i18n::strs().msg_hyperlink_new()).clicked()
                    {
                        draft = Some(HyperlinkDraft {
                            edit_row: None,
                            from_master: false,
                            address: String::new(),
                            sub: String::new(),
                            desc: String::new(),
                        });
                    }
                    // #257: the Close pin mirrors with the chrome.
                    row_rev(ui, |ui| {
                        if ui.button(crate::i18n::strs().msg_close()).clicked() {
                            draft = None;
                            close_btn = true;
                        }
                    });
                });
            });
        if open && !close_btn && !esc_close {
            self.hyperlink_draft = draft;
        } else {
            // Cancelled: discard the draft and close.
            self.hyperlink_open = None;
            self.hyperlink_draft = None;
        }
    }

    /// Visio's Reorder Pages dialog (#54): the page list with Move Up /
    /// Move Down buttons; every click is one undoable MovePage command,
    /// and the active page stays active wherever it lands.
    pub(super) fn reorder_pages_window(&mut self, ctx: &egui::Context) {
        if !self.reorder_open {
            return;
        }
        let mut close = false;
        let mut open = true;
        egui::Window::new(crate::i18n::strs().msg_reorder_pages())
            // #110 tab-order audit: the title-bar close X — the window
            // was the one dialog without a keyboard-reachable dismiss
            // besides tabbing to Close (Esc arrives through
            // `close_front_dialog`).
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                let n = self.ed().map(|e| e.doc.pages.len()).unwrap_or(0);
                let names: Vec<String> = self
                    .ed()
                    .map(|e| e.doc.pages.iter().map(|p| p.name.clone()).collect())
                    .unwrap_or_default();
                egui::Grid::new("dc-reorder-list")
                    .num_columns(3)
                    .spacing(egui::vec2(8.0, 4.0))
                    .show(ui, |ui| {
                        // #257: the RTL row puts the mover buttons on
                        // the visual left, the page name on the right
                        // (the LTR row keeps Visio's name first).
                        let rtl_grid = crate::i18n::rtl();
                        for (i, name) in names.iter().enumerate() {
                            let mut up_clicked = false;
                            let mut down_clicked = false;
                            let name_cell = |ui: &mut egui::Ui, name: &str| {
                                ui.label(egui::RichText::new(name.to_string()).size(12.0));
                            };
                            if rtl_grid {
                                if ui
                                    .add_enabled(
                                        i > 0,
                                        egui::Button::new(crate::i18n::strs().msg_move_up())
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    up_clicked = true;
                                }
                                if ui
                                    .add_enabled(
                                        i + 1 < n,
                                        egui::Button::new(crate::i18n::strs().msg_move_down())
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    down_clicked = true;
                                }
                                name_cell(ui, name);
                            } else {
                                name_cell(ui, name);
                                if ui
                                    .add_enabled(
                                        i > 0,
                                        egui::Button::new(crate::i18n::strs().msg_move_up())
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    up_clicked = true;
                                }
                                if ui
                                    .add_enabled(
                                        i + 1 < n,
                                        egui::Button::new(crate::i18n::strs().msg_move_down())
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    down_clicked = true;
                                }
                            }
                            if up_clicked {
                                self.tab_cmd_reorder(i, i - 1);
                            }
                            if down_clicked {
                                self.tab_cmd_reorder(i, i + 1);
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                row_rev(ui, |ui| {
                    if ui.button(crate::i18n::strs().msg_close()).clicked() {
                        close = true;
                    }
                });
            });
        if close || !open {
            self.reorder_open = false;
        }
    }

    /// The Visio 2016 shape context menu: clipboard, duplicate, delete,
    /// z-order, rotate/flip, text editing, ShapeSheet. Backlog items
    /// (Group, Format Shape, Shape Data) show grayed with their milestone
    /// so the surface matches Visio while the backends catch up.
    /// Master-defined right-click actions (ShapeSheet Actions section),
    /// in row order: (menu label, is-flyout-child). Visio heads the shape
    /// menu with these.
    /// Master-defined Actions rows (#25): labels, evaluated
    /// Checked/Invisible/Disabled, BeginGroup/FlyoutChild, SortKey
    /// ordering — everything the context menu needs, evaluated against
    /// the INSTANCE's effective sheet so instance-local User rows
    /// (which the actions themselves write) drive the checkmarks.
    pub(super) fn master_actions(&self, shape_id: i64) -> Vec<MasterActionRow> {
        struct RawRow {
            menu: String,
            action: String,
            checked_f: Option<String>,
            checked_v: Option<String>,
            disabled_f: Option<String>,
            disabled_v: Option<String>,
            invisible_f: Option<String>,
            invisible_v: Option<String>,
            begin_group: bool,
            flyout_child: bool,
            sort_key: f64,
            /// `Action` = No Formula or ReadOnly: a flyout header, not a
            /// command.
            display_only: bool,
        }
        // A cell's live value: formula first (evaluated in `env`),
        // cached V as fallback — the row-cell convention.
        let eval_flag =
            |f: &Option<String>, v: &Option<String>, env: &vsdx::formula::Env| -> bool {
                let by_formula = f.as_deref().and_then(|f| {
                    if f == "Inh" || f == "No Formula" {
                        None
                    } else {
                        vsdx::formula::eval(f, env).ok().and_then(|x| x.as_f64())
                    }
                });
                by_formula
                    .or_else(|| v.as_deref().and_then(|v| v.trim().parse::<f64>().ok()))
                    .is_some_and(|x| x != 0.0)
            };
        let collect = |self_: &App| -> Option<Vec<MasterActionRow>> {
            let ed = self_.ed.as_ref()?;
            let page = ed.doc.pages.get(self_.page_idx)?;
            let shape = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
            let m = ed.doc.master(shape.master?)?;
            // Instance evaluation environment: instance-local User rows
            // override the master's (dc::svg::shape_env layers them).
            let chain = dc_core::scope::shape_chain(&ed.doc, page.id, shape_id).ok()?;
            let env = dc::svg::shape_env(&ed.doc, shape, &chain);
            let mut rows: Vec<RawRow> = Vec::new();
            fn walk(sh: &Shape, out: &mut Vec<RawRow>) {
                for sec in sh.sheet.sections_named("Actions") {
                    for row in &sec.rows {
                        // Visio writes flyout labels either as V or as
                        // the Cell's text content (multi-line menus) —
                        // the parser keeps the latter as raw_inner.
                        let menu = row
                            .cell("Menu")
                            .and_then(|c| {
                                c.v.clone()
                                    .or_else(|| c.raw_inner.as_deref().map(str::to_owned))
                            })
                            .map(|t| t.trim().to_owned())
                            .filter(|t| !t.is_empty());
                        let Some(menu) = menu else { continue };
                        let f = |n: &str| -> Option<String> {
                            row.cell(n)
                                .and_then(|c| c.f.clone())
                                .filter(|f| !f.is_empty() && f != "No Formula" && f != "Inh")
                        };
                        let v =
                            |n: &str| -> Option<String> { row.cell(n).and_then(|c| c.v.clone()) };
                        // The Action FORMULA is F; V caches the last
                        // evaluation ('No Formula' rows carry V only).
                        let action_f = row.cell("Action").and_then(|c| c.f.clone());
                        let action = action_f
                            .clone()
                            .filter(|f| f != "No Formula")
                            .or_else(|| v("Action"))
                            .unwrap_or_default();
                        let display_only = action_f.as_deref() == Some("No Formula")
                            || v("ReadOnly")
                                .and_then(|x| x.trim().parse::<f64>().ok())
                                .is_some_and(|x| x != 0.0);
                        out.push(RawRow {
                            menu,
                            action,
                            display_only,
                            checked_f: f("Checked"),
                            checked_v: v("Checked"),
                            disabled_f: f("Disabled"),
                            disabled_v: v("Disabled"),
                            invisible_f: f("Invisible"),
                            invisible_v: v("Invisible"),
                            begin_group: v("BeginGroup")
                                .and_then(|x| x.trim().parse::<f64>().ok())
                                .is_some_and(|x| x != 0.0),
                            flyout_child: v("FlyoutChild")
                                .and_then(|x| x.trim().parse::<f64>().ok())
                                .is_some_and(|x| x != 0.0),
                            sort_key: v("SortKey")
                                .and_then(|x| x.trim().parse::<f64>().ok())
                                .unwrap_or(0.0),
                        });
                    }
                }
                for sub in &sh.sub_shapes {
                    walk(sub, out);
                }
            }
            for root in &m.shapes {
                walk(root, &mut rows);
            }
            // Visio orders by SortKey, then row order (stable).
            rows.sort_by(|a, b| {
                a.sort_key
                    .partial_cmp(&b.sort_key)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Some(
                rows.into_iter()
                    .filter(|r| !eval_flag(&r.invisible_f, &r.invisible_v, &env))
                    .map(|r| MasterActionRow {
                        menu: r.menu,
                        action: r.action,
                        checked: eval_flag(&r.checked_f, &r.checked_v, &env),
                        disabled: eval_flag(&r.disabled_f, &r.disabled_v, &env),
                        begin_group: r.begin_group,
                        flyout_child: r.flyout_child,
                        display_only: r.display_only,
                    })
                    .collect(),
            )
        };
        collect(self).unwrap_or_default()
    }

    /// Execute a master Actions row (#25): parse the SETF/GetRef chain
    /// and apply it as one undoable Macro against the instance. Rows
    /// whose formula needs a runtime we do not ship (RUNADDON, …)
    /// defer with a status note — the EventDblClick policy (#20).
    pub(super) fn run_master_action(&mut self, shape_id: i64, row: &MasterActionRow) {
        let Some(ed) = self.ed() else { return };
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(shape) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = dc_core::scope::shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        let env = dc::svg::shape_env(&ed.doc, shape, &chain);
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        match dc_core::commands::shape_action_commands(scope, &row.action, &env) {
            Ok(cmds) if cmds.is_empty() => {
                self.status =
                    crate::i18n::strs().msg_action_display_only(format!("{:?}", row.menu));
            }
            Ok(cmds) => {
                let label = format!("run shape action {:?}", row.menu);
                self.apply(dc_core::Macro::new(label, cmds));
            }
            Err(why) => {
                self.status =
                    crate::i18n::strs().msg_action_deferred(format!("{:?}", row.menu), why.clone());
            }
        }
    }
}
