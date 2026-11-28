//! The Find dialog (GitHub #115 phase 2, Functional Specification 2):
//! the modeless window — Find what + history dropdown, the Special
//! caret-code menu, Search-in radios (current page default), the four
//! target checkboxes (shape text default-checked), the Options
//! modifiers, the "Found in:" status area, and Find Next as the
//! default action. Pure UI over [`FindState`]; the app side
//! (`app/find.rs`) computes matches, navigates, and owns the ribbon
//! menu.

use egui::Context;

use crate::app::{dialog_dir, row};

/// Which pages a search covers — spec 2's radios.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindScopeKind {
    Selection,
    CurrentPage,
    AllPages,
}

/// The "Found in:" status area's content.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FindStatus {
    /// Nothing searched yet — the area stays blank.
    Idle,
    /// The last Find Next landed here.
    Found { page: String, shape: String },
    /// The search ran and matched nothing.
    NotFound,
}

/// What the dialog asked the app for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindOutcome {
    Open,
    FindNext,
    Help,
    Close,
}

/// Everything a Find-Next run keys its cached match list on: any
/// change — query, scope, targets, options, the selection the
/// Selection scope reads, the current page the Current-page scope
/// reads, or the document itself (the undo-stack depth is the
/// revision) — recomputes from the first match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FindKey {
    pub query: String,
    pub scope: FindScopeKind,
    pub page: Option<i64>,
    pub selection: Vec<(i64, i64)>,
    pub targets: dc_core::FindTargets,
    pub match_case: bool,
    pub match_width: bool,
    pub whole_words: bool,
    /// The document revision (`App::version`).
    pub rev: u64,
}

/// The search session shared by the Find and Replace windows: the
/// query and modifiers, the cached match list with its key and
/// Find-Next cursor, and the navigation bookkeeping. The Replace
/// window fixes `targets` to every class (spec 3 carries no target
/// checkboxes; the decision is recorded on #115).
#[derive(Debug, Clone)]
pub(crate) struct FindSession {
    pub query: String,
    /// Previous Find-what strings, most recent first, bounded — the
    /// session's own history (session lifetime, the #115 decision).
    pub history: Vec<String>,
    pub scope: FindScopeKind,
    pub targets: dc_core::FindTargets,
    pub match_case: bool,
    pub match_width: bool,
    pub whole_words: bool,
    /// The session's match list, the Find-Next cursor (the index of
    /// the NEXT match to show, wrapping past the last), and the key
    /// the list was computed under.
    pub matches: Vec<dc_core::FindMatch>,
    pub cursor: usize,
    pub key: Option<FindKey>,
    /// The selection as Find Next's own navigation left it — while
    /// the live selection still equals this, the Selection scope's
    /// key keeps the selection the search STARTED from (the user
    /// changing the selection re-scopes; the search itself must not).
    pub nav_sel: Option<Vec<(i64, i64)>>,
    /// The match the last Find Next highlighted — Replace's gate
    /// (spec 3: enabled "only when a match is highlighted by Find
    /// Next"). Cleared whenever the key changes (stale coordinates).
    pub current: Option<dc_core::FindMatch>,
}

impl Default for FindSession {
    fn default() -> Self {
        FindSession {
            query: String::new(),
            history: Vec::new(),
            // Spec 2: "Current page (Default selected state)" — and
            // spec 3's default for the Replace dialog too.
            scope: FindScopeKind::CurrentPage,
            // Spec 2: "Shape text (Default checked)" — the engine's
            // `FindTargets::default` is exactly that set. The Replace
            // window overrides to `FindTargets::all()` (fixed).
            targets: dc_core::FindTargets::default(),
            match_case: false,
            match_width: false,
            whole_words: false,
            matches: Vec::new(),
            cursor: 0,
            key: None,
            nav_sel: None,
            current: None,
        }
    }
}

/// The Find window's session state. History is session-lifetime (the
/// implementation-time decision recorded on #115: it never outlives
/// the app, nothing persisted into settings).
#[derive(Debug, Clone)]
pub(crate) struct FindState {
    pub open: bool,
    /// The Find-what caret (CHAR index) the Special menu inserts at,
    /// captured from the edit's cursor; `None` appends at the end.
    caret: Option<usize>,
    pub status: FindStatus,
    pub session: FindSession,
}

impl Default for FindState {
    fn default() -> Self {
        FindState {
            open: false,
            caret: None,
            status: FindStatus::Idle,
            session: FindSession::default(),
        }
    }
}

