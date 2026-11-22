//! #246 i18n — the shell's language runtime over the fluent-typed
//! generated accessors ([`crate::l10n`]).
//!
//! Layout: the catalogs live at PROJECT level (`locales/<lang>/app.ftl`
//! at the workspace root — a new language never digs through the code);
//! `build.rs` generates the typed accessors into `src/l10n.rs`, which is
//! committed, and joins the translations into `gen/translations.ftl`
//! which the accessors embed. flynt lints the catalogs (`.flynt.toml`);
//! `crates/dc-ui/tests/i18n.rs` is the usage gate (completeness, orphans,
//! literal scan) plus the language-switch kittest coverage.
//!
//! Call sites never hold user-facing English: they read message data off
//! the current language's generated struct via [`strs()`] — e.g.
//! `i18n::strs().msg_align_center()` — and pass raw numbers/strings as
//! typed arguments. Plurals, punctuation and word order belong in the
//! `.ftl` files, never in Rust.
//!
//! The language is process-global: the production constructor calls
//! [`init_from_disk()`] (persisted choice in `language.json`, else the
//! system locale, else English); the headless kittest constructors pin
//! English via [`pin_english()`] so goldens never move. Switching calls
//! [`set_language`] — the next frame re-renders in the new language; no
//! stored widget strings mutate (localization happens at the render
//! boundary).

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub use crate::l10n::{L10n, L10nLanguage};
use crate::recent::config_base;

const SCHEMA_VERSION: u64 = 1;

/// Slot index into [`L10n::iter`] — the process-global current language.
/// `u8::MAX` = unset, so `current()` falls through to `L10n::default()`
/// (English): `L10n::iter()` starts at German, and a bare `0` would
/// make every unpinned reader — unit tests, the pre-init window title —
/// resolve German.
static CURRENT: AtomicU8 = AtomicU8::new(u8::MAX);

/// Whether the active language came from the SYSTEM locale rather than
/// an explicit persisted choice (#268): first run, a cleared choice
/// (`language.json` without a language key), or an unreadable file.
/// The Options dialog's Language page marks the resolved system
/// language "(default)" while this is set.
static SYSTEM_MATCH: AtomicBool = AtomicBool::new(false);

/// True while the language resolves from the system locale instead of
/// an explicit persisted choice (#268).
pub fn is_system_match() -> bool {
    SYSTEM_MATCH.load(Ordering::Relaxed)
}

/// English pinned (headless constructors, tests): no persistence.
pub fn pin_english() {
    set_current(L10n::En);
}

