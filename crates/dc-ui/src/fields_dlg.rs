//! The Insert Field dialog (GitHub #267): Visio 2016's Category /
//! Field / Format picker over the formula engine's field set.
//!
//! Categories and their entries follow the 2016 dialog's set, pared
//! to what EVALUATES in our engine (no dead entries — an offered
//! field must render): Custom Formula (free text), Date/Time and
//! Document Info (the `docProps/core.xml` facts), Geometry
//! (Width/Height), Object Info (the master name), Page Info (name,
//! number), and Shape Data (the shape's live `Prop` rows — model
//! vocabulary, never translated). The Format dropdown carries the
//! corpus's own format pictures.

/// One dialog category. The order matches Visio's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldsCategory {
    CustomFormula,
    DateTime,
    DocumentInfo,
    Geometry,
    ObjectInfo,
    PageInfo,
    ShapeData,
}

impl FieldsCategory {
    fn label(self) -> String {
        let s = crate::i18n::strs();
        match self {
            FieldsCategory::CustomFormula => s.msg_field_cat_custom_formula(),
            FieldsCategory::DateTime => s.msg_field_cat_date_time(),
            FieldsCategory::DocumentInfo => s.msg_field_cat_document_info(),
            FieldsCategory::Geometry => s.msg_field_cat_geometry(),
            FieldsCategory::ObjectInfo => s.msg_field_cat_object_info(),
            FieldsCategory::PageInfo => s.msg_field_cat_page_info(),
            FieldsCategory::ShapeData => s.msg_field_cat_shape_data(),
        }
    }

    pub(crate) const ALL: [FieldsCategory; 7] = [
        FieldsCategory::CustomFormula,
        FieldsCategory::DateTime,
        FieldsCategory::DocumentInfo,
        FieldsCategory::Geometry,
        FieldsCategory::ObjectInfo,
        FieldsCategory::PageInfo,
        FieldsCategory::ShapeData,
    ];
}

/// One selectable field: a display label plus the Value formula it
/// writes.
pub(crate) struct FieldEntry {
    pub label: String,
    pub value_f: String,
}

/// The entries of one category (`shape_data` carries the shape's
/// `Prop` row names — empty for a shape with none).
pub(crate) fn category_entries(cat: FieldsCategory, shape_data: &[String]) -> Vec<FieldEntry> {
    let s = crate::i18n::strs();
    let mk = |label: String, value_f: &str| FieldEntry {
        label,
        value_f: value_f.to_string(),
    };
    match cat {
        FieldsCategory::CustomFormula => Vec::new(),
        FieldsCategory::DateTime => vec![
            mk(s.msg_field_creation_date(), "DOCCREATION()"),
            mk(s.msg_field_last_save_date(), "DOCLASTSAVE()"),
            mk(s.msg_field_last_edit_date(), "DOCLASTEDIT()"),
        ],
        FieldsCategory::DocumentInfo => vec![
            mk(s.msg_field_creator(), "CREATOR()"),
            mk(s.msg_field_title(), "TITLE()"),
            mk(s.msg_field_subject(), "SUBJECT()"),
            mk(s.msg_field_keywords(), "KEYWORDS()"),
            mk(s.msg_field_description(), "DESCRIPTION()"),
        ],
        FieldsCategory::Geometry => vec![
            mk(s.msg_field_width(), "Width"),
            mk(s.msg_field_height(), "Height"),
        ],
        FieldsCategory::ObjectInfo => vec![mk(s.msg_field_master_name(), "MASTERNAME()")],
        FieldsCategory::PageInfo => vec![
            mk(s.msg_field_page_name(), "PAGENAME()"),
            mk(s.msg_field_page_number(), "PAGENUMBER()"),
        ],
        FieldsCategory::ShapeData => shape_data
            .iter()
            .map(|n| mk(n.clone(), &format!("Prop.{n}")))
            .collect(),
    }
}

/// One Format dropdown entry: a friendly label plus the Format
/// formula the row carries.
pub(crate) struct FormatEntry {
    pub label: String,
    pub format_f: String,
}

pub(crate) fn format_entries() -> Vec<FormatEntry> {
    let s = crate::i18n::strs();
    let mk = |label: String, format_f: &str| FormatEntry {
        label,
        format_f: format_f.to_string(),
    };
    vec![
        mk(s.msg_field_fmt_general(), "FIELDPICTURE(0)"),
        mk(s.msg_field_fmt_number_0(), "0"),
        mk(s.msg_field_fmt_number_0_00(), "0.00"),
        mk(s.msg_field_fmt_number_0_unit(), "0u"),
        mk(s.msg_field_fmt_number_0_00_unit(), "0.00u"),
        mk(s.msg_field_fmt_percent(), "FIELDPICTURE(3)"),
        mk(s.msg_field_fmt_feet_inches(), "FIELDPICTURE(10)"),
        mk(s.msg_field_fmt_text(), "@"),
        mk(s.msg_field_fmt_uppercase_text(), "@+"),
        mk(s.msg_field_fmt_short_date(), "c"),
        mk(s.msg_field_fmt_long_date(), "{<202>}"),
        mk(s.msg_field_fmt_month_year(), "{{MMM yyyy}}"),
        mk(s.msg_field_fmt_full_date(), "{{MMMM d, yyyy}}"),
    ]
}

