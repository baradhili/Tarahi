//! Spell checking (#121): the dialog-driven Check Spelling engine —
//! an embedded English wordlist, a text tokenizer, and ranked
//! suggestions.
//!
//! Wordlist: the vendored SCOWL-derived american-english list
//! (`assets/words/` — provenance beside the files), checked
//! case-insensitively. Suggestions: bounded Damerau-Levenshtein
//! (transpositions count, "teh"→"the") with an early-exit band over
//! the whole list, ranked by distance then shape. Tokenizer: ASCII
//! alphabetic runs honoring INTERNAL apostrophes and hyphens
//! ("don't", "state-of-the-art"); numbers, URLs, and non-ASCII text
//! never tokenize. Field runs (#267's structured pieces) are skipped
//! by the caller — a computed value is not checkable text.
//!
//! Fixes ride the `text_edit_macro` precedent: one undoable SetText
//! per accepted Change with the TEXTWIDTH/TEXTHEIGHT cache refresh in
//! the same undo step.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The top-of-ranking tier: ~250 ubiquitous English function and
/// content words, hand-authored (common knowledge, not copied from
/// any corpus). A misspelling one edit away from one of these —
/// "teh"→"the", "adn"→"and" — suggests it ahead of the alphabetical
/// run of equal-distance rarities, which is what the frequency-
/// ranked checkers achieve with a corpus. The dictionary itself
/// carries NO frequency information.
const COMMON: [&str; 255] = [
    "the", "be", "to", "of", "and", "a", "in", "that", "have", "it",
    "for", "not", "on", "with", "he", "as", "you", "do", "at", "this",
    "but", "his", "by", "from", "they", "we", "say", "her", "she", "or",
    "an", "will", "my", "one", "all", "would", "there", "their", "what",
    "so", "up", "out", "if", "about", "who", "get", "which", "go", "me",
    "when", "make", "can", "like", "time", "no", "just", "him", "know",
    "take", "people", "into", "year", "your", "good", "some", "could",
    "them", "see", "other", "than", "then", "now", "look", "only", "come",
    "its", "over", "think", "also", "back", "after", "use", "two", "how",
    "our", "work", "first", "well", "way", "even", "new", "want", "any",
    "these", "give", "day", "most", "us", "is", "was", "are", "were",
    "been", "has", "had", "did", "having", "may", "should", "each",
    "because", "does", "while", "where", "during", "before", "above",
    "between", "under", "both", "same", "such", "very", "own", "down",
    "need", "too", "here", "must", "without", "within", "process",
    "system", "data", "value", "name", "type", "size", "width", "height",
    "line", "text", "shape", "page", "file", "document", "list", "set",
    "point", "number", "page", "color", "style", "form", "item", "area",
    "level", "top", "bottom", "left", "right", "center", "middle",
    "start", "end", "next", "previous", "open", "close", "save", "print",
    "select", "copy", "paste", "delete", "change", "add", "remove",
    "insert", "move", "group", "align", "order", "view", "zoom", "fit",
    "design", "review", "insert", "draw", "make", "run", "test", "check",
    "help", "about", "yes", "no", "ok", "cancel", "apply", "default",
    "custom", "general", "none", "auto", "manual", "on", "off", "more",
    "less", "best", "new", "old", "high", "low", "small", "large",
    "big", "little", "long", "short", "wide", "thin", "thick", "dark",
    "light", "red", "green", "blue", "yellow", "black", "white", "gray",
    "email", "address", "phone", "date", "name", "title", "company",
    "manager", "project", "team", "plan", "task", "step", "phase",
    "flow", "chart", "diagram", "drawing", "model", "link", "comment",
];

/// The embedded wordlist as an iterator (the merged-dictionary
/// source: `Spell::new(embedded_words().chain(custom...))`).
pub fn embedded_words() -> impl Iterator<Item = String> {
    include_str!("../assets/words/en-american.txt")
        .lines()
        .map(str::to_lowercase)
}

/// The embedded dictionary, lowercased into a hash set on first use.
pub fn embedded() -> &'static Spell {
    static SPELL: OnceLock<Spell> = OnceLock::new();
    SPELL.get_or_init(|| Spell::new(embedded_words()))
}

