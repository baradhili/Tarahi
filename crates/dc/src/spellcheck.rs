//! Spell checking over shape text (GitHub #121) — [`spellbook`]-backed.
//!
//! Three layers live here:
//!
//! 1. **Dictionaries** ([`SpellDictionary`], [`available`], [`get`]) —
//!    Hunspell `.aff`/`.dic` pairs compiled through `spellbook` (pure Rust,
//!    no libhunspell FFI). Three pairs are embedded from
//!    `assets/dictionaries/` (license provenance beside the files, the
//!    fonts pattern): `en_US`, `de_DE_frami`, `fa-IR`. At first use the
//!    registry also scans the standard system Hunspell locations (plus
//!    app-registered directories) for local pairs and **prefers the local
//!    dictionary over the embedded one for the same stem** — the stencil
//!    built-ins' disk-first rule mirrored. Legacy-encoded local
//!    dictionaries transcode at load time from their declared aff `SET`
//!    encoding (`spellbook` requires UTF-8 input). Headless ctors pin the
//!    embedded fallback via [`pin_embedded`] so kittest never reads the
//!    user's machine.
//! 2. **Tokenizer** ([`words`]) — walks the shape's raw `<Text>` through
//!    `vsdx::text::text_pieces`, tokenizing **literal runs only**: field
//!    (`<fld>`) runs are skipped — a spell change to a computed value is
//!    meaningless — while their cached display still OCCUPIES plain-text
//!    space, so every token's offsets are positions in the full plain
//!    projection that [`vsdx::text::plain_to_raw_offset`] maps back onto
//!    the raw (the write path). Word rules: alphabetic runs (plus Arabic
//!    diacritics) joined by apostrophes, hyphens, and the Persian ZWNJ
//!    (U+200C — the fa-IR dictionary's TRY carries it); whitespace runs
//!    that look like URLs, e-mails, or `=`-formulas never check.
//! 3. **Checking** ([`Checker`]) — dictionary check plus the session-level
//!    word lists (custom dictionary, Ignore All), so a dialog walk has one
//!    decision function.
//!
//! [`spellbook`]: https://crates.io/crates/spellbook

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use vsdx::text::{self, Piece};

// ---------------------------------------------------------------------------
// Dictionaries
// ---------------------------------------------------------------------------

/// One available dictionary: its stem (the `.aff`/`.dic` basename) and,
/// when discovered on disk rather than embedded, where the pair lives.
#[derive(Debug, Clone, PartialEq)]
pub struct LangEntry {
    pub stem: String,
    /// `Some((aff_path, dic_path))` for a locally discovered dictionary.
    pub local: Option<(PathBuf, PathBuf)>,
}

impl LangEntry {
    /// Native display name of the dictionary's language — dictionary
    /// vocabulary, never translated (the master-NameU rule). The stem's
    /// primary subtag picks the name; unknown subtags show the stem.
    pub fn display_name(&self) -> String {
        display_name(&self.stem)
    }
}

/// Native names for the language subtags a Hunspell dictionary stem can
/// carry (LibreOffice's set plus the common extras).
fn display_name(stem: &str) -> String {
    let mut subtags = stem.split(['_', '-']);
    let primary = subtags
        .next()
        .unwrap_or(stem)
        .to_ascii_lowercase();
    // The remaining subtags distinguish variants that share a language:
    // `en_GB` and `en_US` both read "English", so the qualifier rides
    // along — `English (GB)` vs `English (US)`, `de_DE_frami` reads
    // `Deutsch (DE, frami)` (PR #299 review: the dropdown's entries AND
    // its selected value both route through here).
    let qualifier = subtags.filter(|p| !p.is_empty()).collect::<Vec<_>>();
    let name = match primary.as_str() {
        "af" => "Afrikaans",
        "an" => "Aragonés",
        "ar" => "العربية",
        "as" => "অসমীয়া",
        "be" => "Беларуская",
        "bg" => "Български",
        "bn" => "বাংলা",
        "bo" => "བོད་ཡིག",
        "br" => "Brezhoneg",
        "bs" => "Bosanski",
        "ca" => "Català",
        "ckb" => "کوردی",
        "cs" => "Čeština",
        "da" => "Dansk",
        "de" => "Deutsch",
        "el" => "Ελληνικά",
        "en" => "English",
        "eo" => "Esperanto",
        "es" => "Español",
        "et" => "Eesti",
        "eu" => "Euskara",
        "fa" => "فارسی",
        "fr" => "Français",
        "ga" => "Gaeilge",
        "gd" => "Gàidhlig",
        "gl" => "Galego",
        "gu" => "ગુજરાતી",
        "he" => "עברית",
        "hi" => "हिन्दी",
        "hr" => "Hrvatski",
        "hu" => "Magyar",
        "id" => "Bahasa Indonesia",
        "is" => "Íslenska",
        "it" => "Italiano",
        "ja" => "日本語",
        "kn" => "ಕನ್ನಡ",
        "ko" => "한국어",
        "lo" => "ລາວ",
        "lt" => "Lietuvių",
        "lv" => "Latviešu",
        "mn" => "Монгол",
        "mr" => "मराठी",
        "ne" => "नेपाली",
        "nl" => "Nederlands",
        "no" => "Norsk",
        "oc" => "Occitan",
        "or" => "ଓଡ଼ିଆ",
        "pa" => "ਪੰਜਾਬੀ",
        "pl" => "Polski",
        "pt" => "Português",
        "ro" => "Română",
        "ru" => "Русский",
        "sa" => "संस्कृतम्",
        "si" => "සිංහල",
        "sk" => "Slovenčina",
        "sl" => "Slovenščina",
        "sq" => "Shqip",
        "sr" => "Српски",
        "sv" => "Svenska",
        "sw" => "Kiswahili",
        "ta" => "தமிழ்",
        "te" => "తెలుగు",
        "th" => "ไทย",
        "tr" => "Türkçe",
        "uk" => "Українська",
        "ur" => "اردو",
        "vi" => "Tiếng Việt",
        "zh" => "中文",
        _ => return stem.to_string(),
    };
    if qualifier.is_empty() {
        name.to_string()
    } else {
        format!("{name} ({})", qualifier.join(", "))
    }
}

