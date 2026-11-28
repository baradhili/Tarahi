//! The Replace dialog (GitHub #115 phase 3, Functional Specification
//! 3): Find what and Replace with (each with a history dropdown),
//! the Special caret-code menu below Replace with (it targets
//! whichever field holds focus), the "Search" group with HORIZONTAL
//! scope radios — no target checkboxes; the target set is FIXED to
//! every class (the decision recorded on #115: the spec shows no
//! checkboxes, and the issue's phase-3 write path names cells via
//! `SetRowCell`, so Replace hits the full set) — the Options
//! modifiers, and the Replace / Replace All / Find Next (default) /
//! Cancel row. Pure UI over [`ReplaceState`]; `app/find.rs` owns the
//! matching, replacing, and navigation.

use egui::Context;

use crate::app::{dialog_dir, row};
use crate::find_dlg::{insert_code, FindScopeKind, FindSession, SPECIAL_CODES};

/// What the dialog asked the app for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceOutcome {
    Open,
    FindNext,
    Replace,
    ReplaceAll,
    Help,
    Close,
}

/// The Replace window's session state. Both histories are
/// session-lifetime (the #115 decision).
#[derive(Debug, Clone)]
pub(crate) struct ReplaceState {
    pub open: bool,
    pub replacement: String,
    /// The caret (CHAR index) the Special menu inserts at, and which
    /// field it belongs to (`false` Find what, `true` Replace with) —
    /// spec 3: the menu "targets whichever text box currently has
    /// focus".
    caret: Option<usize>,
    caret_in_replacement: bool,
    /// Previous Replace-with strings, most recent first, bounded.
    pub repl_history: Vec<String>,
    pub session: FindSession,
}

impl Default for ReplaceState {
    fn default() -> Self {
        ReplaceState {
            open: false,
            replacement: String::new(),
            caret: None,
            caret_in_replacement: false,
            repl_history: Vec::new(),
            // Spec 3 defaults: Current page selected; the target set
            // fixed to every class (no checkboxes to edit it with).
            session: FindSession {
                targets: dc_core::FindTargets::all(),
                ..FindSession::default()
            },
        }
    }
}

/// Draw the Replace window. Enter runs Find Next when either input
/// holds focus (the default-button behavior).
pub(crate) fn window(ctx: &Context, state: &mut ReplaceState) -> ReplaceOutcome {
    let mut outcome = ReplaceOutcome::Open;
    let mut open = true;
    let mut edit_focused = false;
    egui::Window::new(crate::i18n::strs().msg_replace())
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            let s = crate::i18n::strs();
            ui.with_layout(dialog_dir(), |ui| {
                // 3.1: Find what and Replace with, each with its
                // history dropdown; the caret tracks whichever field
                // is focused (the Special menu's target).
                for (is_repl, buffer, history, label) in [
                    (
                        false,
                        &mut state.session.query,
                        &mut state.session.history,
                        s.msg_find_what(),
                    ),
                    (
                        true,
                        &mut state.replacement,
                        &mut state.repl_history,
                        s.msg_replace_with(),
                    ),
                ] {
                    let label = label.clone();
                    row(ui, |ui| {
                        ui.label(label.clone());
                        let out = egui::TextEdit::singleline(buffer)
                            .desired_width(ui.available_width() - 28.0)
                            .show(ui);
                        let re = out.response.response;
                        // Name the input (the Options dialog's rule —
                        // kittest's a11y check requires it).
                        re.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, label.as_str())
                        });
                        if let Some(cr) = out.cursor_range {
                            state.caret = Some(cr.primary.index.0);
                            state.caret_in_replacement = is_repl;
                        }
                        edit_focused |= re.has_focus();
                        let hist =
                            ui.add_enabled(!history.is_empty(), egui::Button::new("›").small());
                        hist.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::ComboBox,
                                !history.is_empty(),
                                label.as_str(),
                            )
                        });
                        egui::Popup::menu(&hist)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                            .show(|ui| {
                                ui.set_min_width(150.0);
                                let entries = history.clone();
                                for e in entries {
                                    if ui.selectable_label(false, e.clone()).clicked() {
                                        *buffer = e;
                                    }
                                }
                            });
                    });
                }
                // 3.1: the Special menu below Replace with.
                let sp = ui.button(format!("{}  ›", s.msg_special()));
                egui::Popup::menu(&sp)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(170.0);
                        let labels = [
                            s.msg_find_tab_character(),
                            s.msg_find_manual_return(),
                            s.msg_find_optional_hyphen(),
                            s.msg_find_caret_character(),
                            s.msg_find_any_character(),
                        ];
                        for (code, label) in SPECIAL_CODES.iter().zip(labels) {
                            if ui.add(egui::Button::new(label).small()).clicked() {
                                let buffer = if state.caret_in_replacement {
                                    &mut state.replacement
                                } else {
                                    &mut state.session.query
                                };
                                insert_code(buffer, state.caret, code);
                            }
                        }
                    });
                ui.add_space(6.0);
                // 3.2: "Search" — the scope radios HORIZONTAL (spec
                // 3's layout difference); no target checkboxes.
                ui.strong(s.msg_search());
                row(ui, |ui| {
                    ui.radio_value(
                        &mut state.session.scope,
                        FindScopeKind::Selection,
                        s.msg_selection(),
                    );
                    ui.radio_value(
                        &mut state.session.scope,
                        FindScopeKind::CurrentPage,
                        s.msg_current_page(),
                    );
                    ui.radio_value(
                        &mut state.session.scope,
                        FindScopeKind::AllPages,
                        s.msg_all_pages(),
                    );
                });
                ui.add_space(6.0);
                // 3.3: Options — the modifiers in a two-column grid.
                ui.strong(s.msg_options());
                row(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.checkbox(&mut state.session.match_case, s.msg_match_case());
                        ui.checkbox(
                            &mut state.session.whole_words,
                            s.msg_find_whole_words_only(),
                        );
                    });
                    ui.vertical(|ui| {
                        ui.checkbox(
                            &mut state.session.match_width,
                            s.msg_match_character_width(),
                        );
                    });
                });
                ui.separator();
                ui.add_space(4.0);
                // 3.4: Help at the bottom left; the actions at the
                // bottom right — Replace (enabled only when a match
                // is current), Replace All, Find Next (the default),
                // Cancel.
                row(ui, |ui| {
                    let help = ui.button(egui::RichText::new("?").small());
                    help.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, s.msg_help().as_str())
                    });
                    if help.clicked() {
                        outcome = ReplaceOutcome::Help;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(s.msg_cancel()).clicked() {
                            outcome = ReplaceOutcome::Close;
                        }
                        if ui
                            .button(egui::RichText::new(s.msg_find_next()).strong())
                            .clicked()
                        {
                            outcome = ReplaceOutcome::FindNext;
                        }
                        if ui.button(s.msg_replace_all()).clicked() {
                            outcome = ReplaceOutcome::ReplaceAll;
                        }
                        let can_replace =
                            state.session.current.is_some() && !state.session.query.is_empty();
                        if ui
                            .add_enabled(can_replace, egui::Button::new(s.msg_replace()))
                            .clicked()
                        {
                            outcome = ReplaceOutcome::Replace;
                        }
                    });
                });
            });
        });
    if !open {
        outcome = ReplaceOutcome::Close;
    }
    // Enter in either field is the default button.
    if outcome == ReplaceOutcome::Open
        && edit_focused
        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
    {
        outcome = ReplaceOutcome::FindNext;
    }
    outcome
}
