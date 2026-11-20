//! Typed `<Text>` run model (GitHub #267).
//!
//! A shape's `<Text>` element interleaves character data with inline
//! markers — `<cp IX='n'/>` (character formatting), `<pp IX='n'/>`
//! (paragraph), `<tp IX='n'/>` (tabs), and the FIELD marker
//! `<fld IX='n'>cached</fld>` — where `IX` selects the row of the
//! `Character`/`Paragraph`/`Tabs`/`Field` section the run formats
//! with. `Sheet::text_raw` stays the byte-exact WRITE path; this
//! module is the derived typed VIEW the renderer and the field
//! engine read.
//!
//! Corpus shapes (measured 2026-11-13 over the shipped Visio 2016
//! stencil library, 679 packages — 180 carry `<fld>`): the field
//! marker comes both self-closing (`<fld IX='0'/>`, empty cached
//! value — the common case) and value-carrying
//! (`<fld IX='0'>4/17</fld>`, Visio caches the evaluated display
//! string as the marker's text content).
//!
//! The contract that keeps this a view and not a second write path:
//! concatenating every piece's [`Piece::plain`] reproduces the
//! markup-stripped plain text byte for byte.

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;

/// One run of a `<Text>` element, in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// Entity-resolved character data.
    Text(String),
    /// `<fld IX='n'>content</fld>` — a field reference. `content` is
    /// the cached display string (`None` for the self-closing form,
    /// which contributes nothing to the plain text).
    Fld { ix: i64, content: Option<String> },
    /// `<cp IX='n'/>` — character-formatting switch.
    Cp { ix: i64 },
    /// `<pp IX='n'/>` — paragraph-formatting switch.
    Pp { ix: i64 },
    /// `<tp IX='n'/>` — tab-stop switch.
    Tp { ix: i64 },
}

impl Piece {
    /// The character data this piece contributes to the plain text.
    pub fn plain(&self) -> &str {
        match self {
            Piece::Text(t) => t,
            Piece::Fld {
                content: Some(c), ..
            } => c,
            _ => "",
        }
    }
}