/// The three embedded dictionaries — the deterministic floor every
/// process (headless ctors included) can load without touching disk.
const EMBEDDED: &[(&str, &str, &str)] = &[
    (
        "en_US",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/en_US/en_US.aff"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/en_US/en_US.dic"
        )),
    ),
    (
        "de_DE_frami",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/de_DE/de_DE_frami.aff"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/de_DE/de_DE_frami.dic"
        )),
    ),
    (
        "fa-IR",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/fa_IR/fa-IR.aff"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/dictionaries/fa_IR/fa-IR.dic"
        )),
    ),
];

/// A compiled Hunspell dictionary (`spellbook::Dictionary`).
pub struct SpellDictionary {
    stem: String,
    inner: spellbook::Dictionary,
}

impl SpellDictionary {
    /// Compile an `.aff`/`.dic` pair (both UTF-8 — transcode first, the
    /// way [`decode_pair`] does for local dictionaries).
    pub fn compile(stem: &str, aff: &str, dic: &str) -> Result<Self, String> {
        let inner = spellbook::Dictionary::new(aff, dic)
            .map_err(|e| format!("dictionary {stem:?} failed to compile: {e}"))?;
        Ok(SpellDictionary {
            stem: stem.to_string(),
            inner,
        })
    }

    pub fn stem(&self) -> &str {
        &self.stem
    }

    /// Is `word` in the dictionary (hunspell case/affix semantics)?
    pub fn check(&self, word: &str) -> bool {
        if self.inner.check(word) {
            return true;
        }
        // Decomposed input (`grü` + combining diaeresis + `n`) never
        // matches the dictionary's composed entry — try the NFC form
        // before ruling the word misspelled (PR #299 review).
        use unicode_normalization::UnicodeNormalization;
        let composed: String = word.nfc().collect();
        composed != word && self.inner.check(&composed)
    }

    /// Ranked spelling suggestions (Nuspell-modeled; no phonetic
    /// strategy — spellbook parity level).
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.inner.suggest(word, &mut out);
        out
    }
}

struct Registry {
    /// Compiled dictionaries keyed by stem (embedded and local alike).
    loaded: HashMap<String, Arc<SpellDictionary>>,
    /// Local `<stem>.aff`/`<stem>.dic` pairs found in the scan
    /// directories; `None` until the first [`available`]/[`get`] scan.
    local: Option<Vec<(String, PathBuf, PathBuf)>>,
    /// Extra scan directories the app layer registers (the user's
    /// app-config `dictionaries/` drop-point).
    extra_dirs: Vec<PathBuf>,
    /// Test hook: replace the whole scan list (system dirs included)
    /// with exactly these, so a fixture test is machine-independent.
    override_dirs: Option<Vec<PathBuf>>,
    /// Headless ctors pin the embedded fallback — no user-machine reads.
    local_enabled: bool,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            loaded: HashMap::new(),
            local: None,
            extra_dirs: Vec::new(),
            override_dirs: None,
            local_enabled: true,
        })
    })
}

/// Headless ctor pin: local dictionary discovery stays OFF for the rest
/// of the process, so kittest never reads the user's installed
/// dictionaries (the stencil built-ins' pinning rule).
pub fn pin_embedded() {
    let mut r = registry().lock().unwrap();
    r.local_enabled = false;
    // A policy change must not keep dictionaries compiled under the
    // previous policy — `get` recompiles from the effective source
    // (CodeRabbit round).
    r.loaded.clear();
}

/// Re-enable local discovery after a [`pin_embedded`] — the production
/// ctor's path (it boots through the headless ctor, which pins, then
/// adopts the user-machine state).
pub fn enable_local_discovery() {
    let mut r = registry().lock().unwrap();
    r.local_enabled = true;
    r.local = None; // rescan on next use
    r.loaded.clear(); // same rule as pin_embedded
}

/// Register extra local-scan directories (the app layer's config dir and
/// any platform-specific locations). Best-effort: unknown stems simply
/// never appear.
pub fn register_local_dirs(dirs: Vec<PathBuf>) {
    registry().lock().unwrap().extra_dirs.extend(dirs);
}

/// Test hook: replace the local-scan directory list wholesale — system
/// directories included, so a fixture test is machine-independent even
/// where hunspell packages are installed. Production wiring uses
/// [`register_local_dirs`].
pub fn set_local_dirs_for_test(dirs: Vec<PathBuf>) {
    let mut r = registry().lock().unwrap();
    r.override_dirs = Some(dirs);
    r.local = None; // rescan on next use
    r.loaded.clear(); // policy change — recompile from the new sources
}

