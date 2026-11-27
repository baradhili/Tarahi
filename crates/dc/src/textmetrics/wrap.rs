// ------------- CJK line-break classes (#282 zh-Hans) -------------
//
// The greedy wrap is space-based (0.2.9), and Chinese paragraphs
// carry no spaces: without these classes a whole paragraph measures
// as ONE unbreakable word that only the over-long hard-split can cut
// — at any character, ignoring the kinsoku rules. The classes below
// give the wrap break opportunities BETWEEN ideographs (UAX #14
// lite), with the two prohibition families Chinese typography
// requires. The ja wave (#282) extends both ends of the classes to
// Japanese: halfwidth kana join the grant class, and the Japanese
// kinsoku starters (small kana, prolonged mark, iteration marks,
// middle dots) join the line-head prohibitions.

/// The "break between any two" class: CJK ideographs and kana, both
/// full- and half-width. Hangul is deliberately absent — Korean text
/// is space-separated and needs no intra-word breaks (`ko` rides the
/// wave's fonts, not these classes).
fn is_cjk_ideographic(c: char) -> bool {
    matches!(
        c,
        '\u{3005}'..='\u{3007}'     // 々〆〇 (iteration/closing/zero)
            | '\u{3400}'..='\u{4DBF}'     // CJK Extension A
            | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs (the zh workhorse)
            | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
            | '\u{3040}'..='\u{30FF}' // Hiragana + Katakana (ja rides the same classes)
            | '\u{31F0}'..='\u{31FF}' // Katakana Phonetic Extensions
            | '\u{FF66}'..='\u{FF9F}' // halfwidth kana (legacy Japanese Visio text)
    )
}

/// Full-width CJK closing punctuation (kinsoku 禁則): grants the
/// break AFTER it (sentence enders) and never starts a line. The
/// Japanese additions at the tail — the middle dots and the halfwidth
/// closers — behave the same way.
fn is_cjk_closing(c: char) -> bool {
    const CLOSING: &str = "。，、；：！？）］｝〉》」』】〕”’…—～·％￥・｡｣､･";
    CLOSING.contains(c)
}

/// Japanese kinsoku (行頭禁則) starters — the small kana, the
/// prolonged sound mark, the iteration marks, the middle dots, the
/// STANDALONE sound marks (゛゜ and their halfwidth twins ﾞﾟ — Lm
/// class, not combining, so only the kinsoku rule keeps them glued
/// to their base) and the halfwidth small kana a wrapped Japanese
/// line may never BEGIN with (UAX #14's CJ class, lite). Each stays
/// glueable to its preceding character: a break may END a line on
/// the preceding char, never leave one of these to open the next.
fn is_ja_no_line_start(c: char) -> bool {
    matches!(
        c,
        '\u{3005}'                      // 々
            | '\u{309B}'..='\u{309C}'   // ゛゜ (standalone sound marks)
            | '\u{309D}'..='\u{309E}'   // ゝゞ
            | '\u{3041}' | '\u{3043}' | '\u{3045}' | '\u{3047}' | '\u{3049}' // ぁぃぅぇぉ
            | '\u{3095}' | '\u{3096}'   // ゕゖ (small ka/ke)
            | '\u{3063}'                // っ
            | '\u{3083}' | '\u{3085}' | '\u{3087}' | '\u{308E}' // ゃゅょゎ
            | '\u{30A1}' | '\u{30A3}' | '\u{30A5}' | '\u{30A7}' | '\u{30A9}' // ァィゥェォ
            | '\u{30C3}'                // ッ
            | '\u{30E3}' | '\u{30E5}' | '\u{30E7}' | '\u{30EE}' // ャュョヮ
            | '\u{30F5}' | '\u{30F6}'   // ヵヶ
            | '\u{30FC}'                // ー
            | '\u{30FD}'..='\u{30FE}'   // ヽヾ
            | '\u{31F0}'..='\u{31FF}'   // small katakana extensions ㇰㇱ…
            | '\u{FF67}'..='\u{FF70}'   // halfwidth small kana + ｰ
            | '\u{FF9E}'..='\u{FF9F}'   // ﾞﾟ (halfwidth sound marks)
    )
}