/// The active language.
pub fn current() -> L10n {
    L10n::iter()
        .nth(CURRENT.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or_default()
}

fn set_current(lang: L10n) {
    let ix = L10n::iter()
        .position(|l| *l == lang)
        .expect("language from L10n::iter() is always present");
    CURRENT.store(ix as u8, Ordering::Relaxed);
}

/// Switch the language and persist the choice app-level (never in the
/// `.vsdx` — DocumentSettings' schema is closed). Applies on the next
/// frame: call sites read [`strs()`] fresh, no stored strings mutate.
pub fn set_language(lang: L10n) {
    set_current(lang);
    SYSTEM_MATCH.store(false, Ordering::Relaxed);
    persist(lang);
}

/// Switch without persisting (tests, previews).
pub fn set_language_ephemeral(lang: L10n) {
    set_current(lang);
    SYSTEM_MATCH.store(false, Ordering::Relaxed);
}

/// Switch to "match the system locale" (#268): the language re-resolves
/// from the environment now and on the next run (`language.json` loses
/// its explicit language key). Applies on the next frame like
/// [`set_language`].
pub fn set_system_default() {
    set_current(detect_system_locale());
    SYSTEM_MATCH.store(true, Ordering::Relaxed);
    persist_system();
}

/// The ephemeral twin of [`set_system_default`] (tests, previews): no
/// persistence.
pub fn set_system_default_ephemeral() {
    set_current(detect_system_locale());
    SYSTEM_MATCH.store(true, Ordering::Relaxed);
}

/// The system locale's language, re-read at call time — the Language
/// page marks it "(default)" while a system match is active (#268).
pub fn system_locale() -> L10n {
    detect_system_locale()
}

/// The NATIVE-SURFACE boundary (GitHub #280): Fluent wraps every
/// placeable in bidi-isolation marks (`use_isolating: true` — on
/// build-wide since #257 item 3 re-enabled it for the RTL chrome).
/// Inside egui the marks are zero-width and shape away, but a string
/// that LEAVES egui meets the window manager's font, which has no
/// glyph for U+2066–U+2069 and paints tofu — the fresh-document
/// window title (`drawing-n = Drawing{ $n }`) showed it either side
/// of the 1. This strips the isolation marks (FSI/PDI/LRI/RLI) at
/// that boundary and must wrap every Fluent output handed to a
/// non-egui surface — the OS window title today; the OS clipboard is
/// NOT one (it carries shape text, never catalog output). Do NOT
/// apply to text that stays inside egui: the marks are load-bearing
/// inside RTL sentences (#257) and stripping them there would change
/// run ordering.
pub fn to_native_surface(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{2066}'..='\u{2069}'))
        .collect()
}

/// The RTL language set (#257 item 3): the shell chrome mirrors for
/// these. Arabic/Hebrew catalogs join the set with their folders.
const RTL_LANGS: &[L10n] = &[L10n::Fa];

/// True while the current language reads right-to-left: the shell
/// mirrors its chrome for it (ribbon order, panel sides, strip and
/// status alignment, horizontal-ruler zero direction).
pub fn rtl() -> bool {
    RTL_LANGS.contains(&current())
}

/// The current language's generated accessors, loaded once and leaked
/// per language (bounded by [`L10n::iter`]). Every chrome string in the
/// shell resolves through this.
pub fn strs() -> &'static L10nLanguage {
    strs_for(current())
}

/// One language's generated accessors, loaded once and leaked per
/// language (bounded by [`L10n::iter`]). #257: the RTL keytip scheme
/// reads the ENGLISH labels through this — the Latin anchors a
/// non-Latin chrome keys its letters off — while the visible strings
/// stay on [`strs()`].
pub fn strs_for(lang: L10n) -> &'static L10nLanguage {
    static LOADED: Mutex<Vec<Option<&'static L10nLanguage>>> = Mutex::new(Vec::new());
    let ix = L10n::iter().position(|l| *l == lang).unwrap_or(0);
    let mut slots = LOADED.lock().unwrap();
    slots.resize_with(L10n::iter().len(), || None);
    if let Some(loaded) = slots[ix] {
        return loaded;
    }
    let loaded: &'static L10nLanguage = Box::leak(Box::new(lang.load()));
    slots[ix] = Some(loaded);
    loaded
}

/// Load the persisted choice (or the system locale on first run) —
/// the production constructor calls this once at startup.
pub fn init_from_disk() {
    let text = store_path().and_then(|p| std::fs::read_to_string(p).ok());
    match text.as_deref().map(parse_persisted) {
        Some(Persisted::Language(lang)) => {
            set_current(lang);
            SYSTEM_MATCH.store(false, Ordering::Relaxed);
        }
        // An explicit system-match choice (or anything unreadable —
        // the first-run behavior): resolve from the environment.
        _ => {
            set_current(detect_system_locale());
            SYSTEM_MATCH.store(true, Ordering::Relaxed);
        }
    }
}

fn store_path() -> Option<PathBuf> {
    config_base().map(|b| b.join("diagramcraft").join("language.json"))
}

fn persist(lang: L10n) {
    let Some(path) = store_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        &path,
        serde_json::json!({ "version": SCHEMA_VERSION, "language": lang.to_string() }).to_string(),
    );
}

/// Persist the "match the system locale" choice (#268): `language.json`
/// WITHOUT a language key — the schema version stays, the choice reads
/// back as [`Persisted::System`].
fn persist_system() {
    let Some(path) = store_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::json!({ "version": SCHEMA_VERSION }).to_string());
}