/// Every dictionary the process can check against: the embedded three,
/// plus locally discovered pairs (a local dictionary replaces its
/// embedded twin — disk-first, the stencil rule).
pub fn available() -> Vec<LangEntry> {
    let mut r = registry().lock().unwrap();
    scan_local_if_needed(&mut r);
    let mut out: Vec<LangEntry> = EMBEDDED
        .iter()
        .map(|(stem, _, _)| LangEntry {
            stem: (*stem).to_string(),
            local: None,
        })
        .collect();
    if r.local_enabled {
        for (stem, aff, dic) in r.local.as_deref().unwrap_or(&[]) {
            // A local pair shadows the embedded twin for the same stem.
            if let Some(e) = out.iter_mut().find(|e| &e.stem == stem) {
                e.local = Some((aff.clone(), dic.clone()));
            } else {
                out.push(LangEntry {
                    stem: stem.clone(),
                    local: Some((aff.clone(), dic.clone())),
                });
            }
        }
    }
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    out
}

/// Did the dictionary for `stem` actually load? The UI distinguishes a
/// clean page from a failed dictionary (a discovered pair that fails to
/// load must not read as "Spelling check complete" — CodeRabbit round).
pub fn loaded(stem: &str) -> bool {
    registry().lock().unwrap().loaded.contains_key(stem)
}

/// Get (compiling on first use) the dictionary for `stem` — the local
/// pair when one was discovered, else the embedded bytes.
pub fn get(stem: &str) -> Option<Arc<SpellDictionary>> {
    let mut r = registry().lock().unwrap();
    if let Some(d) = r.loaded.get(stem) {
        return Some(d.clone());
    }
    scan_local_if_needed(&mut r);
    let local = if r.local_enabled { r.local.clone() } else { None };
    let local = local
        .unwrap_or_default()
        .into_iter()
        .find(|(s, _, _)| s == stem)
        .map(|(_, a, d)| (a, d));
    let embedded = || {
        EMBEDDED.iter().find(|(s, _, _)| *s == stem).and_then(
            |(s, aff, dic)| match SpellDictionary::compile(s, aff, dic) {
                Ok(d) => Some(d),
                Err(e) => {
                    eprintln!("dc::spellcheck: {e}");
                    None
                }
            },
        )
    };
    // Prefer the local pair, but fall back to the embedded twin when it
    // fails to load (CodeRabbit round: a discovered pair can become
    // unreadable between the scan and the load — the desktop then keeps
    // its vocabulary instead of silently checking nothing).
    let compiled = match local {
        Some((aff_path, dic_path)) => load_local(stem, &aff_path, &dic_path).or_else(embedded),
        None => embedded(),
    }?;
    let arc = Arc::new(compiled);
    r.loaded.insert(stem.to_string(), arc.clone());
    Some(arc)
}

fn scan_local_if_needed(r: &mut Registry) {
    if r.local.is_some() || !r.local_enabled {
        return;
    }
    let dirs: Vec<PathBuf> = match &r.override_dirs {
        Some(o) => o.clone(),
        None => {
            let mut d = system_dict_dirs();
            d.extend(r.extra_dirs.iter().cloned());
            d
        }
    };
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut dics: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "dic"))
            .collect();
        dics.sort();
        for dic in dics {
            let Some(stem) = dic.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let aff = dic.with_extension("aff");
            if !aff.is_file() {
                continue;
            }
            // First directory wins for duplicate stems (system order is
            // the priority order).
            if !found.iter().any(|(s, _, _): &(String, PathBuf, PathBuf)| s == stem) {
                found.push((stem.to_string(), aff, dic));
            }
        }
    }
    r.local = Some(found);
}

/// The standard Hunspell dictionary locations per platform (Linux's
/// myspell paths are what Debian/Ubuntu's hunspell packages populate).
fn system_dict_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        vec![
            PathBuf::from("/usr/share/hunspell"),
            PathBuf::from("/usr/share/myspell/dicts"),
            PathBuf::from("/usr/share/myspell"),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![PathBuf::from("/Library/Spelling")]
    }
    #[cfg(windows)]
    {
        // LibreOffice's per-locale extension dictionaries (the common
        // Hunspell source on Windows) live DIRECTLY under the install's
        // `share\extensions` as `dict-en`, `dict-de`, … folders — the
        // old scan looked for a `dict` subfolder real installs never
        // have (PR #299 review). Firefox-style install `dictionaries`
        // folders arrive through `register_local_dirs` from the app
        // layer. Both ProgramFiles hops carry installs.
        let mut out = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(pf) = std::env::var(var) {
                let ext = PathBuf::from(&pf)
                    .join("LibreOffice")
                    .join("share")
                    .join("extensions");
                if let Ok(entries) = std::fs::read_dir(&ext) {
                    out.extend(
                        entries
                            .flatten()
                            .map(|x| x.path())
                            .filter(|p| p.is_dir()),
                    );
                }
            }
        }
        out
    }
}

/// Load a local `.aff`/`.dic` pair, transcoding legacy encodings through
/// the aff's declared `SET` (`spellbook` needs UTF-8). Unreadable or
/// non-compilable pairs return `None` with a note on stderr.
fn load_local(stem: &str, aff_path: &Path, dic_path: &Path) -> Option<SpellDictionary> {
    let aff = std::fs::read(aff_path).ok()?;
    let dic = std::fs::read(dic_path).ok()?;
    let (aff_s, dic_s) = match decode_pair(&aff, &dic) {
        Some(pair) => pair,
        None => {
            eprintln!("dc::spellcheck: dictionary {stem:?} is not decodable (bad SET?)");
            return None;
        }
    };
    match SpellDictionary::compile(stem, &aff_s, &dic_s) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("dc::spellcheck: {e}");
            None
        }
    }
}