/// Chars a line may never START with: the CJK closers, the ASCII
/// closers riding along, and the Japanese kinsoku starters, so a Han
/// char before an ASCII `.`, `)`, … or a kana before `ー` stays glued
/// the same way. ASCII closers SUPPRESS only — they never GRANT a
/// boundary (CodeRabbit on #282: letting them grant split `x.y` into
/// `x.`|`y`), so Latin-internal tokenization stays space-driven.
/// Opening punct needs no set of its own — it grants no break (see
/// [`cjk_break_between`]), so a line never ends right after （ 「 『.
/// Public to the wrap's emergency splitter, which must never leave a
/// forbidden char to OPEN a line either.
pub(crate) fn line_head_forbidden(c: char) -> bool {
    is_cjk_closing(c)
        || is_ja_no_line_start(c)
        || matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '%')
}

/// Explicit non-breaking controls (UAX #14 class GL and friends): no
/// boundary sits before OR after one — `图⍽表` with a no-break space
/// is the author saying "keep these together".
pub(crate) fn is_glue_pub(c: char) -> bool {
    is_glue(c)
}

fn is_glue(c: char) -> bool {
    matches!(c, '\u{00A0}' | '\u{202F}' | '\u{2060}' | '\u{FEFF}')
}

/// Combining marks and joiners: a boundary adjacent to one is not a
/// grapheme-cluster boundary (UAX #29) — breaking there splits a
/// decomposed character (カ + dakuten, CodeRabbit on #282).
fn is_combining(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'       // combining diacritical marks
            | '\u{3099}'..='\u{309A}' // combining kana voicing
            | '\u{FE00}'..='\u{FE0F}' // variation selectors
            | '\u{200D}'              // zero-width joiner
            | '\u{20D0}'..='\u{20FF}' // combining marks for symbols
    )
}

/// The emergency hard-splitter's combining predicate (svg.rs).
pub(crate) fn is_combining_pub(c: char) -> bool {
    is_combining(c)
}

/// A wrap opportunity between `a` and `b`: the LEFT char must grant
/// one — an ideograph, kana, or a full-width CJK closing punct (Latin
/// and ASCII punctuation never grant; Latin-internal breaks stay
/// space-driven, so pure-Latin wrap output is untouched) — and the
/// RIGHT char must accept it (not a line-head-forbidden closer). A
/// boundary adjacent to glue or a combining mark never opens
/// (explicit no-break controls and grapheme interiors); after a
/// COMPLETE combining sequence the cluster's BASE decides —
/// `カ\u{3099}ク` breaks before `ク` exactly like the precomposed
/// `ガク` does (CodeRabbit PR review).
pub(crate) fn cjk_break_between(a: char, b: char) -> bool {
    if is_glue(a) || is_glue(b) || is_combining(b) {
        return false;
    }
    if is_combining(a) {
        // `a` ends a combining sequence, and the pair alone only
        // knows the base for KANA VOICING (its base is definitionally
        // kana): `カ\u{3099}ク` breaks like the precomposed `ガク`.
        // Every other mark's base is unknowable from the pair — the
        // tokenizer classifies those through the tracked BASE
        // instead, so `A\u{0301}中` wraps exactly like `Á中` (no
        // break; CodeRabbit PR review).
        return matches!(a, '\u{3099}'..='\u{309A}') && is_cjk_ideographic(b);
    }
    (is_cjk_ideographic(a) || is_cjk_closing(a)) && !line_head_forbidden(b)
}

