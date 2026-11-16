//! Spell checking session (#121): the dialog-driven Check Spelling
//! pass over a page's shape text. The engine lives in `dc::spell`
//! (dictionary, tokenizer, suggestions); THIS module owns the pass —
//! the shape-by-shape walk, the Ignore-All set (per-session, in
//! memory), and the app-level custom dictionary (the `language.json`
//! precedent: persisted in the config dir, NEVER in the `.vsdx`).
//!
//! The pass is SERIAL, Visio-style: shapes in page order; each
//! shape's text walks to its first misspelling outside the ignored
//! set, outside the already-reviewed occurrences, and outside field
//! runs (a computed value is not checkable text — #267's structured
//! pieces); the dialog resolves it (Ignore / Ignore All / Change /
//! Change All / Add) and the walk continues. A Change applies through
//! the `text_edit_macro` precedent: one undoable SetText per accepted
//! fix, cache refresh in the same step.

use crate::recent::config_base;
use std::collections::HashSet;
use std::path::PathBuf;

const SCHEMA_VERSION: u64 = 1;

/// One misspelling under review: byte range in the shape's plain
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    pub start: usize,
    pub text: String,
}

impl Misspelling {
    fn span(&self) -> (usize, usize) {
        (self.start, self.start + self.text.len())
    }
}

/// The pass state: the page's text-bearing shape ids in order, the
/// walk cursor, the Ignore-All set, and the current shape's
/// already-reviewed occurrences (a plain Ignore stays on the shape —
/// its NEXT misspelling surfaces next).
pub struct SpellSession {
    pub shape_ids: Vec<i64>,
    pub shape_ix: usize,
    pub ignore_all: HashSet<String>,
    pub ignored_here: Vec<Misspelling>,
    pub page_shape_count: usize,
}

impl SpellSession {
    /// Walk the page's top-level shapes (document order) that carry
    /// any text.
    pub fn start(doc: &vsdx::Document, page_id: i64) -> SpellSession {
        let ids: Vec<i64> = doc
            .pages
            .iter()
            .find(|p| p.id == page_id)
            .map(|p| {
                p.shapes
                    .iter()
                    .filter(|s| s.sheet.text.as_deref().is_some_and(|t| !t.is_empty()))
                    .map(|s| s.id)
                    .collect()
            })
            .unwrap_or_default();
        let n = ids.len();
        SpellSession {
            shape_ids: ids,
            shape_ix: 0,
            ignore_all: HashSet::new(),
            ignored_here: Vec::new(),
            page_shape_count: n,
        }
    }

    /// The current shape's id, if the walk is still inside the page.
    pub fn current_shape(&self) -> Option<i64> {
        self.shape_ids.get(self.shape_ix).copied()
    }

    /// The first misspelling of the current shape — always a FRESH
    /// scan (the text may have changed since the last look).
    pub fn current_miss(
        &self,
        spell: &dc::spell::Spell,
        doc: &vsdx::Document,
        page_id: i64,
    ) -> Option<Misspelling> {
        let sid = self.current_shape()?;
        let page = doc.pages.iter().find(|p| p.id == page_id)?;
        let s = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
        let spans: Vec<(usize, usize)> =
            self.ignored_here.iter().map(Misspelling::span).collect();
        dc::spell::first_error(
            spell,
            s.sheet.text.as_deref().unwrap_or(""),
            s.sheet.text_raw.as_deref(),
            &self.ignore_all,
            &spans,
        )
        .map(|t| Misspelling {
            start: t.start,
            text: t.text,
        })
    }

    /// Ignore this occurrence only: the shape's next misspelling (if
    /// any) surfaces next.
    pub fn ignore(&mut self, miss: &Misspelling) {
        self.ignored_here.push(miss.clone());
    }

    /// Ignore All: the word drops out of every later check this pass.
    pub fn ignore_all(&mut self, miss: &Misspelling) {
        self.ignore_all.insert(miss.text.to_lowercase());
    }

    /// The shape is clean or resolved — next shape.
    pub fn advance(&mut self) {
        self.shape_ix += 1;
        self.ignored_here.clear();
    }