/// The on-disk form of `language.json` (#268): an explicit language, or
/// the match-the-system-locale choice (the key absent or null — a file
/// without one is the Options Language page's "Match system locale").
enum Persisted {
    System,
    Language(L10n),
}

/// Parse the on-disk form. Anything unreadable, corrupt, or carrying a
/// schema version this build does not know reads as [`Persisted::System`]
/// — the pre-#268 first-run behavior for all of those.
fn parse_persisted(text: &str) -> Persisted {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return Persisted::System;
    };
    if v.get("version").and_then(|x| x.as_u64()) != Some(SCHEMA_VERSION) {
        return Persisted::System;
    }
    match v.get("language") {
        None | Some(serde_json::Value::Null) => Persisted::System,
        Some(lang) => lang
            .as_str()
            .and_then(|s| s.parse().ok())
            .map(Persisted::Language)
            .unwrap_or(Persisted::System),
    }
}

/// Parse the on-disk form's explicit language; `None` for anything
/// unreadable, the keyless system-match form, or a schema version this
/// build does not know. (Test seam — the runtime reads through
/// [`parse_persisted`].)
#[cfg(test)]
fn parse(text: &str) -> Option<L10n> {
    match parse_persisted(text) {
        Persisted::Language(lang) => Some(lang),
        Persisted::System => None,
    }
}

/// First-run detection from the process environment (`LC_ALL` /
/// `LC_MESSAGES` / `LANG`, e.g. `de_DE.UTF-8` → `de`); English when no
/// known locale is advertised. Unix-only: other platforms default to
/// English until a native locale API is worth a dependency.
fn detect_system_locale() -> L10n {
    detect_from_envs(|key| std::env::var(key).ok())
}

/// POSIX precedence (`LC_ALL` > `LC_MESSAGES` > `LANG`): the FIRST
/// set, non-empty variable decides the locale outright (an empty
/// value counts as unset). An unsupported value there (e.g.
/// `LC_ALL=C`) means English — the lower-priority variables are never
/// consulted (PR #269 review: `LC_ALL=C` alongside
/// `LANG=fa_IR.UTF-8` must stay English, not fall through to the
/// Persian `LANG`).
fn detect_from_envs(lookup: impl Fn(&str) -> Option<String>) -> L10n {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(value) = lookup(key).filter(|v| !v.is_empty()) {
            return parse_locale(&value).unwrap_or(L10n::En);
        }
    }
    L10n::En
}