/// Insert `code` into the query at the captured caret (CHAR index),
/// appending at the end when the edit never reported one.
pub(crate) fn insert_code(query: &mut String, caret: Option<usize>, code: &str) {
    let at = caret.unwrap_or_else(|| query.chars().count());
    let byte = query
        .char_indices()
        .nth(at)
        .map(|(b, _)| b)
        .unwrap_or(query.len());
    query.insert_str(byte, code);
}

/// The Special menu's caret codes, in spec 2's order.
pub(crate) const SPECIAL_CODES: &[&str] = &["^t", "^p", "^-", "^^", "^?"];

/// Draw the Find window. Enter runs Find Next when the Find-what edit
/// holds focus (the default-button behavior of a modeless window).
pub(crate) fn window(ctx: &Context, state: &mut FindState) -> FindOutcome {
    let mut outcome = FindOutcome::Open;
    let mut open = true;
    let mut edit_focused = false;
    egui::Window::new(crate::i18n::strs().msg_find())
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(340.0)
        .show(ctx, |ui| {
            let s = crate::i18n::strs();
            ui.with_layout(dialog_dir(), |ui| {
                // 3.1: the query field with its history dropdown.
                row(ui, |ui| {
                    let what = s.msg_find_what();
                    ui.label(what.clone());
                    let out = egui::TextEdit::singleline(&mut state.session.query)
                        .desired_width(ui.available_width() - 28.0)
                        .show(ui);
                    let re = out.response.response;
                    // Name the input (the Options dialog's rule —
                    // kittest's a11y check requires it).
                    re.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, what.as_str())
                    });
                    if let Some(cr) = out.cursor_range {
                        state.caret = Some(cr.primary.index.0);
                    }
                    edit_focused = re.has_focus();
                    // The history dropdown arrow (the spell dialog's
                    // language-dropdown pattern).
                    let hist = ui.add_enabled(
                        !state.session.history.is_empty(),
                        egui::Button::new("›").small(),
                    );
                    hist.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::ComboBox,
                            !state.session.history.is_empty(),
                            s.msg_find_what().as_str(),
                        )
                    });
                    egui::Popup::menu(&hist)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                        .show(|ui| {
                            ui.set_min_width(150.0);
                            let entries = state.session.history.clone();
                            for e in entries {
                                if ui.selectable_label(false, e.clone()).clicked() {
                                    state.session.query = e;
                                }
                            }
                        });
                });
                // 3.1: the Special menu below the input.
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
                                insert_code(&mut state.session.query, state.caret, code);
                            }
                        }
                    });
                ui.add_space(6.0);
                // 3.2: Search in — scope radios, target checkboxes in
                // the column to the right.
                ui.strong(s.msg_search_in());
                row(ui, |ui| {
                    ui.vertical(|ui| {
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
                    ui.vertical(|ui| {
                        ui.checkbox(&mut state.session.targets.text, s.msg_find_shape_text());
                        ui.checkbox(
                            &mut state.session.targets.shape_data,
                            s.msg_find_shape_data(),
                        );
                        ui.checkbox(
                            &mut state.session.targets.shape_name,
                            s.msg_find_shape_name(),
                        );
                        ui.checkbox(
                            &mut state.session.targets.user_cell,
                            s.msg_find_user_defined_cell(),
                        );
                    });
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
                // 3.4: the status area (bottom left).
                match &state.status {
                    FindStatus::Idle => {}
                    FindStatus::Found { page, shape } => {
                        ui.weak(s.msg_found_in(page.clone(), shape.clone()));
                    }
                    FindStatus::NotFound => {
                        ui.weak(s.msg_find_not_found());
                    }
                }
                ui.add_space(4.0);
                // 3.4: Help at the bottom left; the actions at the
                // bottom right with Find Next as the highlighted
                // default.
                row(ui, |ui| {
                    let help = ui.button(egui::RichText::new("?").small());
                    help.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, s.msg_help().as_str())
                    });
                    if help.clicked() {
                        outcome = FindOutcome::Help;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(s.msg_cancel()).clicked() {
                            outcome = FindOutcome::Close;
                        }
                        if ui
                            .button(egui::RichText::new(s.msg_find_next()).strong())
                            .clicked()
                        {
                            outcome = FindOutcome::FindNext;
                        }
                    });
                });
            });
        });
    if !open {
        outcome = FindOutcome::Close;
    }
    // Enter in the query field is the default button.
    if outcome == FindOutcome::Open
        && edit_focused
        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
    {
        outcome = FindOutcome::FindNext;
    }
    outcome
}