/// A word list: the case-insensitive check set.
pub struct Spell {
    words: HashSet<String>,
}

impl Spell {
    pub fn new(words: impl Iterator<Item = String>) -> Spell {
        Spell {
            words: words.filter(|w| !w.is_empty()).collect(),
        }
    }

    /// Merge extra words (the app-level custom dictionary).
    pub fn add(&mut self, word: &str) {
        let w = word.trim().to_lowercase();
        if !w.is_empty() {
            self.words.insert(w);
        }
    }

    /// Case-insensitive membership. Single letters and tokens with
    /// digits are accepted without lookup (the tokenizer pre-filters,
    /// but callers may hand anything through).
    pub fn correct(&self, word: &str) -> bool {
        if word.chars().any(|c| c.is_ascii_digit()) {
            return true;
        }
        if word.chars().count() <= 1 {
            return true;
        }
        self.words.contains(&word.to_lowercase())
    }

    /// Top suggestions for `word`: whole-list bounded
    /// Damerau-Levenshtein scan, ranked by distance then length
    /// proximity then alphabetical order (deterministic). Words much
    /// longer/shorter than the target are skipped by the band.
    pub fn suggestions(&self, word: &str, max: usize) -> Vec<String> {
        let w = word.to_lowercase();
        let n = w.chars().count();
        let first = w.chars().next();
        // Rank: distance, then the COMMON tier (the hand-authored
        // function-word list — "teh" suggests "the" ahead of "meh"),
        // then same leading letter, then length proximity, then
        // alphabetical (deterministic).
        let mut scored: Vec<(u8, u8, u8, usize, String)> = Vec::new();
        for cand in &self.words {
            let m = cand.chars().count();
            if m.abs_diff(n) > 2 {
                continue;
            }
            let d = bounded_dl(&w, cand, 2);
            if d <= 2 {
                let common = u8::from(!COMMON.contains(&cand.as_str()));
                let first_match = u8::from(cand.chars().next() != first);
                scored.push((d, common, first_match, m.abs_diff(n), cand.clone()));
            }
        }
        scored.sort();
        scored.truncate(max);
        scored.into_iter().map(|(_, _, _, _, c)| c).collect()
    }
}

/// Damerau-Levenshtein with an early exit past `max`: true distance
/// when ≤ max, else `max + 1`. Two-row DP with the transposition
/// lookback.
fn bounded_dl(a: &str, b: &str, max: u8) -> u8 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) as u8 > max {
        return max + 1;
    }
    let mut prev2: Vec<u16> = (0..=m as u16).collect();
    let mut prev: Vec<u16> = (0..=m as u16).collect();
    for i in 1..=n {
        let mut cur = vec![i as u16; m + 1];
        for j in 1..=m {
            let cost = u16::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
        }
        // Row-minimum early exit: the distance can only grow.
        if cur.iter().min().copied().unwrap_or(u16::MAX) as u8 > max {
            return max + 1;
        }
        prev2 = prev;
        prev = cur;
    }
    let d = prev[m];
    if d as u8 > max {
        max + 1
    } else {
        d as u8
    }
}

/// One checkable word: its byte range in the checked text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub text: String,
}