/// Parse a `<Text>` element's raw inner XML into typed pieces.
///
/// Unknown elements are skipped (their inner text still accumulates,
/// matching the plain-text extraction), and malformed markup degrades
/// to plain text rather than failing — rendering must never lose a
/// shape's words over a bad `IX`.
pub fn text_pieces(raw: &str) -> Vec<Piece> {
    let mut r = quick_xml::Reader::from_str(raw);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut buf = Vec::new();
    // Depth INSIDE an open value-carrying `<fld>`: 0 = not inside
    // one, 1 = directly in the marker, >1 = inside a nested element
    // (not corpus-attested; handled so words survive anyway — its
    // TEXT joins the cached display like `plain_text_of`, and each
    // nested start balances its own end so `</fld>` closes the
    // MARKER, never a nested tag) (CodeRabbit PR review).
    let mut inside_fld = 0usize;
    // The open element stack while inside a marker (its names), so an
    // End only pops the depth when it matches the innermost open tag.
    let mut open_stack: Vec<String> = Vec::new();
    loop {
        buf.clear();
        match r.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => {
                let s = t.as_ref().to_string();
                push_chars(&mut pieces, &s);
            }
            Ok(Event::CData(t)) => {
                let s = t.as_ref().to_string();
                push_chars(&mut pieces, &s);
            }
            Ok(Event::GeneralRef(g)) => {
                // Re-form the entity and resolve it exactly the way
                // the plain-text extraction does.
                let mut ent = String::from("&");
                ent.push_str(g.as_ref());
                ent.push(';');
                let s = crate::opc::unescape(ent.as_bytes());
                push_chars(&mut pieces, &s);
            }
            Ok(Event::Start(e)) => {
                let name = tag_name(e.name());
                if let Some(ix) = marker_ix_opt(&name, &e) {
                    match name.as_str() {
                        "fld" => {
                            if inside_fld == 0 {
                                pieces.push(Piece::Fld {
                                    ix,
                                    content: Some(String::new()),
                                });
                                open_stack.push(name.clone());
                            }
                            inside_fld += 1;
                        }
                        "cp" if inside_fld == 0 => pieces.push(Piece::Cp { ix }),
                        "pp" if inside_fld == 0 => pieces.push(Piece::Pp { ix }),
                        "tp" if inside_fld == 0 => pieces.push(Piece::Tp { ix }),
                        _ => {}
                    }
                } else if inside_fld > 0 {
                    // Nested element inside a value-carrying marker:
                    // its payload is part of the cached display.
                    inside_fld += 1;
                    open_stack.push(name.clone());
                }
            }
            Ok(Event::Empty(e)) => {
                // Self-closing elements contribute no characters and
                // no depth; markers only count at depth 0.
                if inside_fld == 0 {
                    if let Some(ix) = marker_ix_opt(&tag_name(e.name()), &e) {
                        match tag_name(e.name()).as_str() {
                            "fld" => pieces.push(Piece::Fld { ix, content: None }),
                            "cp" => pieces.push(Piece::Cp { ix }),
                            "pp" => pieces.push(Piece::Pp { ix }),
                            "tp" => pieces.push(Piece::Tp { ix }),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = tag_name(e.name());
                if inside_fld > 0 && open_stack.last().is_some_and(|n| n == &name) {
                    open_stack.pop();
                    inside_fld -= 1;
                    if inside_fld == 0 {
                        // Close the marker's content slot: subsequent
                        // characters are plain text again, not cached
                        // display (an empty Text piece is the merge
                        // target `push_chars` appends to).
                        pieces.push(Piece::Text(String::new()));
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    // An unterminated `<fld>` (raw cut short) keeps whatever content
    // accumulated — degrade to text, never lose words.
    pieces
}

fn tag_name(name: QName<'_>) -> String {
    name.as_ref().to_string()
}

/// Push character data onto the piece list, merging into a trailing
/// text piece (adjacent runs stay one piece).
fn push_chars(pieces: &mut Vec<Piece>, s: &str) {
    if s.is_empty() {
        return;
    }
    // Inside an open `<fld>` the characters are its cached content;
    // outside they are plain text.
    match pieces.last_mut() {
        Some(Piece::Fld {
            content: Some(c), ..
        }) => c.push_str(s),
        Some(Piece::Text(t)) => t.push_str(s),
        _ => pieces.push(Piece::Text(s.to_string())),
    }
}

/// The `IX` attribute of a marker start/empty tag (0 when absent),
/// or `None` when the element is not one of the four markers.
fn marker_ix_opt(name: &str, e: &BytesStart) -> Option<i64> {
    if !matches!(name, "fld" | "cp" | "pp" | "tp") {
        return None;
    }
    Some(ix_of(e))
}

fn ix_of(e: &BytesStart) -> i64 {
    let mut ix = 0i64;
    for a in e.attributes().flatten() {
        if a.key.as_ref() == "IX" {
            ix = a.value.as_ref().trim().parse().unwrap_or(0);
        }
    }
    ix
}

/// The field markers of a raw `<Text>`, in order: `(IX, cached
/// display string)` — `None` for the self-closing form.
pub fn flds(raw: &str) -> Vec<(i64, Option<String>)> {
    text_pieces(raw)
        .into_iter()
        .filter_map(|p| match p {
            Piece::Fld { ix, content } => Some((ix, content)),
            _ => None,
        })
        .collect()
}

/// Map a plain-text CHARACTER offset to the byte offset in the raw
/// XML where the character at that plain position sits (text-editor
/// caret mapping for field insertion, #267). The plain projection
/// counts character data AND `<fld>` cached content, skips tags, and
/// resolves entities — exactly [`text_pieces`] concatenation — so
/// `plain_chars` is an offset into that projection. Offsets beyond
/// the end clamp to the raw length.
/// The byte offset just past the tag starting at `i`, honoring `>`
/// inside quoted attribute values.
fn tag_end(raw: &str, i: usize) -> usize {
    let bytes = raw.as_bytes();
    let mut j = i + 1;
    let mut quote: Option<u8> = None;
    while j < bytes.len() {
        let b = bytes[j];
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'>' => return j + 1,
                _ => {}
            },
        }
        j += 1;
    }
    raw.len()
}

pub fn plain_to_raw_offset(raw: &str, plain_chars: usize) -> usize {
    let mut count = 0usize;
    let mut chars = raw.char_indices().peekable();
    // The innermost open value-carrying `<fld>`: a target that lands
    // strictly INSIDE its cached content snaps to the marker's
    // opening tag (inserting there would nest markers — CodeRabbit
    // round 2).
    let mut open_fld_at: Option<usize> = None;
    while let Some((i, c)) = chars.next() {
        match c {
            // A CDATA section is not a tag: its CONTENT is plain
            // text (`text_pieces` counts it — quick_xml's CData
            // event), the wrappers are not. A target that lands
            // strictly INSIDE the content snaps past the `]]>` — a
            // `<fld>` marker inserted inside CDATA is literal
            // markup, never a field (Greptile round 6).
            '<' if raw[i..].starts_with("<![CDATA[") => {
                const OPEN: usize = 9; // "<![CDATA[".len()
                let content_start = i + OPEN;
                let (end, after) = match raw[content_start..].find("]]>") {
                    Some(k) => (content_start + k, content_start + k + 3),
                    None => (raw.len(), raw.len()),
                };
                let content_chars = raw[content_start..end].chars().count();
                if count >= plain_chars {
                    // The target sits at/before the section: insert
                    // ahead of it — unless the section lives INSIDE
                    // a field's cached content, where a marker would
                    // disappear into the outer display (snap to its
                    // opening tag, the `_` arm's rule — Greptile
                    // round 7).
                    return open_fld_at.unwrap_or(i);
                }
                if count + content_chars > plain_chars {
                    return open_fld_at.unwrap_or(after);
                }
                count += content_chars;
                while let Some(&(j, _)) = chars.peek() {
                    if j >= after {
                        break;
                    }
                    chars.next();
                }
            }
            '<' => {
                // Skip the tag wholesale: tags contribute no plain
                // characters. An unterminated tag ends the scan. A
                // `>` inside a QUOTED attribute does not close the
                // tag (`<x title=">">`; CodeRabbit PR review).
                let close = tag_end(raw, i);
                let self_closing = raw[..close].ends_with("/>");
                let closing_fld = raw[i..].starts_with("</fld");
                let opening_fld =
                    !closing_fld && !self_closing && raw[i..close].starts_with("<fld");
                if count >= plain_chars {
                    // A caret at the END of a field's cached display
                    // maps to the tag ahead — inserting there would
                    // nest inside the marker; land AFTER the closing
                    // tag instead. Any OTHER tag reached with the
                    // caret inside a display snaps to the marker's
                    // opening (the `_` arm's rule — a marker written
                    // inside the display disappears into it,
                    // Greptile round 7).
                    if closing_fld {
                        return close;
                    }
                    return open_fld_at.unwrap_or(i);
                }
                if opening_fld {
                    open_fld_at = Some(i);
                } else if closing_fld {
                    open_fld_at = None;
                }
                // Consume through the tag.
                while let Some(&(j, _)) = chars.peek() {
                    if j >= close {
                        break;
                    }
                    chars.next();
                }
            }
            '&' => {
                if count >= plain_chars {
                    return open_fld_at.unwrap_or(i);
                }
                // The entity resolves to its unescaped characters.
                let semi = raw[i..].find(';').map_or(raw.len(), |k| i + k + 1);
                let bytes = raw.as_bytes();
                let n = crate::opc::unescape(&bytes[i..semi]).chars().count();
                count += n;
                while let Some(&(j, _)) = chars.peek() {
                    if j >= semi {
                        break;
                    }
                    chars.next();
                }
            }
            _ => {
                if count >= plain_chars {
                    // Inside a field's cached content: snap to the
                    // marker's opening tag.
                    return open_fld_at.unwrap_or(i);
                }
                count += 1;
            }
        }
    }
    raw.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Piece concatenation equals the parser's plain-text extraction
    /// byte for byte (the module contract).
    fn plain_of_pieces(raw: &str) -> String {
        text_pieces(raw).iter().map(|p| p.plain()).collect()
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(text_pieces("hello"), vec![Piece::Text("hello".into())]);
        assert_eq!(plain_of_pieces("hello"), "hello");
    }

    #[test]
    fn entities_resolve_like_the_parser() {
        let raw = "a &amp; b &lt;tag&gt; &#65;";
        assert_eq!(
            plain_of_pieces(raw),
            crate::parse::plain_text_of(raw),
            "pieces and the plain extractor must agree"
        );
    }

    #[test]
    fn self_closing_fld_contributes_nothing() {
        let raw = "before<fld IX='0'/>after";
        assert_eq!(
            text_pieces(raw),
            vec![
                Piece::Text("before".into()),
                Piece::Fld {
                    ix: 0,
                    content: None
                },
                Piece::Text("after".into()),
            ]
        );
        assert_eq!(plain_of_pieces(raw), "beforeafter");
    }

    #[test]
    fn value_carrying_fld_keeps_its_cached_display() {
        let raw = "due <fld IX='3'>4/17</fld>";
        assert_eq!(
            text_pieces(raw),
            vec![
                Piece::Text("due ".into()),
                Piece::Fld {
                    ix: 3,
                    content: Some("4/17".into())
                },
                Piece::Text(String::new()),
            ]
        );
        assert_eq!(plain_of_pieces(raw), "due 4/17");
    }

    #[test]
    fn all_field_block_with_empty_plain_text() {
        // The corpus's whole-value shapes: plain text is empty but
        // the marker still carries the display string.
        let raw = "<fld IX='0'>2015</fld>";
        assert_eq!(plain_of_pieces(raw), "2015");
        assert_eq!(flds(raw), vec![(0, Some("2015".into()))]);
    }

    #[test]
    fn text_after_the_closing_tag_is_plain_not_cached_display() {
        let raw = "Due <fld IX='0'>4/17</fld> today";
        assert_eq!(
            flds(raw),
            vec![(0, Some("4/17".into()))],
            "the cached display stops at the closing tag"
        );
        assert_eq!(plain_of_pieces(raw), "Due 4/17 today");
    }

    #[test]
    fn caret_at_display_end_lands_after_the_closing_tag() {
        let raw = "<fld IX='0'>4/17</fld> today";
        // Plain offset 4 (after "4/17") must map past `</fld>`.
        let at = plain_to_raw_offset(raw, 4);
        assert!(
            raw[..at].ends_with("</fld>"),
            "insertion after the closing tag, got byte {at} of {raw:?}"
        );
    }

    #[test]
    fn caret_inside_a_cached_display_snaps_to_the_marker() {
        // Between '/' and '1' (plain offset 2): inserting there would
        // nest markers — the snap goes to the opening tag (CodeRabbit
        // round 2).
        let raw = "x<fld IX='0'>4/17</fld>y";
        assert_eq!(plain_to_raw_offset(raw, 3), 1);
        // At the display's very start (offset 1): already at the tag.
        assert_eq!(plain_to_raw_offset(raw, 1), 1);
    }

    #[test]
    fn formatting_markers_are_typed() {
        let raw = "<pp IX='0'/>line<cp IX='1'/>x<tp IX='0'/>\ttab";
        let pieces = text_pieces(raw);
        assert!(pieces.contains(&Piece::Pp { ix: 0 }));
        assert!(pieces.contains(&Piece::Cp { ix: 1 }));
        assert!(pieces.contains(&Piece::Tp { ix: 0 }));
        assert_eq!(plain_of_pieces(raw), "linex\ttab");
    }

    #[test]
    fn missing_ix_defaults_to_zero() {
        assert_eq!(flds("<fld/>"), vec![(0, None)]);
    }

    #[test]
    fn cdata_and_refs_inside_fld_content() {
        let raw = "<fld IX='0'>a &amp; b</fld>";
        assert_eq!(flds(raw), vec![(0, Some("a & b".into()))]);
    }

    #[test]
    fn unterminated_fld_degrades_to_text() {
        let raw = "x<fld IX='0'>partial";
        // Never lose words: the content stays visible.
        assert_eq!(plain_of_pieces(raw), "xpartial");
    }

    #[test]
    fn nested_elements_inside_a_field_join_its_display() {
        // The nested <b>'s payload is cached content (CodeRabbit PR
        // review) and the field closes at </fld>, not at </b>.
        let raw = "x<fld IX='2'>a<b>b</b>c</fld>y";
        assert_eq!(flds(raw), vec![(2, Some("abc".into()))]);
        assert_eq!(plain_of_pieces(raw), "xabcy");
        // Trailing text is NOT part of the display.
        let raw2 = "x<fld IX='2'>a<b></b>c</fld>y";
        assert_eq!(flds(raw2), vec![(2, Some("ac".into()))]);
    }

    #[test]
    fn quoted_attribute_gt_does_not_close_the_tag() {
        let raw = "a<x title=\">\">b</x>c";
        // Plain offset 2 (after `ab`) lands at the CLOSING tag — a
        // naive `find('>')` stops at the quoted `>`, counts the
        // attribute tail as plain text, and shifts every later
        // mapping into the markup.
        let at2 = plain_to_raw_offset(raw, 2);
        assert!(raw[at2..].starts_with("</x>"), "got {at2}");
    }

    #[test]
    fn plain_to_raw_offset_maps_the_caret() {
        let raw = "ab<cp IX='0'/>cd<fld IX='0'>4/17</fld>ef";
        // Plain projection: "abcd4/17ef".
        assert_eq!(plain_to_raw_offset(raw, 0), 0);
        assert_eq!(plain_to_raw_offset(raw, 2), 2);
        // At the tag boundary: offset 3 (plain "d") is past the tag.
        let at3 = plain_to_raw_offset(raw, 3);
        assert_eq!(&raw[at3..at3 + 1], "d");
        // Inside the marker's cached content (plain "abcd4/17ef":
        // offsets 5 and 6 fall between the display's characters)
        // snaps to the marker's opening tag — never into the content.
        assert_eq!(plain_to_raw_offset(raw, 5), raw.find("<fld").unwrap());
        assert_eq!(plain_to_raw_offset(raw, 6), raw.find("<fld").unwrap());
        // Beyond the end clamps.
        assert_eq!(plain_to_raw_offset(raw, 999), raw.len());
        // Entities count as their resolved characters ("a & b":
        // offset 4 is the 'b').
        let ent = "a &amp; b";
        assert_eq!(&ent[plain_to_raw_offset(ent, 4)..], "b");
    }

    /// CDATA content counts as plain text (the piece model does —
    /// `adjacent_text_runs_merge`), the wrappers do not, and a caret
    /// landing strictly INSIDE a section maps past the `]]>` — a
    /// marker inserted inside CDATA would be literal markup
    /// (Greptile round 6: `<![CDATA[A>B]]>C` used to end its "tag"
    /// at the inner `>`).
    #[test]
    fn plain_to_raw_offset_counts_cdata_content_as_text() {
        // Plain projection: "abc".
        let raw = "a<![CDATA[b]]>c";
        assert_eq!(plain_to_raw_offset(raw, 0), 0);
        assert_eq!(plain_to_raw_offset(raw, 1), 1);
        // Caret 2 (the b|c boundary — inside AND content-end for a
        // one-character section) maps past the `]]>`.
        assert_eq!(&raw[plain_to_raw_offset(raw, 2)..], "c");
        // Beyond the end clamps.
        assert_eq!(plain_to_raw_offset(raw, 3), raw.len());
        // Greptile's repro: a `>` inside CDATA must not end a "tag".
        let gt = "<![CDATA[A>B]]>C";
        // Plain projection: "A>BC"; caret 1 (inside) and caret 3
        // (content end) both map past the section, at "C".
        assert_eq!(&gt[plain_to_raw_offset(gt, 1)..], "C");
        assert_eq!(&gt[plain_to_raw_offset(gt, 3)..], "C");
        // A marker still lands safely around a CDATA section.
        let with_fld = "a<![CDATA[b]]><fld IX='0'>x</fld>d";
        assert_eq!(
            plain_to_raw_offset(with_fld, 2),
            with_fld.find("<fld").unwrap()
        );
    }

    /// A CDATA section INSIDE a field's cached display snaps to the
    /// marker's opening tag — a marker written inside the display
    /// (where the CDATA branch's after-`]]>` offset lands) merges
    /// into the outer field and the insertion disappears (Greptile
    /// round 7: `<fld IX='0'><![CDATA[ABC]]></fld>D`).
    #[test]
    fn cdata_inside_a_field_display_snaps_to_the_marker() {
        let raw = "<fld IX='0'><![CDATA[ABC]]></fld>D";
        // Plain projection: "ABCD"; offsets 1 and 2 sit inside the
        // CDATA content, i.e. inside the display.
        let fld_at = raw.find("<fld").unwrap();
        assert_eq!(plain_to_raw_offset(raw, 1), fld_at);
        assert_eq!(plain_to_raw_offset(raw, 2), fld_at);
        // Offset 3 == content end (still inside the display): the
        // `<` of `</fld>` with count >= plain lands PAST the marker,
        // at the "D".
        assert_eq!(&raw[plain_to_raw_offset(raw, 3)..], "D");
        // Offset 4 (past the end) clamps.
        assert_eq!(plain_to_raw_offset(raw, 4), raw.len());
    }

    #[test]
    fn adjacent_text_runs_merge() {
        assert_eq!(
            text_pieces("a<![CDATA[b]]>c&#100;"),
            vec![Piece::Text("abcd".into())]
        );
    }
}