    /// A Change landed in the current shape: restart its scan fresh
    /// (offsets shifted).
    pub fn changed(&mut self) {
        self.ignored_here.clear();
    }
}

/// The app-level custom dictionary: a JSON file in the config dir
/// beside `language.json`. Missing/corrupt = empty; persistence is
/// best-effort (a read-only config dir never takes the app down —
/// the MRU discipline).
pub struct CustomWords {
    words: Vec<String>,
    store: Option<PathBuf>,
}

impl CustomWords {
    /// Open from the user's config dir.
    pub fn open() -> CustomWords {
        let store = config_base().map(|b| b.join("diagramcraft").join("custom_words.json"));
        let words = store
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| parse(&text))
            .unwrap_or_default();
        CustomWords { words, store }
    }

    /// An empty, non-persisting set (headless tests, kittest).
    pub fn ephemeral() -> CustomWords {
        CustomWords {
            words: Vec::new(),
            store: None,
        }
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// Add one word (trimmed, non-empty, deduped) and persist.
    pub fn add(&mut self, word: &str) {
        let w = word.trim().to_string();
        if w.is_empty() {
            return;
        }
        if !self.words.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
            self.words.push(w);
            self.persist();
        }
    }

    fn persist(&self) {
        let Some(path) = &self.store else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            path,
            serde_json::json!({ "version": SCHEMA_VERSION, "words": self.words }).to_string(),
        );
    }
}

fn parse(text: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("version")?.as_u64()? != SCHEMA_VERSION {
        return None;
    }
    let words = v.get("words")?.as_array()?;
    Some(
        words
            .iter()
            .filter_map(|w| w.as_str().map(str::to_string))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk visits text-bearing shapes in order and stops at the
    /// page end.
    #[test]
    fn session_walks_text_bearing_shapes() {
        let mut doc = vsdx::Document::default();
        let mk = |id: i64, text: &str| {
            let mut s = vsdx::Shape {
                id,
                ..Default::default()
            };
            if !text.is_empty() {
                s.sheet.text = Some(text.into());
            }
            s
        };
        doc.pages.push(vsdx::Page {
            id: 0,
            name: "P".into(),
            shapes: vec![mk(1, "alpha"), mk(2, ""), mk(3, "beta")],
            ..Default::default()
        });
        let ses = SpellSession::start(&doc, 0);
        assert_eq!(ses.shape_ids, vec![1, 3], "the empty shape skips");
        assert_eq!(ses.page_shape_count, 2);
    }

    /// Ignore stays on the shape (the next misspelling surfaces);
    /// Ignore All lifts the word for the rest of the pass; advance
    /// clears the per-shape review list.
    #[test]
    fn ignore_semantics() {
        let mut doc = vsdx::Document::default();
        doc.pages.push(vsdx::Page {
            id: 0,
            name: "P".into(),
            shapes: vec![vsdx::Shape {
                id: 1,
                ..Default::default()
            }],
            ..Default::default()
        });
        doc.pages[0].shapes[0].sheet.text = Some("teh teh the".into());
        let mut ses = SpellSession::start(&doc, 0);
        let spell = dc::spell::Spell::new(["the".to_string()].into_iter());
        let m1 = ses.current_miss(&spell, &doc, 0).unwrap();
        assert_eq!(m1.text, "teh");
        ses.ignore(&m1);
        let m2 = ses.current_miss(&spell, &doc, 0).unwrap();
        assert_eq!(m2.start, 4, "the SECOND occurrence surfaces");
        ses.ignore_all(&m2);
        assert!(ses.current_miss(&spell, &doc, 0).is_none(), "the lifted");
    }

    /// Custom words persist and round-trip; a corrupt store is empty.
    #[test]
    fn custom_words_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("custom_words.json");
        {
            let mut cw = CustomWords {
                words: Vec::new(),
                store: Some(path.clone()),
            };
            cw.add("  Tarahi ");
            cw.add("Tarahi"); // deduped, case-insensitive
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Tarahi"));
        let parsed = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed, ["Tarahi"]);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(parse(&std::fs::read_to_string(&path).unwrap()), None);
    }
}
