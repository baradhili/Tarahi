//! File ▸ Options (GitHub #268) — the Visio-2016-style Options dialog
//! behind the backstage rail's Options entry, with **General** (user
//! name/initials — the Comments author field), **Save** (AutoRecover
//! interval + location) and **Language** (the display-language choice,
//! including the "match system locale" entry mirroring first-run
//! detection) as first-class pages; Proofing and Advanced stay grayed
//! "planned" rows until their waves land (each grayed row keeps its
//! Visio name — never invented sections).
//!
//! This module owns the domain (the app.rs decomposition direction):
//! the app-level options store (`options.json` in the user-config dir,
//! the `recent.json` pattern), the dialog draft, and the dialog
//! window. `app.rs` keeps only the state fields, the backstage glue,
//! and the outcome forwarding. Everything the dialog edits is
//! app-level — nothing here writes into the `.vsdx`.
//!
//! The dialog contract is the Hyperlinks dialog's (#71): Enter = OK,
//! Apply commits without closing, Cancel / Esc discard the draft and
//! close. It is a true MODAL (the #166 `egui::Modal` pattern) — the
//! backdrop blocks the backstage behind it, and a backdrop click is
//! ignored (it never discards edits). The dialog manages its own Esc
//! (#110 audit) — the backstage's Esc guard skips while it is up.

use std::path::{Path, PathBuf};

use egui::Context;

use crate::i18n::{self, L10n};

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The app-level options (#268). Persisted as `options.json` beside
/// `recent.json` / `language.json` — written on the first COMMIT, not
/// at startup, so a user who never opens Options never writes config.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The Comments author name (Visio's General-page user name);
    /// empty falls back to the environment (the pre-#268 behavior).
    pub user_name: String,
    /// The comment-avatar initials for the configured user; empty
    /// derives from the name.
    pub user_initials: String,
    /// Save AutoRecover information (Visio's Save page checkbox).
    pub autorecover: bool,
    /// The AutoRecover interval, minutes (Visio's default 10). The
    /// settings are stored here; the timer itself is the AutoRecover
    /// issue's scope.
    pub autorecover_minutes: u32,
    /// The AutoRecover file location; empty = the app default.
    pub autorecover_dir: String,
    /// Where the options persist; `None` = in-memory (ephemeral
    /// constructors — headless tests never touch the user's config).
    store: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            user_name: String::new(),
            user_initials: String::new(),
            autorecover: true,
            autorecover_minutes: 10,
            autorecover_dir: String::new(),
            store: None,
        }
    }
}

impl Options {
    /// A non-persisting copy of the defaults (headless tests, kittest).
    pub fn ephemeral() -> Options {
        Options::default()
    }

    /// Load from the user-config path. A missing file is the defaults,
    /// unpersisted — the file lands on the first commit; a corrupt or
    /// unknown-version file falls back to the defaults without
    /// overwriting it.
    pub fn open() -> Options {
        let Some(path) = crate::recent::config_base()
            .map(|b| b.join("diagramcraft").join("options.json"))
        else {
            return Options::ephemeral();
        };
        let mut o = match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_default(),
            Err(_) => Options::default(),
        };
        o.store = Some(path);
        o
    }

    /// Parse the JSON form; `None` on any structural mismatch or a
    /// schema version this build does not know.
    fn parse(text: &str) -> Option<Options> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        if v.get("version")?.as_i64()? != 1 {
            return None;
        }
        let get_str =
            |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(Options {
            user_name: get_str("user_name"),
            user_initials: get_str("user_initials"),
            autorecover: v
                .get("autorecover")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            autorecover_minutes: v
                .get("autorecover_minutes")
                .and_then(|x| x.as_u64())
                .map(|m| m.clamp(1, 120) as u32)
                .unwrap_or(10),
            autorecover_dir: get_str("autorecover_dir"),
            store: None,
        })
    }

    /// Whether this store persists (production) — the Language page's
    /// commit picks the persistent or ephemeral `i18n` call
    /// accordingly (the headless pinning rule).
    pub(crate) fn persists(&self) -> bool {
        self.store.is_some()
    }

    /// Commit a dialog draft into the store (and persist when not
    /// ephemeral). The interval clamps to 1..=120 minutes; garbage
    /// keeps the previous value; the name/initials trim.
    pub(crate) fn apply(&mut self, d: &Draft) {
        self.user_name = d.user_name.trim().to_string();
        self.user_initials = d.user_initials.trim().to_string();
        self.autorecover = d.autorecover;
        if let Ok(m) = d.autorecover_minutes.trim().parse::<u32>() {
            self.autorecover_minutes = m.clamp(1, 120);
        }
        self.autorecover_dir = d.autorecover_dir.clone();
        if self.store.is_some() {
            self.persist();
        }
    }

    /// Serialize + write (best-effort — a read-only config dir must
    /// not break the commit), returning whether the file landed.
    fn persist_to(&self, path: &Path) -> bool {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = serde_json::json!({
            "version": 1,
            "user_name": self.user_name,
            "user_initials": self.user_initials,
            "autorecover": self.autorecover,
            "autorecover_minutes": self.autorecover_minutes,
            "autorecover_dir": self.autorecover_dir,
        })
        .to_string();
        std::fs::write(path, text).is_ok()
    }

    fn persist(&self) -> bool {
        match self.store.as_ref() {
            Some(path) => self.persist_to(path),
            None => false,
        }
    }

    /// Test seam: persist against an explicit path (the real persist
    /// uses config_base(); the logic is shared).
    #[doc(hidden)]
    pub fn persist_to_for_test(&self, path: &Path) -> bool {
        self.persist_to(path)
    }

    /// Test seam: parse the JSON form.
    #[doc(hidden)]
    pub fn parse_for_test(text: &str) -> Option<Options> {
        Self::parse(text)
    }
}