/// The dialog's edit state, owned by the app between frames.
#[derive(Debug, Clone)]
pub(crate) struct FieldsDraft {
    pub cat: FieldsCategory,
    /// Selected entry index within the category (0 when the category
    /// has none — Custom Formula uses `formula`).
    pub item: usize,
    /// Selected Format index.
    pub fmt: usize,
    /// Custom Formula text.
    pub formula: String,
}

impl Default for FieldsDraft {
    fn default() -> Self {
        FieldsDraft {
            cat: FieldsCategory::DateTime,
            item: 0,
            fmt: 0,
            formula: String::new(),
        }
    }
}

impl FieldsDraft {
    /// The Value formula the current selection writes (empty when
    /// nothing is selected yet).
    pub fn value_f(&self, shape_data: &[String]) -> String {
        match self.cat {
            FieldsCategory::CustomFormula => self.formula.trim().to_string(),
            _ => category_entries(self.cat, shape_data)
                .get(self.item)
                .map(|e| e.value_f.clone())
                .unwrap_or_default(),
        }
    }
}

/// What the dialog decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldsOutcome {
    Open,
    /// Insert the current selection (`value_f`, `format_f`).
    Ok,
    Cancel,
}

/// Show the Insert Field window. Returns a terminal outcome; `open`
/// is the window's open flag (the caller clears its state on
/// terminal outcomes). `preview` is the evaluated display string
/// ("" when nothing resolves — the preview line shows the formula
/// then).
pub(crate) fn window(
    ctx: &egui::Context,
    open: &mut bool,
    draft: &mut FieldsDraft,
    shape_data: &[String],
    preview: &str,
    value_f: &str,
) -> FieldsOutcome {
    let mut outcome = FieldsOutcome::Open;
    egui::Window::new(crate::i18n::strs().msg_insert_field())
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            let s = crate::i18n::strs();
            ui.horizontal(|ui| {
                ui.label(s.msg_field_category());
                // A manual dropdown (the house pattern): the stock
                // ComboBox's arrow glyph is missing from the pinned
                // fonts, and the explicit WidgetInfo carries the
                // accessible name.
                let selected = draft.cat.label();
                let r = ui.button(format!("{selected}  ›"));
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::ComboBox,
                        true,
                        s.msg_field_category().as_str(),
                    )
                });
                egui::Popup::menu(&r)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(150.0);
                        for cat in FieldsCategory::ALL {
                            ui.selectable_value(&mut draft.cat, cat, cat.label());
                        }
                    });
            });
            if draft.cat == FieldsCategory::CustomFormula {
                ui.horizontal(|ui| {
                    ui.label(s.msg_field_formula());
                    let mut f = draft.formula.clone();
                    let resp = ui.text_edit_singleline(&mut f);
                    if f != draft.formula {
                        draft.formula = f;
                    }
                    let _ = resp;
                });
            } else {
                let entries = category_entries(draft.cat, shape_data);
                if entries.is_empty() {
                    ui.label(s.msg_field_no_entries());
                } else {
                    draft.item = draft.item.min(entries.len() - 1);
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (i, e) in entries.iter().enumerate() {
                                ui.selectable_value(&mut draft.item, i, e.label.clone());
                            }
                        });
                }
            }
            ui.horizontal(|ui| {
                ui.label(s.msg_field_format());
                let formats = format_entries();
                draft.fmt = draft.fmt.min(formats.len() - 1);
                let selected = formats[draft.fmt].label.clone();
                let r = ui.button(format!("{selected}  ›"));
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::ComboBox,
                        true,
                        s.msg_field_format().as_str(),
                    )
                });
                egui::Popup::menu(&r)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(150.0);
                        for (i, f) in formats.iter().enumerate() {
                            ui.selectable_value(&mut draft.fmt, i, f.label.clone());
                        }
                    });
            });
            ui.separator();
            // The preview: the evaluated display, or the formula when
            // nothing resolves yet.
            ui.horizontal(|ui| {
                ui.label(s.msg_field_preview());
                let shown = if preview.is_empty() { value_f } else { preview };
                ui.monospace(shown);
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(s.msg_ok()).clicked() {
                    outcome = FieldsOutcome::Ok;
                }
                if ui.button(s.msg_cancel()).clicked() {
                    outcome = FieldsOutcome::Cancel;
                }
            });
        });
    outcome
}
