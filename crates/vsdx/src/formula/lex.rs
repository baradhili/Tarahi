//! The tokenizer: source text → [`Tok`] tokens, including unit-suffixed
//! numeric literals (their conversion lives in [`unit_factor`]) and the
//! `Sheet.5!Width` cross-sheet ident shape.

/// A lexical token.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Amp,
    LParen,
    RParen,
    Comma,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Multiplier converting a formula literal's unit suffix to internal units
/// (inches for lengths, radians for angles). Unknown suffixes are treated as
/// unit-less: shipped `V` values are already normalized ([MS-VSDX]
/// lengthInternalUnitNumber / angleInternalUnitNumber).
fn unit_factor(suffix: &str) -> f64 {
    const IN_PER_CM: f64 = 0.3937007874015748;
    match suffix.to_ascii_uppercase().as_str() {
        "" | "DL" | "DP" | "IN" | "PZ" => 1.0, // drawn/local, page, inch
        "FT" => 12.0,
        "YD" => 36.0,
        "MI" => 63360.0,
        "MM" => IN_PER_CM / 10.0,
        "CM" => IN_PER_CM,
        "M" => IN_PER_CM * 100.0,
        "KM" => IN_PER_CM * 100000.0,
        "HI" => IN_PER_CM / 10000.0, // himetric = 0.0001 cm
        "PT" => 1.0 / 72.0,
        "PICA" | "PICA_PT" => 1.0 / 6.0,
        "DEG" => std::f64::consts::PI / 180.0,
        "RAD" => 1.0,
        "PER" => 0.01,
        _ => 1.0,
    }
}

/// A leading UTF-8 byte's scalar length (1 for continuation bytes —
/// callers treat those as invalid-continuation input).
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

pub(crate) fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '+' => {
                toks.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                toks.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                toks.push(Tok::Star);
                i += 1;
            }
            '/' => {
                toks.push(Tok::Slash);
                i += 1;
            }
            '^' => {
                toks.push(Tok::Caret);
                i += 1;
            }
            '&' => {
                toks.push(Tok::Amp);
                i += 1;
            }
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            '=' => {
                toks.push(Tok::Eq);
                i += 1;
            }
            '<' => {
                // `<>` is Visio's not-equal; `<=` less-or-equal.
                if i + 1 < b.len() {
                    match b[i + 1] as char {
                        '>' => {
                            toks.push(Tok::Ne);
                            i += 2;
                            continue;
                        }
                        '=' => {
                            toks.push(Tok::Le);
                            i += 2;
                            continue;
                        }
                        _ => {}
                    }
                }
                toks.push(Tok::Lt);
                i += 1;
            }
            '>' => {
                if i + 1 < b.len() && b[i + 1] as char == '=' {
                    toks.push(Tok::Ge);
                    i += 2;
                } else {
                    toks.push(Tok::Gt);
                    i += 1;
                }
            }
            '"' => {
                let mut out = String::new();
                i += 1;
                while i < b.len() && b[i] as char != '"' {
                    if b[i] as char == '&' && i + 2 < b.len() {
                        // minimal entity pass
                        let ent = &s[i + 1..(i + 3).min(s.len())];
                        if ent.starts_with("amp") {
                            out.push('&');
                            i += 4;
                            continue;
                        }
                    }
                    // Copy UTF-8 spans at scalar boundaries: pushing
                    // `b[i] as char` garbles multibyte text — a quoted
                    // Han string became one Latin-1 mojibake char per
                    // BYTE (CodeRabbit PR review).
                    let ch_len = utf8_len(b[i]);
                    let end = (i + ch_len).min(b.len());
                    // The closing-quote check above guarantees b[i]
                    // is not `"`; a partial sequence at the string's
                    // end falls through byte-wise (invalid input).
                    if s.is_char_boundary(end) {
                        out.push_str(&s[i..end]);
                        i = end;
                    } else {
                        out.push(b[i] as char);
                        i += 1;
                    }
                }
                i += 1; // closing quote
                toks.push(Tok::Str(out));
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] as char == '.') {
                    i += 1;
                }
                let mut digits_end = i;
                // Scientific notation: `E`/`e` followed by an optional
                // sign and digits is an exponent, not a unit suffix —
                // shipped masters carry cells like `Height*9.2184E-5`
                // whose misparse as `Height*9.218-5` throws geometry
                // inches off the shape (issue #15).
                if i < b.len() && matches!(b[i] as char, 'E' | 'e') {
                    let mut j = i + 1;
                    if j < b.len() && matches!(b[j] as char, '+' | '-') {
                        j += 1;
                    }
                    if j < b.len() && (b[j] as char).is_ascii_digit() {
                        while j < b.len() && (b[j] as char).is_ascii_digit() {
                            j += 1;
                        }
                        digits_end = j;
                        i = j;
                    }
                }
                // A directly-attached unit suffix (0.24PT, 90DEG) is
                // converted to internal units (inches / radians) per
                // [MS-VSDX] §2.5.7–2.5.8.
                while i < b.len() && (b[i] as char).is_ascii_alphabetic() {
                    i += 1;
                }
                let n: f64 = s[start..digits_end]
                    .parse()
                    .map_err(|_| format!("bad number '{}'", &s[start..digits_end]))?;
                let factor = unit_factor(&s[digits_end..i]);
                toks.push(Tok::Num(n * factor));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < b.len() {
                    let ch = b[i] as char;
                    // `!` keeps `Sheet.5!Width` / `Sheet.5!User.X` refs one
                    // token (Visio has no other use of `!`; not-equal is `<>`).
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '!' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let word = &s[start..i];
                if word == "TRUE" {
                    toks.push(Tok::Num(1.0));
                } else if word == "FALSE" {
                    toks.push(Tok::Num(0.0));
                } else {
                    toks.push(Tok::Ident(word.to_string()));
                }
            }
            other => return Err(format!("unexpected char '{other}'")),
        }
    }
    Ok(toks)
}