/// One greedy-wrap token of a paragraph: `sep` is the separator
/// content that precedes the token when it JOINS the current line
/// (the one space of a word boundary, the ideographic space U+3000,
/// or nothing at a CJK break); a line break consumes its separator,
/// exactly like the space-based wrap consumed the joining space.
pub(crate) struct WrapToken<'a> {
    pub sep: &'a str,
    pub tok: &'a str,
}

/// Tokenize one paragraph for the greedy wrap: words separated by
/// ASCII spaces (each space reproducing the `split(' ')` /
/// rejoin-with-one-space semantics verbatim — including the empty
/// words consecutive spaces yield, so space runs and leading/trailing
/// spaces survive byte-for-byte), the ideographic space U+3000
/// breaking like a space, and every [`cjk_break_between`] opportunity
/// inside a word splitting it — so Chinese paragraphs wrap between
/// ideographs under the kinsoku rules instead of measuring as one
/// unbreakable word. A paragraph without CJK grant-chars tokenizes
/// to its unsplit words: wrap output is byte-identical to the
/// space-only engine.
pub(crate) fn wrap_tokens(para: &str) -> Vec<WrapToken<'_>> {
    // Parts between separators — `split(' ')` semantics, generalized
    // to U+3000: every separator char ends a part (possibly empty),
    // so a trailing separator yields a final empty part and
    // consecutive separators yield empty parts between, exactly like
    // `para.split(' ')`.
    let mut parts: Vec<(char, &str)> = Vec::new();
    {
        let bytes = para.as_bytes();
        let mut start = 0usize;
        let mut sep = ' ';
        let mut i = 0usize;
        while i < bytes.len() {
            let c = para[i..].chars().next().unwrap();
            if c == ' ' || c == '\u{3000}' {
                parts.push((sep, &para[start..i]));
                sep = c;
                start = i + c.len_utf8();
            }
            i += c.len_utf8();
        }
        parts.push((sep, &para[start..]));
    }
    // Each part splits at the CJK opportunities; the FIRST sub-token
    // carries the part's separator (empty for the paragraph's opening
    // part — the wrap loop starts unstarted), the rest join with
    // nothing.
    let mut out: Vec<WrapToken> = Vec::new();
    for (ix, (sep, part)) in parts.into_iter().enumerate() {
        let mut pending_sep: &str = if ix == 0 {
            ""
        } else {
            match sep {
                '\u{3000}' => "\u{3000}",
                _ => " ",
            }
        };
        let mut sub_start = 0usize;
        let mut prev: Option<char> = None;
        // The running cluster BASE: when `prev` is a combining mark,
        // the boundary classifies through the base — a decomposed
        // sequence wraps like its precomposed twin (`A\u{0301}中`
        // matches `Á中`; `カ\u{3099}ク` matches `ガク` — CodeRabbit
        // PR review).
        let mut base: Option<char> = None;
        // Where the previous char starts: a hyphen opening the part
        // (`-5`, `-x`) is a sign, not a break opportunity (UAX #14 LB20.1).
        let mut prev_at = 0usize;
        for (i, c) in part.char_indices() {
            let left = if prev.is_some_and(is_combining) {
                base
            } else {
                prev
            };
            // A break after a hyphen inside a word (UAX #14 LB21: never
            // before one; LB25: never before a digit, so `2026-11` and
            // `-5` stay whole). Visio 2021 wraps `nsa1-ext` as
            // `nsa1-` / `ext` in a narrow box.
            let after_hyphen = matches!(prev, Some('-' | '\u{2010}'))
                && prev_at > 0
                && !c.is_ascii_digit()
                && !matches!(c, '-' | '\u{2010}');
            if after_hyphen || left.is_some_and(|p| cjk_break_between(p, c)) {
                out.push(WrapToken {
                    sep: pending_sep,
                    tok: &part[sub_start..i],
                });
                pending_sep = "";
                sub_start = i;
            }
            prev = Some(c);
            prev_at = i;
            if !is_combining(c) {
                base = Some(c);
            }
        }
        out.push(WrapToken {
            sep: pending_sep,
            tok: &part[sub_start..],
        });
    }
    // UAX #14 LB13: no break before `)` `]` `}` `!` `?` `,` `.` `:` `;`
    // or `/`, even after spaces: such a token joins the one before it,
    // separator included. Visio 2021 wraps `… nsa1-ext / nsa2-ext …` in
    // a narrow box as `… nsa1-` / `ext / nsa2-ext …`: `ext /` did not
    // fit, and the break before `/` is not allowed.
    let mut merged: Vec<WrapToken<'_>> = Vec::with_capacity(out.len());
    for t in out {
        let glue = !t.sep.is_empty()
            && t.tok
                .starts_with([')', ']', '}', '!', '?', ',', '.', ':', ';', '/']);
        if let (true, Some(prev)) = (glue, merged.last_mut()) {
            let base = para.as_ptr() as usize;
            let start = prev.tok.as_ptr() as usize - base;
            let end = t.tok.as_ptr() as usize - base + t.tok.len();
            prev.tok = &para[start..end];
            continue;
        }
        merged.push(t);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #282 zh-Hans: the CJK break classes — opportunities between
    /// ideographs, kinsoku suppression before closing punctuation,
    /// never inside Latin (the space-only engine keeps its output).
    #[test]
    fn cjk_break_classes_follow_kinsoku() {
        use super::cjk_break_between as brk;
        // Between two ideographs: the workhorse opportunity.
        assert!(brk('图', '表'));
        assert!(brk('フ', 'ロ'));
        // フォ stays glued (the ja wave's kinsoku: small kana never
        // start a line — see ja_kinsoku_extends_the_cjk_classes).
        assert!(!brk('フ', 'ォ'));
        // Closing punctuation never STARTS a line, and grants the
        // break that FOLLOWS it (sentence enders).
        assert!(!brk('图', '。'));
        assert!(brk('。', '下'));
        assert!(!brk('表', '，'));
        assert!(!brk('形', ')'));
        assert!(!brk('状', '%'));
        // Opening punctuation never ENDS a line (it grants nothing)…
        assert!(!brk('（', '中'));
        // …but a line may START with one (only closers are
        // line-head-forbidden — the strict rule set).
        assert!(brk('起', '（'));
        // Latin-internal: no new opportunities.
        assert!(!brk('a', 'b'));
        assert!(!brk('1', '2'));
        // ASCII closers suppress but never GRANT — `x.y` and `3.14`
        // stay whole (CodeRabbit on #282).
        assert!(!brk('.', 'y'));
        assert!(!brk('.', '1'));
        assert!(!brk(')', 'x'));
        // …but a Han-to-Latin boundary is a real break.
        assert!(brk('图', 'a'));
        // Explicit non-breaking controls bind both sides.
        assert!(!brk('图', '\u{00A0}'));
        assert!(!brk('\u{00A0}', '表'));
        assert!(!brk('图', '\u{2060}'));
        // Combining sequences never split INTERNALLY (decomposed ガ
        // = カ + U+3099), but a COMPLETE sequence breaks before a
        // following kana exactly like the precomposed form
        // (CodeRabbit PR review).
        assert!(!brk('カ', '\u{3099}'));
        assert!(brk('\u{3099}', 'ク'));
    }

    /// #282 ja: the Japanese kinsoku extensions — halfwidth kana join
    /// the grant class, and the Japanese starters (small kana, the
    /// prolonged mark, iteration marks, the middle dots and the
    /// halfwidth closers) never open a line but grant the break after
    /// themselves. Korean stays outside the grant classes entirely
    /// (Hangul is space-separated; `ko` rides the wave's fonts only).
    #[test]
    fn ja_kinsoku_extends_the_cjk_classes() {
        use super::cjk_break_between as brk;
        // Halfwidth kana split like their fullwidth twins…
        assert!(brk('ｱ', 'ｲ'));
        // …and the prolonged mark grants the break AFTER it but never
        // starts a line, full- or halfwidth.
        assert!(brk('ー', 'ア'));
        assert!(!brk('ア', 'ー'));
        assert!(brk('ｰ', 'ｱ'));
        assert!(!brk('ｱ', 'ｰ'));
        // Small kana, iteration marks and middle dots stay glued to
        // their predecessor.
        assert!(!brk('ア', 'ィ'));
        assert!(!brk('カ', 'っ'));
        assert!(!brk('あ', 'ゕ'));
        assert!(!brk('け', 'ゖ'));
        assert!(!brk('追', '々'));
        assert!(!brk('サ', '・'));
        // The STANDALONE sound marks (both widths — Lm class, not
        // combining, so only the kinsoku rule glues them) never start
        // a line, but grant the break AFTER them (CodeRabbit round 2
        // on #282: the halfwidth grant range had left ﾞ startable).
        assert!(!brk('カ', '\u{309B}'));
        assert!(!brk('ｶ', '\u{FF9E}'));
        assert!(brk('\u{FF9E}', 'ｸ'));
        assert!(brk('\u{309B}', 'カ'));
        // The halfwidth closers behave like the fullwidth ones.
        assert!(!brk('サ', '｡'));
        assert!(brk('｡', 'サ'));
        assert!(!brk('サ', '､'));
        assert!(brk('､', 'サ'));
        // The wrap tokenizer agrees: ハロー wraps as ハ / ロー (the
        // prolonged mark glued), サン・ハイツ drops the dot onto the
        // first line and breaks after it.
        let toks = |s: &str| {
            wrap_tokens(s)
                .into_iter()
                .map(|t| (t.sep.to_string(), t.tok.to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            toks("ハロー"),
            vec![("".into(), "ハ".into()), ("".into(), "ロー".into())]
        );
        assert_eq!(
            toks("サン・ハイツ"),
            vec![
                ("".into(), "サ".into()),
                ("".into(), "ン・".into()),
                ("".into(), "ハ".into()),
                ("".into(), "イ".into()),
                ("".into(), "ツ".into()),
            ]
        );
        // Korean: no intra-word opportunities — 한글 measures as one
        // space-driven word.
        assert_eq!(toks("한글 문서"), vec![
            ("".into(), "한글".into()),
            (" ".into(), "문서".into()),
        ]);
        // Decomposed halfwidth kana wrap like their precomposed
        // twins: ｶﾞ stays one unit and the break before ｸ survives
        // (CodeRabbit round 2 on #282).
        assert_eq!(
            toks("ｶ\u{FF9E}ｸ"),
            vec![
                ("".into(), "ｶ\u{FF9E}".into()),
                ("".into(), "ｸ".into()),
            ]
        );
        assert_eq!(
            toks("ﾊ\u{FF9F}ｲ"),
            vec![
                ("".into(), "ﾊ\u{FF9F}".into()),
                ("".into(), "ｲ".into()),
            ]
        );
    }

    /// #282 zh-Hans: the wrap tokenizer reproduces the space-only
    /// engine's parts verbatim for Latin (goldens pin the wrap), and
    /// splits CJK words at the kinsoku opportunities — with the
    /// U+3000 ideographic space acting as a word separator.
    #[test]
    fn wrap_tokens_split_cjk_and_preserve_latin() {
        let toks = |s: &str| {
            wrap_tokens(s)
                .into_iter()
                .map(|t| (t.sep.to_string(), t.tok.to_string()))
                .collect::<Vec<_>>()
        };
        // Pure Latin == split(' ') parts with single-space seps.
        assert_eq!(
            toks("ab cd"),
            vec![("".into(), "ab".into()), (" ".into(), "cd".into())]
        );
        // Space runs, leading and trailing spaces survive verbatim.
        assert_eq!(
            toks("a  b"),
            vec![
                ("".into(), "a".into()),
                (" ".into(), "".into()),
                (" ".into(), "b".into()),
            ]
        );
        assert_eq!(
            toks(" x"),
            vec![("".into(), "".into()), (" ".into(), "x".into())]
        );
        assert_eq!(
            toks("x "),
            vec![("".into(), "x".into()), (" ".into(), "".into())]
        );
        assert_eq!(toks(""), vec![("".into(), "".into())]);
        // A hyphen inside a word is a break opportunity after it (Visio
        // 2021: `nsa1-` / `ext`); not before a digit, not at the start.
        assert_eq!(
            toks("nsa1-ext x"),
            vec![
                ("".into(), "nsa1-".into()),
                ("".into(), "ext".into()),
                (" ".into(), "x".into()),
            ]
        );
        assert_eq!(toks("2026-11-25"), vec![("".into(), "2026-11-25".into())]);
        assert_eq!(toks("-x"), vec![("".into(), "-x".into())]);
        // No break before `/` (or closing punctuation) even after a
        // space: the token joins its predecessor (UAX #14 LB13).
        assert_eq!(
            toks("nsa1-ext / b"),
            vec![
                ("".into(), "nsa1-".into()),
                ("".into(), "ext /".into()),
                (" ".into(), "b".into()),
            ]
        );
        assert_eq!(toks("a )"), vec![("".into(), "a )".into())]);
        assert_eq!(
            toks("a--b"),
            vec![("".into(), "a--".into()), ("".into(), "b".into())]
        );
        // CJK: every ideograph is its own token, kinsoku-glued
        // punctuation rides its neighbor.
        assert_eq!(
            toks("图表形状"),
            vec![
                ("".into(), "图".into()),
                ("".into(), "表".into()),
                ("".into(), "形".into()),
                ("".into(), "状".into()),
            ]
        );
        assert_eq!(
            toks("画图，上色。"),
            vec![
                ("".into(), "画".into()),
                ("".into(), "图，".into()),
                ("".into(), "上".into()),
                ("".into(), "色。".into()),
            ]
        );
        // Mixed: the Latin word stays one token, the Han run splits,
        // and the word's separator lands on its FIRST sub-token.
        assert_eq!(
            toks("ab 图表 cd"),
            vec![
                ("".into(), "ab".into()),
                (" ".into(), "图".into()),
                ("".into(), "表".into()),
                (" ".into(), "cd".into()),
            ]
        );
        // The ideographic space separates words like an ASCII space.
        assert_eq!(
            toks("图\u{3000}表"),
            vec![("".into(), "图".into()), ("\u{3000}".into(), "表".into())]
        );
        // A no-break space binds ideographs (CodeRabbit on #282).
        assert_eq!(
            toks("图\u{00A0}表"),
            vec![("".into(), "图\u{00A0}表".into())]
        );
        // Dotted ASCII words stay one token (no grant from '.').
        assert_eq!(
            toks("a x.y"),
            vec![("".into(), "a".into()), (" ".into(), "x.y".into())]
        );
        assert_eq!(
            toks("3.14 15"),
            vec![("".into(), "3.14".into()), (" ".into(), "15".into())]
        );
        // Decomposed kana stays one grapheme (カ + U+3099 = ガ), and
        // the COMPLETE sequence breaks before ク like precomposed ガク.
        assert_eq!(
            toks("カ\u{3099}ク"),
            vec![("".into(), "カ\u{3099}".into()), ("".into(), "ク".into())]
        );
        // A LATIN-based combining sequence classifies through its
        // BASE: `A\u{0301}中` wraps exactly like precomposed `Á中`
        // (one token — the base grants no boundary), while the mark
        // itself never splits from the base (CodeRabbit round 5).
        assert_eq!(toks("A\u{0301}中"), vec![("".into(), "A\u{0301}中".into())]);
        assert_eq!(toks("Á中"), vec![("".into(), "Á中".into())]);
    }
}
