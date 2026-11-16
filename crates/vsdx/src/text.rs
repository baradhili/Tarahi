//! Structured text runs (#267): the `<Text>` element's inline markers
//! as a typed model.
//!
//! Real packages interleave plain character data with formatting and
//! field markers — `<cp IX='n'/>` (character-run pointer), `<pp IX='n'/>`
//! (paragraph), `<tp IX='n'/>` (tabs), and `<fld IX='n'/>` (text FIELD
//! marker, self-closing or value-carrying:
//! `<fld IX='0'>4/17</fld>` — Visio caches the evaluated value as the
//! marker's text content). The writer keeps [`Sheet::text_raw`] as the
//! byte-exact path, so this module is a READ view: [`text_pieces`]
//! splits the raw inner XML into ordered pieces — plain text runs with
//! the FIELD marker (and its cached content) that produced them — for
//! the renderer's field substitution and the editor's caret math.
//! Concatenating the pieces' `text` reproduces the markup-stripped
//! plain text byte for byte.

/// One piece of a `<Text>` element: a plain character run and the
/// field marker that governs it. `fld` names the Field-section row
/// (the `IX`) whose evaluated value renders in place of the run's
/// text. `cached` is the marker's own content when Visio saved one
/// (the last-evaluated value — the renderer's fallback when the
/// `Value` formula no longer evaluates). `cp` records the governing
/// character-run pointer (the editor uses it; run-level formatting
/// itself stays first-row granularity in the renderer, #230). A
/// self-closing marker yields an empty-text piece so its position
/// survives for the editor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextPiece {
    pub text: String,
    /// The `<fld IX>` this text follows, if the run is field content.
    pub fld: Option<usize>,
    /// The marker's own cached text content (self-closing markers
    /// carry none).
    pub cached: Option<String>,
    /// The `<cp IX>` governing this run's character formatting.
    pub cp: Option<usize>,
}

/// Split a raw `<Text>` inner XML into ordered pieces. Markers
/// (`cp`/`pp`/`tp`/`fld`) occupy no character positions; `pp`/`tp`
/// strip entirely. A value-carrying `<fld IX='n'>cached</fld>` becomes
/// ONE piece whose text (and `cached`) is the marker's content; a
/// self-closing `<fld IX='n'/>` becomes an empty piece carrying the
/// row index — the next run is NOT absorbed into it (Visio renders the
/// field's value exactly at the marker, nothing after it).
pub fn text_pieces(raw: &str) -> Vec<TextPiece> {
    use crate::opc::unescape;
use quick_xml::events::Event;
    let mut pieces: Vec<TextPiece> = Vec::new();
    let mut run = TextPiece::default();
    let mut open_fld: Option<usize> = None;
    let mut fld_content = String::new();
    let attr_ix = |e: &quick_xml::events::BytesStart<'_>| {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == "IX")
            .and_then(|a| a.value.as_ref().parse::<usize>().ok())
    };
    let flush = |pieces: &mut Vec<TextPiece>, run: &mut TextPiece| {
        if !run.text.is_empty() || run.fld.is_some() || run.cp.is_some() {
            pieces.push(std::mem::take(run));
        }
    };
    let mut r = quick_xml::Reader::from_str(raw);
    let mut buf = Vec::new();
    loop {
        let ev = r.read_event_into(&mut buf);
        match ev {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "fld" => {
                    flush(&mut pieces, &mut run);
                    open_fld = attr_ix(&e);
                    fld_content.clear();
                }
                "cp" => {
                    flush(&mut pieces, &mut run);
                    run.cp = attr_ix(&e);
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                // Self-closing field: an empty piece at the marker's
                // position — its evaluated value renders there.
                "fld" => {
                    flush(&mut pieces, &mut run);
                    pieces.push(TextPiece {
                        text: String::new(),
                        fld: attr_ix(&e),
                        cached: None,
                        cp: run.cp.take(),
                    });
                }
                "cp" => {
                    // A cp BETWEEN runs governs the next one; a cp with
                    // a pending run stays that run's (no re-split).
                    flush(&mut pieces, &mut run);
                    run.cp = attr_ix(&e);
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.name().as_ref() == "fld" => {
                // Value-carrying: the content IS the visible run.
                let cached = std::mem::take(&mut fld_content);
                pieces.push(TextPiece {
                    text: cached.clone(),
                    fld: open_fld.take(),
                    cached: Some(cached),
                    cp: run.cp.take(),
                });
            }
            Ok(Event::Text(t)) => {
                let text = t.as_ref().to_string();
                if open_fld.is_some() {
                    fld_content.push_str(&text);
                } else {
                    run.text.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(r)) => {
                let mut ent = String::from("&");
                ent.push_str(r.as_ref());
                ent.push(';');
                let text = unescape(ent.as_bytes());
                if open_fld.is_some() {
                    fld_content.push_str(&text);
                } else {
                    run.text.push_str(&text);
                }
            }
            Ok(Event::CData(t)) => {
                let text = t.as_ref().to_string();
                if open_fld.is_some() {
                    fld_content.push_str(&text);
                } else {
                    run.text.push_str(&text);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    flush(&mut pieces, &mut run);
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_piece() {
        let p = text_pieces("hello world");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "hello world");
        assert_eq!(p[0].fld, None);
        assert_eq!(p[0].cached, None);
    }

    #[test]
    fn self_closing_fld_is_an_empty_piece_at_its_position() {
        // The corpus shape: <cp IX='0'/> newline <fld IX='0'/> — the
        // newline is real character data between the markers; the
        // field's evaluated value renders at the marker.
        let p = text_pieces("<cp IX='0'/>\n<fld IX='0'/>");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].text, "\n");
        assert_eq!(p[0].fld, None);
        assert_eq!(p[0].cp, Some(0));
        assert_eq!(p[1].text, "");
        assert_eq!(p[1].fld, Some(0));
        assert_eq!(p[1].cached, None);
    }

    #[test]
    fn value_carrying_fld_keeps_the_cached_content() {
        let p = text_pieces("Cable <fld IX='0'>4/17</fld> cross");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].text, "Cable ");
        assert_eq!(p[0].fld, None);
        assert_eq!(p[1].text, "4/17");
        assert_eq!(p[1].fld, Some(0));
        assert_eq!(p[1].cached.as_deref(), Some("4/17"));
        assert_eq!(p[2].text, " cross");
    }

    #[test]
    fn entities_decode_inside_runs() {
        let p = text_pieces("a &amp; b");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "a & b");
    }

    #[test]
    fn two_fields_and_text_interleaved() {
        let p = text_pieces("<fld IX='0'>A</fld>-<fld IX='1'>B</fld>");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].fld, Some(0));
        assert_eq!(p[1].text, "-");
        assert_eq!(p[1].fld, None);
        assert_eq!(p[2].fld, Some(1));
    }

    /// The pieces' text concatenation must reproduce the markup-
    /// stripped plain text (`parse::plain_text_of`) byte for byte —
    /// the write path keeps `text_raw`, and every consumer that reads
    /// `sheet.text` must see the same characters whether it walks the
    /// pieces or the plain form.
    #[test]
    fn piece_concatenation_matches_plain_text() {
        let raw = "Head <cp IX='1'/>mid &amp; <fld IX='0'>4/17</fld>\n<fld IX='1'/>tail";
        let plain: String = text_pieces(raw).iter().map(|p| p.text.as_str()).collect();
        assert_eq!(plain, "Head mid & 4/17\ntail");
    }
}