/// Decode an `.aff`/`.dic` byte pair into UTF-8 strings. The hunspell
/// contract: the aff's `SET <encoding>` names the codec for BOTH files
/// (the .dic carries no SET of its own) and applies even when the bytes
/// happen to also form valid UTF-8 — a SET ISO8859-1 dictionary with
/// bytes `C3 A9` means `Ã©`, not `é`. An unsupported or ambiguous SET
/// label REJECTS the pair rather than silently re-encoding its
/// vocabulary (CodeRabbit round, citing the hunspell man page); without
/// a SET the UTF-8 attempt runs first, falling back to the hunspell
/// default codec (windows-1252, the ISO-8859-1 superset encoding_rs
/// maps the default to).
fn decode_pair(aff: &[u8], dic: &[u8]) -> Option<(String, String)> {
    let declared = || -> Option<Option<&'static encoding_rs::Encoding>> {
        for line in aff.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line);
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("SET") {
                let label = rest.trim_start();
                if label.is_empty() {
                    continue;
                }
                // An unknown label must not silently pick a codec — the
                // decoded vocabulary would be wrong in a way nothing
                // downstream can detect. Outer `None` = reject the pair.
                return match encoding_rs::Encoding::for_label(label.as_bytes()) {
                    Some(enc) => Some(Some(enc)),
                    None => None,
                };
            }
        }
        Some(None) // no SET declaration
    }()?;
    match declared {
        Some(enc) => {
            let (a, _, _) = enc.decode(aff);
            let (d, _, _) = enc.decode(dic);
            Some((a.into_owned(), d.into_owned()))
        }
        None => {
            let aff_s = match std::str::from_utf8(aff) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    let (s, _, _) = encoding_rs::WINDOWS_1252.decode(aff);
                    s.into_owned()
                }
            };
            let dic_s = match std::str::from_utf8(dic) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    let (s, _, _) = encoding_rs::WINDOWS_1252.decode(dic);
                    s.into_owned()
                }
            };
            Some((aff_s, dic_s))
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

/// One checkable word in a shape's text: its plain-space span (CHARACTER
/// offsets over the full plain projection — field displays included —
/// the space [`vsdx::text::plain_to_raw_offset`] maps back onto the raw)
/// and the word itself.
#[derive(Debug, Clone, PartialEq)]
pub struct WordToken {
    pub start: usize,
    pub end: usize,
    pub word: String,
}

/// In-word joiners: ASCII + typographic apostrophes, ASCII + non-breaking
/// hyphen, and the zero-width joiners Persian compounds ride on
/// (the fa-IR dictionary's TRY carries ZWNJ).
fn is_joiner(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '-' | '\u{2011}' | '\u{200C}' | '\u{200D}')
}

/// Combining MARKS that ride inside a word — not
/// `char::is_alphabetic()`, but part of the word when they follow
/// letters. The general Mark category covers the Arabic harakat the
/// fa-IR dictionary needs AND the decomposed Latin accents pasted
/// text carries (`grün` NFD splits into `gru`|`n` without them —
/// PR #299 review); the lookup then tries the word's NFC form so the
/// composed dictionary entry matches.
fn is_word_mark(c: char) -> bool {
    use unicode_general_category::{get_general_category, GeneralCategory};
    matches!(
        get_general_category(c),
        GeneralCategory::SpacingMark | GeneralCategory::NonspacingMark | GeneralCategory::ModifierLetter
    )
}

/// Every checkable word in the raw `<Text>`, in reading order. Field
/// runs contribute no tokens but DO occupy plain space, so the offsets
/// stay aligned with [`vsdx::text::plain_to_raw_offset`].
pub fn words(raw: &str) -> Vec<WordToken> {
    let pieces = text::text_pieces(raw);
    let full_plain: String = pieces.iter().map(|p| p.plain()).collect();
    let mut out = Vec::new();
    let mut base = 0usize; // plain chars before this piece
    for piece in &pieces {
        match piece {
            Piece::Text(t) => {
                tokenize_piece(t, base, &mut out);
                base += t.chars().count();
            }
            _ => base += piece.plain().chars().count(),
        }
    }
    // Whole-context filters: the whitespace run each token sits in must
    // not be a URL, an e-mail, a formula, or touch a digit (Visio never
    // checks those). The char index and the run verdicts build ONCE for
    // the whole text — per-token rescans made a large block quadratic
    // (CodeRabbit round).
    let chars: Vec<char> = full_plain.chars().collect();
    let runs = checkable_runs(&chars);
    out.retain(|tok| checkable_context(&chars, &runs, tok));
    out
}

fn tokenize_piece(t: &str, base: usize, out: &mut Vec<WordToken>) {
    let mut chars = t.chars().enumerate().peekable();
    while let Some((ci, c)) = chars.next() {
        if !(c.is_alphabetic() || is_word_mark(c)) {
            continue;
        }
        // Walk the run: word letters/marks, with joiners only when the
        // next char continues the word (no leading/trailing joiners).
        let mut word = String::new();
        let mut nchars = 0usize;
        let mut cur = Some((ci, c));
        while let Some((_, ch)) = cur {
            if ch.is_alphabetic() || is_word_mark(ch) {
                word.push(ch);
                nchars += 1;
            } else if is_joiner(ch) {
                match chars.peek() {
                    Some((_, nx)) if nx.is_alphabetic() || is_word_mark(*nx) => {
                        word.push(ch);
                        nchars += 1;
                    }
                    _ => break, // trailing joiner: the word ends before it
                }
            } else {
                break;
            }
            cur = chars.next();
        }
        out.push(WordToken {
            start: base + ci,
            end: base + ci + nchars,
            word,
        });
    }
}