/// Tokenize `text`: ASCII alphabetic runs honoring INTERNAL
/// apostrophes and hyphens (never leading/trailing). Digits break a
/// run; URLs (`://` runs) and non-ASCII text yield nothing. The
/// ranges are byte offsets into `text`, so a fix is a plain
/// replacement.
pub fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // A URL run (`scheme://…` up to whitespace) never tokenizes:
        // find the alpha run, check for the scheme marker, skip whole.
        if b[i].is_ascii_alphabetic() {
            let mut k = i;
            while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'.' || b[k] == b'-') {
                k += 1;
            }
            if b[k..].starts_with(b"://") {
                while k < b.len() && !b[k].is_ascii_whitespace() {
                    k += 1;
                }
                i = k;
                continue;
            }
            let start = i;
            let mut end = i;
            let mut j = i;
            while j < b.len() {
                let c = b[j];
                if c.is_ascii_alphabetic() {
                    j += 1;
                    end = j;
                } else if (c == b'\'' || c == b'-')
                    && j > start
                    && j + 1 < b.len()
                    && b[j + 1].is_ascii_alphabetic()
                    && end > start
                {
                    // Internal separator only when a letter ran before
                    // AND one follows — trailing/leading never join.
                    j += 1;
                } else {
                    break;
                }
            }
            if end - start > 1 {
                out.push(Token {
                    start,
                    text: text[start..end].to_string(),
                });
            }
            i = end.max(start + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// The plain-text byte spans a shape's FIELD runs occupy (#267): the
/// caller skips tokens overlapping any span — a spell change to a
/// computed value is meaningless.
pub fn field_run_spans(raw: Option<&str>) -> Vec<(usize, usize)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    if !raw.contains("<fld") {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for piece in vsdx::text_pieces(raw) {
        let len = piece.text.len();
        if piece.fld.is_some() && len > 0 {
            spans.push((pos, pos + len));
        }
        pos += len;
    }
    spans
}

/// The first misspelled token in `text` outside `ignore` (words),
/// outside `ignored_spans` (byte ranges — the session's already-
/// reviewed occurrences), and outside the field spans. `None` when
/// the text is clean.
pub fn first_error(
    spell: &Spell,
    text: &str,
    raw: Option<&str>,
    ignore: &HashSet<String>,
    ignored_spans: &[(usize, usize)],
) -> Option<Token> {
    let spans = field_run_spans(raw);
    tokens(text)
        .into_iter()
        .find(|t| {
            let end = t.start + t.text.len();
            !ignore.contains(&t.text.to_lowercase())
                && !spans.iter().any(|(a, b)| t.start >= *a && t.start < *b)
                && !ignored_spans
                    .iter()
                    .any(|(a, b)| t.start < *b && *a < end)
                && !spell.correct(&t.text)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_loads_and_checks() {
        let s = embedded();
        assert!(s.correct("diagram"));
        assert!(s.correct("Diagram"), "case-insensitive");
        assert!(s.correct("don't"), "apostrophe forms ship in the list");
        assert!(!s.correct("managr"));
    }

    #[test]
    fn tokenizer_rules() {
        let t = tokens("The don't state-of-the-art value42 see http://x.io");
        let words: Vec<&str> = t.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(words, ["The", "don't", "state-of-the-art", "value", "see"]);
        // `value42` splits at the digit; the number itself never checks.
        let t = tokens("hello world");
        assert_eq!(t[0].start, 0);
        assert_eq!(t[1].start, 6);
    }

    #[test]
    fn field_spans_are_skipped() {
        // Literal text with a cached field value in the middle.
        let raw = "The <fld IX='0'>Managr</fld> approves";
        let s = embedded();
        let text = "The Managr approves";
        // Without the span the cached value (a typo here for the test)
        // would flag; with it, nothing on the field run is checked.
        assert!(first_error(s, text, Some(raw), &HashSet::new(), &[]).is_none());
        // A literal typo OUTSIDE the field still flags.
        let raw2 = "The <fld IX='0'>Manager</fld> approves";
        let text2 = "The Manager approves";
        let raw_typo = text2.replacen("approves", "aproves", 1);
        let _ = raw_typo;
        let t = first_error(s, "The Manager aproves", Some(raw2), &HashSet::new(), &[]);
        assert_eq!(t.map(|t| t.text), Some("aproves".to_string()));
    }

    #[test]
    fn suggestions_rank_sensible() {
        let s = embedded();
        let top = s.suggestions("teh", 5);
        assert!(top.contains(&"the".to_string()), "teh → the: {top:?}");
        let top = s.suggestions("managr", 5);
        assert!(top.contains(&"manager".to_string()), "managr → manager: {top:?}");
    }

    #[test]
    fn custom_words_join_the_check() {
        let mut s = Spell::new(["the".to_string(), "and".to_string()].into_iter());
        assert!(!s.correct("Tarahi"));
        s.add("Tarahi");
        assert!(s.correct("Tarahi"));
    }
}
