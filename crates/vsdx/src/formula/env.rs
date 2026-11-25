//! Cell environments, field context, and the text-measurement bridge.

use std::collections::HashMap;

pub type Env = HashMap<String, f64>;

/// String-valued cell environment (GitHub #267): a Field's `Value`
/// formula references STRING cells (`User.Label`, `Prop.TEXT`,
/// `Sheet.5!User.ValueText`) the numeric [`Env`] cannot carry. The
/// field evaluator seeds both; bare idents resolve numeric-first,
/// string-second.
///
/// The environment is LAYERED: a child forked over a shared parent
/// ([`StrEnv::fork`]) shadows it entry-by-entry, so one render pass
/// seeds the page-wide `Sheet.N!`-qualified rows once and every
/// shape forks its own bare-name layer on top (the per-shape rebuild
/// was quadratic in field-bearing shapes — CodeRabbit PR review).
#[derive(Debug, Default)]
pub struct StrEnv {
    entries: HashMap<String, String>,
    parent: Option<std::sync::Arc<StrEnv>>,
}

impl StrEnv {
    pub fn new() -> StrEnv {
        StrEnv::default()
    }

    /// An empty child layer over a shared read-only parent: own
    /// inserts shadow the parent's entries, lookups fall through.
    pub fn fork(parent: &std::sync::Arc<StrEnv>) -> StrEnv {
        StrEnv {
            entries: HashMap::new(),
            parent: Some(std::sync::Arc::clone(parent)),
        }
    }

    pub fn insert(&mut self, key: String, value: String) {
        self.entries.insert(key, value);
    }

    /// Own layer first, then the parent chain.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .get(key)
            .map(String::as_str)
            .or_else(|| self.parent.as_ref().and_then(|p| p.get(key)))
    }
}

/// Page/document facts the field functions need (`PAGENAME()`,
/// `DOCLASTSAVE()`, …) — built per page by the renderer (the master
/// contact sheet renders page-less: `None`).
#[derive(Debug, Clone, Default)]
pub struct FieldCtx {
    /// 1-based page number (`PAGENUMBER()`).
    pub page_number: Option<f64>,
    /// Page name (`PAGENAME()`).
    pub page_name: Option<String>,
    /// The shape's master `NameU` (`MASTERNAME()`, `<no master>` when
    /// the shape has none — the corpus's own sentinel).
    pub master_name: Option<String>,
    /// `docProps/core.xml` modified timestamp (`DOCLASTSAVE()` /
    /// `DOCLASTEDIT()`), ISO form.
    pub doc_last_save: Option<String>,
    /// `docProps/core.xml` created timestamp (`DOCCREATION()`).
    pub doc_creation: Option<String>,
    /// `docProps/core.xml` facts: creator (`dc:creator`, CREATOR()),
    /// title (dc:title, TITLE()), subject (dc:subject, SUBJECT()),
    /// keywords (cp:keywords, KEYWORDS()), description
    /// (dc:description, DESCRIPTION()).
    pub doc_creator: Option<String>,
    pub doc_title: Option<String>,
    pub doc_subject: Option<String>,
    pub doc_keywords: Option<String>,
    pub doc_description: Option<String>,
    /// The document's default unit system for `FORMAT`/`FORMATEX`.
    pub units: crate::fieldfmt::UnitSystem,
}

/// Text measurement bridge for `TEXTWIDTH`/`TEXTHEIGHT` (GitHub #147).
/// The evaluator lives in this bottom crate while the metrics engine
/// (`dc::textmetrics`) lives further up; callers that CAN measure
/// implement this trait and pass it to [`eval_measured`], and the two
/// functions evaluate against real font metrics instead of erroring
/// back to the shape's cached `V`. [`eval`] keeps the no-metrics
/// behavior: `TEXTWIDTH`/`TEXTHEIGHT`/`TheText` are unsupported there
/// and callers fall back to the cached value.
pub trait TextMeasure {
    /// The sheet's `TheText` — the composed text the shape carries.
    fn the_text(&self) -> Option<String>;
    /// `TEXTWIDTH(text)`: single-line width of `text` in the shape's
    /// resolved character formatting, internal inches.
    fn text_width(&self, text: &str) -> Option<f64>;
    /// `TEXTHEIGHT(text, width)`: height of `text` wrapped at `width`
    /// internal inches (the same wrap the renderer performs — a
    /// trailing line break does NOT add a line, #148).
    fn text_height(&self, text: &str, width: f64) -> Option<f64>;
}

/// [`TextMeasure`] for callers without a metrics engine: every method
/// is unavailable, so text functions fail evaluation and callers use
/// the cached `V` (the pre-#147 behavior).
pub(crate) struct NoText;

impl TextMeasure for NoText {
    fn the_text(&self) -> Option<String> {
        None
    }
    fn text_width(&self, _text: &str) -> Option<f64> {
        None
    }
    fn text_height(&self, _text: &str, _width: f64) -> Option<f64> {
        None
    }
}