// ---------------------------------------------------------------------------
// Draft + outcome
// ---------------------------------------------------------------------------

/// The Language page's choice: a concrete catalog, or the
/// match-the-system-locale entry mirroring first-run detection (#268).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangChoice {
    System,
    Concrete(L10n),
}

/// The first-class left-nav pages. Proofing and Advanced are grayed
/// rows in the nav, not pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Save,
    Language,
}

impl Page {
    /// kittest: a page by name ("general" | "save" | "language").
    pub(crate) fn from_test_name(name: &str) -> Option<Page> {
        match name {
            "general" => Some(Page::General),
            "save" => Some(Page::Save),
            "language" => Some(Page::Language),
            _ => None,
        }
    }
}

/// The dialog's edit buffer: captured when the dialog opens, mutated
/// freely per page, committed by OK/Apply, discarded by Cancel/Esc/✕
/// (the #71 contract).
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub page: Page,
    pub user_name: String,
    pub user_initials: String,
    pub autorecover: bool,
    /// The interval as typed (a text field; parsed + clamped on
    /// commit).
    pub autorecover_minutes: String,
    pub autorecover_dir: String,
    pub lang: LangChoice,
}

impl Draft {
    /// The dialog's opening state: the live store values plus the
    /// caller-resolved language choice (match-system while no explicit
    /// language is persisted — the global read stays in app.rs's glue,
    /// so capture stays pure for the unit tests, #268).
    pub(crate) fn capture(o: &Options, lang: LangChoice) -> Draft {
        Draft {
            page: Page::General,
            user_name: o.user_name.clone(),
            user_initials: o.user_initials.clone(),
            autorecover: o.autorecover,
            autorecover_minutes: o.autorecover_minutes.to_string(),
            autorecover_dir: o.autorecover_dir.clone(),
            lang,
        }
    }

    /// Sync the committed store values back into the draft after an
    /// Apply — a clamped interval or trimmed name corrects visibly on
    /// the next paint; the page and language choice stay as the user
    /// left them.
    pub(crate) fn sync_committed(&mut self, o: &Options) {
        self.user_name = o.user_name.clone();
        self.user_initials = o.user_initials.clone();
        self.autorecover = o.autorecover;
        self.autorecover_minutes = o.autorecover_minutes.to_string();
        self.autorecover_dir = o.autorecover_dir.clone();
    }
}

/// The dialog's terminal result for this frame (#71 contract). `app.rs`
/// commits on `Ok`/`Apply` and discards on `Cancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    None,
    Ok,
    Apply,
    Cancel,
}

// ---------------------------------------------------------------------------
// Dialog window
// ---------------------------------------------------------------------------