/// One whitespace-delimited run of the plain text: its char span and
/// whether tokens inside it are checkable at all (URLs, e-mails,
/// formulas, hostnames never are).
type TextRun = (usize, usize, bool);

/// Classify every whitespace-delimited run once — O(n) over the text,
/// shared by all tokens (CodeRabbit round: the per-token rescan made a
/// large block quadratic).
fn checkable_runs(chars: &[char]) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        let ok = !run.contains("://")
            && !run.contains('@')
            && !run.starts_with('=')
            && !run.to_ascii_lowercase().starts_with("www.");
        runs.push((start, i, ok));
    }
    runs
}

/// The plain-space context filters for one token: digit-adjacent tokens
/// never check, and the token's run must be a checkable one.
fn checkable_context(chars: &[char], runs: &[TextRun], tok: &WordToken) -> bool {
    // A letter run that touches a digit ("v2", "A4") is part of a label,
    // not prose.
    if tok.start > 0 && chars[tok.start - 1].is_numeric() {
        return false;
    }
    if tok.end < chars.len() && chars[tok.end].is_numeric() {
        return false;
    }
    // The token's enclosing run (runs are disjoint and ordered — binary
    // search by run start).
    let idx = runs
        .binary_search_by(|(rs, _, _)| rs.cmp(&tok.start))
        .unwrap_or_else(|insert| insert.saturating_sub(1));
    match runs.get(idx) {
        Some((rs, re, ok)) => tok.start >= *rs && tok.end <= *re && *ok,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Checking (dictionary + session word lists)
// ---------------------------------------------------------------------------

/// The decision function a spell session walks with: the dictionary plus
/// the session's accepted words (custom dictionary and Ignore All).
pub struct Checker {
    dict: Arc<SpellDictionary>,
    custom: Vec<String>,
    ignore: Vec<String>,
}

impl Checker {
    pub fn new(dict: Arc<SpellDictionary>) -> Self {
        Checker {
            dict,
            custom: Vec::new(),
            ignore: Vec::new(),
        }
    }

    pub fn dictionary(&self) -> &SpellDictionary {
        &self.dict
    }

    pub fn add_custom(&mut self, word: impl Into<String>) {
        let word = word.into();
        if !self.custom.contains(&word) {
            self.custom.push(word);
        }
    }

    pub fn custom_words(&self) -> &[String] {
        &self.custom
    }

    pub fn ignore_all(&mut self, word: impl Into<String>) {
        let word = word.into();
        if !self.ignore.contains(&word) {
            self.ignore.push(word);
        }
    }

    pub fn ignored(&self) -> &[String] {
        &self.ignore
    }

    /// Is `word` acceptable? Dictionary check (hunspell case semantics),
    /// then the custom dictionary and Ignore-All lists (exact match and
    /// case-folded match — the word lists store base case).
    pub fn check(&self, word: &str) -> bool {
        if self.dict.check(word) {
            return true;
        }
        let lower = word.to_lowercase();
        self.custom
            .iter()
            .any(|c| c == word || c.to_lowercase() == lower)
            || self
                .ignore
                .iter()
                .any(|c| c == word || c.to_lowercase() == lower)
    }

    /// Suggestions for `word` (dictionary-ranked).
    pub fn suggest(&self, word: &str) -> Vec<String> {
        self.dict.suggest(word)
    }

    /// The first misspelling at/after plain char `after_char`, if any.
    pub fn first_misspelling(&self, raw: &str, after_char: usize) -> Option<WordToken> {
        words(raw)
            .into_iter()
            .find(|t| t.start >= after_char && !self.check(&t.word))
    }

    /// Every misspelling in the raw text (Change-All's scan).
    pub fn misspellings(&self, raw: &str) -> Vec<WordToken> {
        words(raw).into_iter().filter(|t| !self.check(&t.word)).collect()
    }
}

// ---------------------------------------------------------------------------
// Shape-text helpers (the session walk)
// ---------------------------------------------------------------------------

/// The plain projection of a raw `<Text>` — the piece concatenation
/// (the contract `text_pieces` keeps byte-exact).
pub fn plain_of(raw: &str) -> String {
    text::text_pieces(raw)
        .iter()
        .map(|p| p.plain())
        .collect()
}

/// The raw text a shape effectively renders: local raw → escaped local
/// plain → the master's inherited raw (an instance with no local text
/// checks its master's words — and a Change materializes the instance's
/// copy, the InsertField convention). `None` when the shape has no
/// text anywhere in its chain.
pub fn effective_raw(
    doc: &vsdx::model::Document,
    page: i64,
    shape: i64,
) -> Option<String> {
    let escape = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let p = doc.pages.iter().find(|p| p.id == page)?;
    let sh = vsdx::model::find_shape_by_id(&p.shapes, shape)?;
    if let Some(raw) = &sh.sheet.text_raw {
        return Some(raw.clone());
    }
    if let Some(t) = &sh.sheet.text {
        return Some(escape(t));
    }
    let chain = dc_core::scope::shape_chain(doc, page, shape).ok()?;
    let ancestors: Vec<_> = chain.into_iter().filter(|c| c.id != shape).collect();
    let src = doc.instance_source(sh, &ancestors)?;
    src.sheet
        .text_raw
        .clone()
        .or_else(|| src.sheet.text.clone().map(|t| escape(&t)))
}

// ---------------------------------------------------------------------------
// Tests — positive and negative spelling coverage for en/de/fa (#121)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is process-global and this binary's tests run in
    /// parallel — every dictionary-touching test holds this lock and
    /// starts from the pinned-embedded state, so a test mutating the
    /// local-scan state (the discovery test) cannot race a reader.
    static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

    fn fresh_registry() -> std::sync::MutexGuard<'static, ()> {
        let g = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut r = registry().lock().unwrap();
            r.loaded.clear();
            r.local = None;
            r.extra_dirs.clear();
            r.override_dirs = None;
            r.local_enabled = false;
        }
        g
    }

    fn dict(stem: &str) -> Arc<SpellDictionary> {
        get(stem).expect("embedded dictionary compiles")
    }

    fn checker(stem: &str) -> Checker {
        Checker::new(dict(stem))
    }

    // -- dictionaries load and check (POSITIVE) --------------------------

    #[test]
    fn embedded_en_de_fa_all_compile() {
        let _g = fresh_registry();
        for stem in ["en_US", "de_DE_frami", "fa-IR"] {
            assert!(get(stem).is_some(), "{stem} compiles");
        }
        // available() content is asserted in the serialized local-scan
        // test (the registry is process-global; this binary's tests run
        // in parallel).
        let names: Vec<String> = available().into_iter().map(|e| e.display_name()).collect();
        // The stems' region/variant qualifiers ride along (PR #299
        // review): two dictionaries sharing a language must read
        // distinctly in the dropdown.
        assert!(names.contains(&"English (US)".to_string()));
        assert!(names.contains(&"Deutsch (DE, frami)".to_string()));
        assert!(names.contains(&"فارسی (IR)".to_string()));
    }

    #[test]
    fn en_positive_words_check() {
        let _g = fresh_registry();
        let c = checker("en_US");
        for w in ["diagram", "shape", "hello", "connector", "Diagram", "shapes"] {
            assert!(c.check(w), "en accepts {w:?}");
        }
    }

    #[test]
    fn de_positive_words_check() {
        let _g = fresh_registry();
        let c = checker("de_DE_frami");
        for w in ["Diagramm", "Form", "Diagramme"] {
            assert!(c.check(w), "de accepts {w:?}");
        }
    }

    #[test]
    fn de_accepts_decomposed_nfc_form() {
        // PR #299 review (Greptile): pasted text decomposes (NFD) —
        // the tokenizer keeps the marks inside the word and the check
        // tries the word's NFC form against the composed dictionary.
        let _g = fresh_registry();
        let c = checker("de_DE_frami");
        use unicode_normalization::UnicodeNormalization;
        let decomposed: String = "grün".nfd().collect();
        assert_ne!(decomposed, "grün", "the fixture really decomposes");
        let toks = words(&format!("{} Haus", decomposed));
        assert_eq!(toks.len(), 2, "one decomposed word + Haus, got {toks:?}");
        assert!(c.check(&toks[0].word), "de accepts the NFC lookup of {toks:?}");
    }

    #[test]
    fn fa_positive_words_check() {
        let _g = fresh_registry();
        let c = checker("fa-IR");
        for w in ["متن", "شکل", "نمودار", "می‌شود"] {
            assert!(c.check(w), "fa accepts {w:?}");
        }
    }

    // -- negatives -------------------------------------------------------

    #[test]
    fn en_negative_typos_flag() {
        let _g = fresh_registry();
        let c = checker("en_US");
        for w in ["diagrma", "shpae", "connectr", "xyzzyqq"] {
            assert!(!c.check(w), "en flags {w:?}");
        }
    }

    #[test]
    fn de_negative_typos_flag() {
        let _g = fresh_registry();
        let c = checker("de_DE_frami");
        for w in ["Digramm", "Fomr", "xyzzyqq"] {
            assert!(!c.check(w), "de flags {w:?}");
        }
    }

    #[test]
    fn fa_negative_typos_flag() {
        let _g = fresh_registry();
        let c = checker("fa-IR");
        // Non-words that no Persian affix could produce (doubled letters
        // mid-word, trailing letters outside the suffix inventory).
        for w in ["نمودااار", "متنتن", "شکلپپ"] {
            assert!(!c.check(w), "fa flags {w:?}");
        }
    }

    // -- suggestions -----------------------------------------------------

    #[test]
    fn en_suggestions_rank_shape_for_shpae() {
        let _g = fresh_registry();
        let c = checker("en_US");
        let s = c.suggest("shpae");
        assert!(s.iter().any(|w| w == "shape"), "shape among {s:?}");
        let s = c.suggest("diagrma");
        assert!(s.iter().any(|w| w == "diagram"), "diagram among {s:?}");
    }

    #[test]
    fn de_suggestions_rank_diagramm_for_digramm() {
        let _g = fresh_registry();
        let c = checker("de_DE_frami");
        let s = c.suggest("Digramm");
        assert!(s.iter().any(|w| w == "Diagramm"), "Diagramm among {s:?}");
    }

    #[test]
    fn fa_suggestions_return_candidates() {
        let _g = fresh_registry();
        let c = checker("fa-IR");
        let s = c.suggest("متون");
        assert!(!s.is_empty(), "fa suggests for {s:?}");
    }

    // -- custom dictionary + Ignore All ----------------------------------

    #[test]
    fn custom_and_ignore_lists_accept_words() {
        let _g = fresh_registry();
        let mut c = checker("en_US");
        assert!(!c.check("zorkmid"));
        c.add_custom("Zorkmid");
        assert!(c.check("zorkmid"), "custom list is case-folded");
        assert!(c.check("Zorkmid"));
        c.ignore_all("Gorblat");
        assert!(c.check("gorblat"), "Ignore All is case-folded");
        // Adds are deduplicated.
        c.add_custom("Zorkmid");
        assert_eq!(c.custom_words().len(), 1);
    }

    // -- tokenizer -------------------------------------------------------

    #[test]
    fn tokenizer_plain_words_with_offsets() {
        let toks = words("Draw a shape here");
        let got: Vec<&str> = toks.iter().map(|t| t.word.as_str()).collect();
        assert_eq!(got, vec!["Draw", "a", "shape", "here"]);
        // Offsets are plain char positions.
        let shape = &toks[2];
        assert_eq!(shape.start, 7);
        assert_eq!(shape.end, 12);
        assert_eq!(&"Draw a shape here"[shape.start..shape.end], "shape");
    }

    #[test]
    fn tokenizer_honors_apostrophes_hyphens_and_zwnj() {
        let got: Vec<String> = words("don't stop — a right-angle joint — می‌شود").into_iter().map(|t| t.word).collect();
        assert!(got.contains(&"don't".to_string()), "{got:?}");
        assert!(got.contains(&"right-angle".to_string()), "{got:?}");
        assert!(got.contains(&"می‌شود".to_string()), "{got:?}");
        // Leading/trailing joiners never join ("'tis" checks "tis",
        // "word-" checks "word").
        let got: Vec<String> = words("'tis done-").into_iter().map(|t| t.word).collect();
        assert_eq!(got, vec!["tis", "done"]);
    }

    #[test]
    fn tokenizer_skips_numbers_urls_emails_and_formulas() {
        let got: Vec<String> = words(
            "v2 and A4 paper https://example.com/path mail me@example.com \
             =SUM(1;2) and www.example.com stay uncheckable; prose passes",
        )
        .into_iter()
        .map(|t| t.word)
        .collect();
        assert!(!got.iter().any(|w| w == "v" || w == "A"), "digits: {got:?}");
        assert!(!got.iter().any(|w| w == "https" || w == "com" || w == "path"), "url: {got:?}");
        assert!(
            !got.iter().any(|w| w == "me" || w == "example"),
            "email run: {got:?}"
        );
        assert!(
            got.contains(&"mail".to_string()),
            "a standalone word before the address still checks: {got:?}"
        );
        assert!(!got.iter().any(|w| w == "SUM" || w == "www"), "formula/host: {got:?}");
        assert!(got.contains(&"prose".to_string()), "{got:?}");
        assert!(got.contains(&"passes".to_string()), "{got:?}");
    }

    #[test]
    fn tokenizer_skips_field_runs_but_their_display_occupies_plain_space() {
        // A literal word, a field with a cached display, another literal
        // word — the second word's offsets must account for the field's
        // display chars.
        let raw = "Draft <fld IX='0'>1.5m</fld> tail";
        let toks = words(raw);
        let got: Vec<&str> = toks.iter().map(|t| t.word.as_str()).collect();
        assert_eq!(got, vec!["Draft", "tail"], "field display never checks: {got:?}");
        let tail = &toks[1];
        // "Draft " (6) + "1.5m" (4) = 10 plain chars, then the literal
        // piece after the fld starts with its own space — "tail" at 11.
        assert_eq!(tail.start, 11);
        // The plain-space offsets map back onto the raw through the
        // command's splice-safe mapping — the markers survive.
        let span = vsdx::text::map_plain_span(raw, tail.start, tail.end).expect("maps");
        let new_raw = format!(
            "{}end{}",
            &raw[..span.head],
            &raw[span.tail..]
        );
        assert_eq!(new_raw, "Draft <fld IX='0'>1.5m</fld> end");
    }

    // -- session-level helpers -------------------------------------------

    #[test]
    fn misspelling_walk_finds_and_skips() {
        let _g = fresh_registry();
        let mut c = checker("en_US");
        c.ignore_all("wrods");
        let raw = "Misspelled wrods hide here; shpae follows.";
        let first = c.first_misspelling(raw, 0).expect("shpae flagged");
        assert_eq!(first.word, "shpae");
        assert!(c.first_misspelling(raw, first.end).is_none(), "Ignore All holds");
        assert_eq!(c.misspellings(raw).len(), 1);
    }

    #[test]
    fn tokenizer_never_merges_across_a_formatting_marker() {
        // A word stored with an INTERIOR formatting marker is two
        // tokens — `text_pieces` yields one piece per contiguous
        // literal run, so a splice can never swallow the marker
        // (CodeRabbit round: mis<cp IX='1'/>spelled).
        let raw = "mis<cp IX='1'/>spelled";
        let toks = words(raw);
        let got: Vec<&str> = toks.iter().map(|t| t.word.as_str()).collect();
        assert_eq!(got, vec!["mis", "spelled"]);
        // Replacing the second token keeps the marker byte-identical.
        let t = &toks[1];
        let span = vsdx::text::map_plain_span(raw, t.start, t.end).expect("maps");
        let new_raw = format!("{}fixed{}", &raw[..span.head], &raw[span.tail..]);
        assert_eq!(new_raw, "mis<cp IX='1'/>fixed");
    }

    #[test]
    fn map_plain_span_refuses_cdata_boundary_crossings() {
        // text_pieces merges adjacent CDATA and ordinary text into one
        // piece, so a token CAN span the boundary — checking sees the
        // merged word, but splicing it would eat the `]]>`: the mapping
        // refuses (CodeRabbit round).
        let raw = "<![CDATA[shp]]>ae tail";
        let toks = words(raw);
        let shpae = toks.iter().find(|t| t.word == "shpae").expect("merged word checks");
        assert!(vsdx::text::map_plain_span(raw, shpae.start, shpae.end).is_none());
        // The following word still splices normally.
        let tail = toks.iter().find(|t| t.word == "tail").expect("tail");
        assert!(vsdx::text::map_plain_span(raw, tail.start, tail.end).is_some());
        // Two ADJACENT CDATA sections merge into one piece the same way:
        // the merged word spans the `]]><![CDATA[` between them.
        let raw = "<![CDATA[sh]]><![CDATA[pae]]> tail";
        let toks = words(raw);
        let shpae = toks.iter().find(|t| t.word == "shpae").expect("merged word checks");
        assert!(vsdx::text::map_plain_span(raw, shpae.start, shpae.end).is_none());
        // And a span covering a formatting marker between two ordinary
        // tokens refuses the same way — the tag is a region boundary.
        let raw = "te<cp IX='1'/>h";
        assert!(vsdx::text::map_plain_span(raw, 0, 3).is_none());
        // Marker-adjacent replacements stay valid (one region each).
        assert!(vsdx::text::map_plain_span(raw, 0, 2).is_some());
        assert!(vsdx::text::map_plain_span(raw, 2, 3).is_some());
    }

    #[test]
    fn cross_language_words_flag_in_the_wrong_dictionary() {
        let _g = fresh_registry();
        // A German-only word does not pass the English dictionary and
        // vice versa — the dialog's language choice matters.
        let en = checker("en_US");
        assert!(!en.check("Diagramm"), "en flags the German word");
        let de = checker("de_DE_frami");
        assert!(!de.check("connector"), "de flags the English word");
    }

    // -- local dictionary discovery + transcoding -------------------------

    #[test]
    fn local_dictionary_discovered_transcoded_and_preferred() {
        let _g = fresh_registry(); // pinned-embedded state, serialized
        let tmp = tempfile::tempdir().expect("tmp");
        // A legacy-encoded (ISO-8859-1) pair the transcoder must rescue:
        // "café" and "rédiger".
        let aff = b"SET ISO8859-1\nWORDCHARS \xe9\n".as_slice();
        let dic = b"2\ncaf\xe9\nr\xe9diger\n".as_slice();
        std::fs::write(tmp.path().join("xx_test.aff"), aff).unwrap();
        std::fs::write(tmp.path().join("xx_test.dic"), dic).unwrap();
        set_local_dirs_for_test(vec![tmp.path().to_path_buf()]);
        {
            let mut r = registry().lock().unwrap();
            r.local_enabled = true;
            r.loaded.clear();
        }
        let entries = available();
        let entry = entries.iter().find(|e| e.stem == "xx_test").expect("local pair listed");
        assert!(entry.local.is_some(), "discovered on disk");
        assert_eq!(entry.display_name(), "xx_test", "unknown subtag shows the stem");
        let d = get("xx_test").expect("local dictionary compiles");
        assert!(d.check("café"), "latin-1 decoded to UTF-8");
        assert!(d.check("rédiger"));
        assert!(!d.check("zzznotaword"));
        // A UTF-8 local pair loads without transcoding and shadows its
        // embedded twin (the same aff, the same entries plus one local
        // extra).
        let stem = "en_US";
        let rest = EMBEDDED[0].2.split_once('\n').expect("count line").1;
        std::fs::write(tmp.path().join("en_US.aff"), EMBEDDED[0].1).unwrap();
        std::fs::write(
            tmp.path().join("en_US.dic"),
            format!("3\nzorkmidlocal\n{rest}"),
        )
        .unwrap();
        {
            let mut r = registry().lock().unwrap();
            r.local = None; // rescan
            r.loaded.clear();
        }
        let entries = available();
        let e = entries.iter().find(|e| e.stem == stem).expect("en_US listed");
        assert!(
            e.local.as_ref().is_some_and(|(a, _)| a.parent() == Some(tmp.path())),
            "the fixture pair shadows the embedded one: {:?}",
            e.local
        );
        let d = get(stem).expect("loads");
        assert!(d.check("zorkmidlocal"), "the LOCAL word list wins");
        assert!(d.check("diagram"), "rest of the vocabulary intact");
        // fresh_registry's guard releases on drop; the next test resets
        // the state itself.
    }

    #[test]
    fn decode_pair_lets_a_declared_set_win_over_valid_utf8_bytes() {
        // C3 A9 is valid UTF-8 (é) — but the aff declares ISO8859-1, and
        // the hunspell contract applies that codec to BOTH files: the
        // bytes decode as Ã© (CodeRabbit round, hunspell man page).
        let (a, d) =
            decode_pair(b"SET ISO8859-1\n".as_slice(), b"caf\xc3\xa9\n".as_slice()).unwrap();
        assert_eq!(d, "caf\u{c3}\u{a9}\n");
        assert!(a.starts_with("SET ISO8859-1"));
    }

    #[test]
    fn decode_pair_rejects_an_unknown_set_label() {
        // An unsupported declaration must not silently pick a codec —
        // the decoded vocabulary would be undetectably wrong.
        assert!(decode_pair(b"SET klingon-9\n".as_slice(), b"x\n".as_slice()).is_none());
    }

    #[test]
    fn decode_pair_prefers_utf8_and_falls_back_to_set() {
        let utf8 = decode_pair(b"SET UTF-8\n".as_slice(), "café\n".as_bytes()).unwrap();
        assert_eq!(utf8.1, "café\n");
        // ISO-8859-1 bytes with an explicit SET decode.
        let (a, d) =
            decode_pair(b"SET ISO8859-1\n".as_slice(), b"caf\xe9\n".as_slice()).unwrap();
        assert!(a.starts_with("SET ISO8859-1"));
        assert_eq!(d, "café\n");
    }
}