/// `de_DE.UTF-8` → German; unknown or `C` locales → `None`.
/// `zh_CN`/`zh_SG`/bare `zh` → the `zh-Hans` catalog (#282): CLDR
/// reads bare `zh` as Simplified, and the POSIX territory carries the
/// script split until the territory string itself does (`zh_Hans_*`
/// also lands here). `zh_TW`/`zh_HK`/`zh_Hant*` stay `None` — no
/// Traditional catalog ships, and guessing Simplified for them would
/// be wrong, not fallback-worthy.
fn parse_locale(value: &str) -> Option<L10n> {
    let code = value.split('.').next().unwrap_or("");
    let code = code.split('@').next().unwrap_or("");
    let code = code.to_ascii_lowercase();
    // Bare `zh` reads as Simplified (CLDR default).
    if code == "zh" {
        return L10n::from_str("zh-Hans").ok();
    }
    if let Some(rest) = code.strip_prefix("zh_").or_else(|| code.strip_prefix("zh-")) {
        // The script subtag decides when present; the POSIX
        // territories map by convention (CN/SG simplified,
        // TW/HK/MO traditional).
        let script_or_terr = rest.split(['_', '-']).next().unwrap_or("");
        return match script_or_terr {
            "hans" | "cn" | "sg" | "" => L10n::from_str("zh-Hans").ok(),
            _ => None, // hant / tw / hk / mo / unknown variants
        };
    }
    let bare = code.split(['_', '-']).next().unwrap_or("");
    L10n::from_str(bare).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_precedence_stops_at_the_first_set_variable() {
        // Nothing set → English.
        assert_eq!(detect_from_envs(|_| None), L10n::En);
        // LANG alone decides.
        assert_eq!(
            detect_from_envs(|k| (k == "LANG").then(|| "fa_IR.UTF-8".to_string())),
            L10n::Fa
        );
        // LC_ALL=C OVERRIDES a supported LANG — English, never the
        // fall-through scan (PR #269 regression).
        assert_eq!(
            detect_from_envs(|k| match k {
                "LC_ALL" => Some("C".to_string()),
                "LANG" => Some("fa_IR.UTF-8".to_string()),
                _ => None,
            }),
            L10n::En
        );
        // An empty value counts as unset (POSIX) — LANG decides.
        assert_eq!(
            detect_from_envs(|k| match k {
                "LC_ALL" => Some(String::new()),
                "LANG" => Some("de_DE.UTF-8".to_string()),
                _ => None,
            }),
            L10n::De
        );
    }

    #[test]
    fn roundtrips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("language.json");
        persist_to(&path, L10n::De);
        assert_eq!(parse_from(&path), Some(L10n::De));
    }

    #[test]
    fn missing_file_parses_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(parse_from(&tmp.path().join("absent.json")), None);
    }

    #[test]
    fn corrupt_file_parses_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("language.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(parse_from(&path), None);
    }

    #[test]
    fn future_schema_version_parses_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("language.json");
        std::fs::write(&path, r#"{"version":99,"language":"de"}"#).unwrap();
        assert_eq!(parse_from(&path), None);
    }

    #[test]
    fn unknown_language_code_parses_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("language.json");
        std::fs::write(&path, r#"{"version":1,"language":"zz"}"#).unwrap();
        assert_eq!(parse_from(&path), None);
    }

    /// #268: the keyless form (absent or null language) is the Options
    /// Language page's "Match system locale" choice; everything that
    /// is not version-1-with-a-valid-language reads as System — the
    /// pre-#268 first-run behavior.
    #[test]
    fn keyless_form_parses_as_system_match() {
        assert!(matches!(parse_persisted(r#"{"version":1}"#), Persisted::System));
        assert!(matches!(
            parse_persisted(r#"{"version":1,"language":null}"#),
            Persisted::System
        ));
        assert!(matches!(
            parse_persisted(r#"{"version":1,"language":"de"}"#),
            Persisted::Language(L10n::De)
        ));
        // An invalid code or an unknown version keeps first-run behavior.
        assert!(matches!(
            parse_persisted(r#"{"version":1,"language":"zz"}"#),
            Persisted::System
        ));
        assert!(matches!(
            parse_persisted(r#"{"version":99,"language":"de"}"#),
            Persisted::System
        ));
        assert!(matches!(parse_persisted("{not json"), Persisted::System));
        // The explicit-language reader keeps its None for those.
        assert_eq!(parse(r#"{"version":1}"#), None);
    }

    /// #268: the system-match persist writes the KEYLESS form, which
    /// reads back as System.
    #[test]
    fn persist_system_writes_the_keyless_form() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("language.json");
        persist_system_to(&path);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(matches!(parse_persisted(&text), Persisted::System));
        // And an explicit-language file alongside it still reads as
        // the language.
        persist_to(&path, L10n::De);
        assert!(matches!(
            parse_persisted(&std::fs::read_to_string(&path).unwrap()),
            Persisted::Language(L10n::De)
        ));
    }

    /// #280: Fluent wraps every placeable in bidi-isolation marks
    /// (`use_isolating`, on build-wide since #257). Inside egui they
    /// shape away; on a NATIVE surface (the OS window title) the WM's
    /// font paints them as tofu. The boundary strips exactly
    /// U+2066–U+2069 and nothing else, for every shipped catalog.
    #[test]
    fn native_surface_strips_bidi_isolation_marks() {
        let isolates = |s: &str| {
            s.chars().any(|c| ('\u{2066}'..='\u{2069}').contains(&c))
        };
        // The source really carries the marks — the fresh-document
        // name is the witnessed tofu case.
        let drawing1 = strs_for(L10n::En).msg_drawing_n(1);
        assert!(
            isolates(&drawing1),
            "Fluent isolates the placeable: {drawing1:?}"
        );
        // The boundary strips them and keeps every other character.
        assert_eq!(to_native_surface(&drawing1), "Drawing1");
        // A full title composes clean in EVERY shipped catalog (fa's
        // whole-chrome isolation is the sharpest case; es, zh-Hans,
        // and the ja/ko wave join the set with #282).
        for lang in [
            L10n::En,
            L10n::De,
            L10n::Fa,
            L10n::Es,
            L10n::ZhHans,
            L10n::Ja,
            L10n::Ko,
        ] {
            let name = strs_for(lang).msg_drawing_n(1);
            let title = to_native_surface(&format!("{name} - Tarahi"));
            assert!(
                !isolates(&title),
                "{lang:?} title carries isolation marks: {title:?}"
            );
            assert!(title.ends_with(" - Tarahi"));
        }
        // Non-isolation characters pass through untouched — including
        // the OTHER bidi controls (LRM/RLM), which Fluent does not
        // emit and which have their own jobs.
        let mixed = "a\u{200e}b\u{200f}c".to_string();
        assert_eq!(to_native_surface(&mixed), mixed);
    }

    #[test]
    fn system_locale_detection_reads_lang() {
        assert_eq!(parse_locale("de_DE.UTF-8"), Some(L10n::De));
        assert_eq!(parse_locale("en"), Some(L10n::En));
        assert_eq!(parse_locale("C"), None);
        assert_eq!(parse_locale("fr_FR.UTF-8"), None);
        // #282: Chinese resolves to the Simplified catalog — bare zh
        // (CLDR default), the simplified territories, and the script
        // subtag itself; the traditional variants carry no catalog
        // and must NOT fall back to Simplified.
        assert_eq!(parse_locale("zh"), Some(L10n::ZhHans));
        assert_eq!(parse_locale("zh_CN.UTF-8"), Some(L10n::ZhHans));
        assert_eq!(parse_locale("zh_SG.UTF-8"), Some(L10n::ZhHans));
        assert_eq!(parse_locale("zh_Hans_CN.UTF-8"), Some(L10n::ZhHans));
        assert_eq!(parse_locale("zh_CN@pinyin"), Some(L10n::ZhHans));
        assert_eq!(parse_locale("zh_TW.UTF-8"), None);
        assert_eq!(parse_locale("zh_HK.UTF-8"), None);
        assert_eq!(parse_locale("zh_Hant_TW.UTF-8"), None);
        // #282 ja/ko: single-script languages — the code before the
        // `_`/`.` split is the whole story, so the POSIX territories
        // resolve without a mapping table (unlike zh's script split).
        assert_eq!(parse_locale("ja"), Some(L10n::Ja));
        assert_eq!(parse_locale("ja_JP.UTF-8"), Some(L10n::Ja));
        assert_eq!(parse_locale("ja_JP.eucJP"), Some(L10n::Ja));
        assert_eq!(parse_locale("ko"), Some(L10n::Ko));
        assert_eq!(parse_locale("ko_KR.UTF-8"), Some(L10n::Ko));
        assert_eq!(parse_locale("ko_KR.eucKR"), Some(L10n::Ko));
    }

    // Test seam: persistence against an explicit path (the real
    // persist/parse use config_base(); the logic is shared).
    fn persist_to(path: &std::path::Path, lang: L10n) {
        let _ = std::fs::write(
            path,
            serde_json::json!({ "version": SCHEMA_VERSION, "language": lang.to_string() })
                .to_string(),
        );
    }

    fn persist_system_to(path: &std::path::Path) {
        let _ = std::fs::write(path, serde_json::json!({ "version": SCHEMA_VERSION }).to_string());
    }

    fn parse_from(path: &std::path::Path) -> Option<L10n> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| super::parse(&text))
    }
}