/// Render the Options dialog. `freshly_opened` is true on the frame
/// the dialog opened — the Enter that ACTIVATED the backstage button
/// is still in the input then, so the key checks skip it once (the
/// CodeRabbit round); the returned outcome drives app.rs's
/// commit/discard glue.
pub(crate) fn window(ctx: &Context, draft: &mut Draft, freshly_opened: bool) -> Outcome {
    let mut outcome = Outcome::None;
    // MODAL (#166 precedent; the CodeRabbit round): the backdrop
    // blocks input to the backstage behind the dialog — a Back/rail/
    // page-content click behind it must never strand the draft (the
    // dialog renders only in the backstage branch). The title rides
    // inside the frame (a modal has no title bar); Windows-dialog
    // semantics: a backdrop click is IGNORED — it never discards
    // edits — and Esc cancels.
    egui::Modal::new(egui::Id::unique("dc-options")).show(ctx, |ui| {
        ui.with_layout(crate::app::dialog_dir(), |ui| {
            ui.strong(i18n::strs().msg_options());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                nav(ui, draft);
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width(400.0);
                    match draft.page {
                        Page::General => general_page(ui, draft),
                        Page::Save => save_page(ui, draft),
                        Page::Language => language_page(ui, draft),
                    }
                });
            });
            ui.add_space(10.0);
            crate::app::row_rev(ui, |ui| {
                if ui.button(i18n::strs().msg_ok()).clicked() {
                    outcome = Outcome::Ok;
                }
                if ui.button(i18n::strs().msg_apply()).clicked() {
                    outcome = Outcome::Apply;
                }
                if ui.button(i18n::strs().msg_cancel()).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });
    });
    // #71 contract: the dialog manages its own Esc — Esc discards the
    // draft and closes (the backstage's Esc guard skips while this
    // dialog is up). #110 audit: Enter is the dialog's OK. Both skip
    // the opening frame (see `freshly_opened`).
    if freshly_opened {
        return outcome;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        outcome = Outcome::Cancel;
    } else if ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
        outcome = Outcome::Ok;
    }
    outcome
}

/// The left category list (Visio's nav): the first-class pages pick
/// the draft's page; Proofing and Advanced are grayed planned rows
/// that keep their Visio names.
fn nav(ui: &mut egui::Ui, draft: &mut Draft) {
    let visuals = ui.visuals().clone();
    let mut pick = None;
    ui.vertical(|ui| {
        ui.set_min_width(150.0);
        let rows: [(Option<Page>, String, Option<String>); 5] = [
            (
                Some(Page::General),
                i18n::strs().msg_options_general(),
                None,
            ),
            (
                None,
                i18n::strs().msg_options_proofing(),
                Some(i18n::strs().msg_options_proofing_planned()),
            ),
            (Some(Page::Save), i18n::strs().msg_save(), None),
            (Some(Page::Language), i18n::strs().msg_language(), None),
            (
                None,
                i18n::strs().msg_options_advanced(),
                Some(i18n::strs().msg_options_advanced_planned()),
            ),
        ];
        for (page, label, tip) in rows {
            let selected = page == Some(draft.page);
            let mut text = egui::RichText::new(label).size(13.0);
            if selected {
                text = text.strong();
            }
            let mut b = egui::Button::new(text).frame(false);
            if selected {
                b = b.fill(visuals.selection.bg_fill);
            }
            let resp = match (page, tip) {
                (Some(_), _) => ui.add(b),
                // A grayed row keeps its Visio name; the planned note
                // rides the disabled hover tip.
                (None, Some(tip)) => ui
                    .add_enabled(false, b)
                    .on_disabled_hover_text(tip),
                (None, None) => ui.add_enabled(false, b),
            };
            if let (Some(p), true) = (page, resp.clicked()) {
                pick = Some(p);
            }
        }
    });
    if let Some(p) = pick {
        draft.page = p;
    }
}

/// The General page: user name + initials — the Comments author field
/// (the name feeds `User.AnnotationAuthor`; the initials the comment
/// avatar).
fn general_page(ui: &mut egui::Ui, draft: &mut Draft) {
    let name_label = i18n::strs().msg_options_user_name();
    ui.label(&name_label);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft.user_name).desired_width(240.0),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &name_label));
    ui.add_space(6.0);
    let initials_label = i18n::strs().msg_options_initials();
    ui.label(&initials_label);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft.user_initials).desired_width(80.0),
    );
    r.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::TextInput, true, &initials_label)
    });
}

/// The Save page: AutoRecover interval + location (the settings the
/// AutoRecover issue consumes). An empty location is the app default.
fn save_page(ui: &mut egui::Ui, draft: &mut Draft) {
    let every = i18n::strs().msg_options_save_autorecover();
    crate::app::row(ui, |ui| {
        ui.checkbox(&mut draft.autorecover, &every);
        let r = ui.add(
            egui::TextEdit::singleline(&mut draft.autorecover_minutes)
                .desired_width(40.0),
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &every));
        ui.label(i18n::strs().msg_options_minutes());
    });
    ui.add_space(10.0);
    let loc = i18n::strs().msg_options_autorecover_location();
    ui.label(&loc);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft.autorecover_dir).desired_width(300.0),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &loc));
    ui.weak(i18n::strs().msg_options_autorecover_location_hint());
}

/// The Language page: the display-language switch (the substance —
/// the View ▸ Window dialog stays the quick shortcut; both surfaces
/// share `i18n::set_language`), plus the grayed editing-languages
/// placeholder (#121/#298 own proofing).
fn language_page(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.weak(i18n::strs().msg_choose_the_display_language_for());
    ui.add_space(4.0);
    ui.strong(i18n::strs().msg_display_language());
    ui.add_space(4.0);
    if ui
        .radio(
            draft.lang == LangChoice::System,
            i18n::strs().msg_options_match_system_locale(),
        )
        .clicked()
    {
        draft.lang = LangChoice::System;
    }
    // The resolved system language carries the "(default)" marker
    // while the system-match choice is active (#268).
    let marker = if i18n::is_system_match() {
        format!(" {}", i18n::strs().msg_options_default_marker())
    } else {
        String::new()
    };
    for lang in L10n::iter() {
        let label = if i18n::is_system_match() && *lang == i18n::system_locale() {
            format!("{}{}", lang.language_name(), marker)
        } else {
            lang.language_name().to_string()
        };
        if ui
            .radio(draft.lang == LangChoice::Concrete(*lang), label)
            .clicked()
        {
            draft.lang = LangChoice::Concrete(*lang);
        }
    }
    ui.add_space(10.0);
    ui.strong(i18n::strs().msg_options_editing_languages());
    ui.weak(i18n::strs().msg_options_editing_languages_planned());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_visio_save_page() {
        let o = Options::ephemeral();
        assert!(o.autorecover);
        assert_eq!(o.autorecover_minutes, 10);
        assert!(o.user_name.is_empty());
        assert!(o.user_initials.is_empty());
        assert!(o.autorecover_dir.is_empty());
        assert!(!o.persists());
    }

    #[test]
    fn roundtrips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("options.json");
        let mut o = Options::ephemeral();
        o.user_name = "Bret".into();
        o.user_initials = "BW".into();
        o.autorecover = false;
        o.autorecover_minutes = 15;
        o.autorecover_dir = "/tmp/autorecover".into();
        assert!(o.persist_to_for_test(&path));
        let loaded =
            Options::parse_for_test(&std::fs::read_to_string(&path).unwrap())
                .expect("round-trips");
        assert_eq!(loaded.user_name, "Bret");
        assert_eq!(loaded.user_initials, "BW");
        assert!(!loaded.autorecover);
        assert_eq!(loaded.autorecover_minutes, 15);
        assert_eq!(loaded.autorecover_dir, "/tmp/autorecover");
    }

    #[test]
    fn corrupt_file_parses_to_none() {
        assert_eq!(Options::parse_for_test("{not json"), None);
    }

    #[test]
    fn future_schema_version_parses_to_none() {
        assert_eq!(
            Options::parse_for_test(r#"{"version":99,"user_name":"x"}"#),
            None
        );
    }

    #[test]
    fn missing_fields_fall_back_to_their_defaults() {
        let o = Options::parse_for_test(r#"{"version":1}"#).expect("version ok");
        assert_eq!(o, Options::ephemeral());
    }

    #[test]
    fn apply_clamps_the_interval_and_trims_the_name() {
        let mut o = Options::ephemeral();
        o.autorecover_minutes = 10;
        let d = Draft {
            page: Page::General,
            user_name: "  Bret Watson  ".into(),
            user_initials: " bw ".into(),
            autorecover: true,
            autorecover_minutes: "0".into(),
            autorecover_dir: String::new(),
            lang: LangChoice::Concrete(L10n::En),
        };
        o.apply(&d);
        assert_eq!(o.user_name, "Bret Watson");
        assert_eq!(o.user_initials, "bw");
        assert_eq!(o.autorecover_minutes, 1, "the interval clamps to 1..=120");
    }

    #[test]
    fn apply_keeps_the_previous_interval_on_garbage() {
        let mut o = Options::ephemeral();
        o.autorecover_minutes = 10;
        let d = Draft {
            page: Page::Save,
            user_name: String::new(),
            user_initials: String::new(),
            autorecover: true,
            autorecover_minutes: "abc".into(),
            autorecover_dir: String::new(),
            lang: LangChoice::Concrete(L10n::En),
        };
        o.apply(&d);
        assert_eq!(o.autorecover_minutes, 10);
    }

    #[test]
    fn capture_reflects_the_store_and_the_passed_choice() {
        let mut o = Options::ephemeral();
        o.autorecover_minutes = 12;
        let d = Draft::capture(&o, LangChoice::Concrete(L10n::De));
        assert_eq!(d.page, Page::General);
        assert_eq!(d.autorecover_minutes, "12");
        assert_eq!(d.lang, LangChoice::Concrete(L10n::De));
        let d = Draft::capture(&o, LangChoice::System);
        assert_eq!(d.lang, LangChoice::System);
    }
}
