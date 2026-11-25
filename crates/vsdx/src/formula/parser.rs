//! The recursive-descent parser: precedence ladder, atom resolution
//! against the environments, and the `call` dispatch over the supported
//! function subset.

use super::color::{
    color_channels, hsl_components, hsl_to_channels, pack_color, rgb_from_channels,
};
use super::datetime::days_from_civil;
use super::env::{Env, FieldCtx, StrEnv, TextMeasure};
use super::lex::Tok;
use super::numeric::{convert_units, decimal_round, multiple_round, num_bin, okf};
use super::value::Value;

pub(crate) struct Parser<'a> {
    pub(crate) toks: &'a [Tok],
    pub(crate) pos: usize,
    pub(crate) env: &'a Env,
    pub(crate) strs: &'a StrEnv,
    pub(crate) fctx: Option<&'a FieldCtx>,
    pub(crate) measure: &'a dyn TextMeasure,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    pub(crate) fn expr(&mut self) -> Result<Value, String> {
        self.comparison()
    }

    /// Comparisons are Visio's lowest-precedence operators and yield
    /// 1/0 (ShapeSheet booleans are numbers).
    fn comparison(&mut self) -> Result<Value, String> {
        let mut lhs = self.concat()?;
        loop {
            let which = match self.peek() {
                Some(Tok::Eq) => 0,
                Some(Tok::Ne) => 1,
                Some(Tok::Lt) => 2,
                Some(Tok::Le) => 3,
                Some(Tok::Gt) => 4,
                Some(Tok::Ge) => 5,
                _ => return Ok(lhs),
            };
            self.bump();
            let rhs = self.concat()?;
            lhs = match (lhs.as_f64(), rhs.as_f64()) {
                (Some(a), Some(b)) => {
                    let r = match which {
                        0 => a == b,
                        1 => a != b,
                        2 => a < b,
                        3 => a <= b,
                        4 => a > b,
                        _ => a >= b,
                    };
                    Value::Num(f64::from(r as i32))
                }
                // Non-numeric operands: only (in)equality is meaningful.
                _ => match which {
                    0 => Value::Num(f64::from(
                        (lhs.clone().coerce_str() == rhs.clone().coerce_str()) as i32,
                    )),
                    1 => Value::Num(f64::from(
                        (lhs.clone().coerce_str() != rhs.clone().coerce_str()) as i32,
                    )),
                    _ => return Err("ordering comparison on strings".into()),
                },
            };
        }
    }

    fn concat(&mut self) -> Result<Value, String> {
        let mut lhs = self.additive()?;
        while matches!(self.peek(), Some(Tok::Amp)) {
            self.bump();
            let rhs = self.additive()?;
            lhs = Value::Str(format!("{}{}", lhs.clone().coerce_str(), rhs.coerce_str()));
        }
        Ok(lhs)
    }

    fn additive(&mut self) -> Result<Value, String> {
        let mut lhs = self.multiplicative()?;
        loop {
            match self.peek() {
                Some(Tok::Plus) => {
                    self.bump();
                    let rhs = self.multiplicative()?;
                    lhs = num_bin(lhs, rhs, |a, b| a + b)?;
                }
                Some(Tok::Minus) => {
                    self.bump();
                    let rhs = self.multiplicative()?;
                    lhs = num_bin(lhs, rhs, |a, b| a - b)?;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn multiplicative(&mut self) -> Result<Value, String> {
        let mut lhs = self.power()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.bump();
                    let rhs = self.power()?;
                    lhs = num_bin(lhs, rhs, |a, b| a * b)?;
                }
                Some(Tok::Slash) => {
                    self.bump();
                    let rhs = self.power()?;
                    lhs = num_bin(lhs, rhs, |a, b| a / b)?;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn power(&mut self) -> Result<Value, String> {
        let base = self.unary()?;
        if matches!(self.peek(), Some(Tok::Caret)) {
            self.bump();
            let exp = self.power()?;
            return num_bin(base, exp, |a, b| a.powf(b));
        }
        Ok(base)
    }

    fn unary(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.bump();
                let v = self.unary()?;
                Ok(Value::Num(-v.as_f64().ok_or("negating string")?))
            }
            Some(Tok::Plus) => {
                self.bump();
                self.unary()
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Value, String> {
        match self.bump() {
            Some(Tok::Num(n)) => Ok(Value::Num(n)),
            Some(Tok::Str(s)) => Ok(Value::Str(s)),
            Some(Tok::Ident(name)) => {
                if matches!(self.peek(), Some(Tok::LParen)) {
                    return self.call(&name);
                }
                // `TheText` is the sheet's composed text — a STRING,
                // so it only resolves under a measurement bridge
                // (#147); plain `eval` leaves it (and the text
                // functions taking it) to the cached-V fallback.
                if name == "TheText" {
                    return self
                        .measure
                        .the_text()
                        .map(Value::Str)
                        .ok_or_else(|| "TheText: no text metrics bridge".to_string());
                }
                // Unit-suffixed idents like `0.5DL` arrive as Num + Ident;
                // bare idents resolve numeric-first, then the string
                // environment a Field's `User.*`/`Prop.*` refs live in
                // (#267).
                if let Some(v) = self.env.get(&name) {
                    return Ok(Value::Num(*v));
                }
                if let Some(s) = self.strs.get(&name) {
                    return Ok(Value::Str(s.to_string()));
                }
                Err(format!("unknown cell '{name}'"))
            }
            Some(Tok::LParen) => {
                let v = self.expr()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(v),
                    _ => Err("expected ')'".into()),
                }
            }
            other => Err(format!("unexpected token {other:?}")),
        }
    }

    /// A numeric single-argument helper the breadth arms dispatch to.
    fn call_1(
        &self,
        name: &str,
        args: &[Value],
        f: impl Fn(f64) -> Result<f64, String>,
    ) -> Result<Value, String> {
        if args.len() != 1 {
            return Err(format!("{name} takes one argument"));
        }
        f(args[0]
            .as_f64()
            .ok_or_else(|| format!("{name}: argument not numeric"))?)
        .map(Value::Num)
    }

    /// The function subset master XForms use (issue #15) plus the
    /// #122/FS-033 breadth slice. `GUARD(x)` / `THEMEGUARD(x)` protect
    /// an expression and evaluate to it — THEMEGUARD's guard marks the
    /// cell as theme-protected (a theme application must not overwrite
    /// it); this evaluator applies no themes, so the identity IS the
    /// guarded evaluation. `IF` / `OR` / `AND` / `NOT` are the control
    /// flow behind layout switches like `User.IconY`. Unknown functions
    /// error so callers fall back to the cached `V`.
    fn call(&mut self, name: &str) -> Result<Value, String> {
        self.bump(); // '('
        let mut args = Vec::new();
        if !matches!(self.peek(), Some(Tok::RParen)) {
            loop {
                // An empty slot (`,,` or a trailing `,`) is Visio's
                // omitted argument — a value the callee may read as
                // absent (`FORMATEX(v,pic,,,1033)`).
                if matches!(self.peek(), Some(Tok::Comma) | Some(Tok::RParen)) {
                    args.push(Value::Omitted);
                } else {
                    args.push(self.expr()?);
                }
                match self.bump() {
                    Some(Tok::Comma) => continue,
                    Some(Tok::RParen) => break,
                    _ => return Err(format!("expected ',' or ')' in {name}(...)")),
                }
            }
        } else {
            self.bump(); // ')'
        }
        let arg = |args: &[Value], i: usize| -> Result<f64, String> {
            args.get(i)
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("{name}: arg {} not numeric", i + 1))
        };
        let upper_name = name.to_ascii_uppercase();
        match upper_name.as_str() {
            "GUARD" | "THEMEGUARD" => match args.len() {
                1 => Ok(args.pop().unwrap()),
                _ => Err(format!("{name} takes one argument")),
            },
            "IF" => {
                if args.len() != 3 {
                    return Err("IF takes three arguments".into());
                }
                // IF(cond, then, else): branches are already evaluated
                // (this evaluator has no AST); pick by the condition.
                let cond = arg(&args, 0)?;
                Ok(if cond != 0.0 {
                    args.into_iter().nth(1).unwrap()
                } else {
                    args.into_iter().nth(2).unwrap()
                })
            }
            "OR" => {
                if args.is_empty() {
                    return Err("OR needs arguments".into());
                }
                let r: bool = (0..args.len())
                    .try_fold(false, |acc, i| -> Result<bool, String> {
                        Ok(acc || arg(&args, i)? != 0.0)
                    })?;
                Ok(Value::Num(f64::from(r as i32)))
            }
            "AND" => {
                if args.is_empty() {
                    return Err("AND needs arguments".into());
                }
                let r: bool = (0..args.len()).try_fold(true, |acc, i| -> Result<bool, String> {
                    Ok(acc && arg(&args, i)? != 0.0)
                })?;
                Ok(Value::Num(f64::from(r as i32)))
            }
            "NOT" => {
                if args.len() != 1 {
                    return Err("NOT takes one argument".into());
                }
                Ok(Value::Num(f64::from((arg(&args, 0)? == 0.0) as i32)))
            }
            // MAX/MIN: the corpus's text-aware blocks compose them
            // around the text functions (`MAX(Width,TEXTWIDTH(TheText))`,
            // GDT's `MAX(3*Char.Size*Scratch.B1,TEXTWIDTH(TheText))`).
            // `upper` keeps the selection case-insensitive like every
            // other arm (CodeRabbit: comparing `name` made `max(...)`
            // silently take the MIN branch).
            "MAX" | "MIN" => {
                if args.is_empty() {
                    return Err(format!("{name} needs arguments"));
                }
                let is_max = upper_name == "MAX";
                let mut best = arg(&args, 0)?;
                for i in 1..args.len() {
                    let v = arg(&args, i)?;
                    let take = if is_max { v > best } else { v < best };
                    if take {
                        best = v;
                    }
                }
                Ok(Value::Num(best))
            }
            // SQRT: the 1D chord masters derive their frame width from
            // the endpoints through it (`Width=SQRT((EndX-BeginX)^2+
            // (EndY-BeginY)^2)`, the Line-arc corpus master); a
            // negative argument errors so callers fall back to the
            // cached V.
            "SQRT" => {
                if args.len() != 1 {
                    return Err(format!("{name} takes one argument"));
                }
                let v = arg(&args, 0)?;
                if v < 0.0 {
                    return Err(format!("{name} of a negative value"));
                }
                Ok(Value::Num(v.sqrt()))
            }
            // Text measurement (#147): the args are already evaluated
            // (TheText arrives as a Str through the bridge), the
            // measurement itself defers to the bridge.
            "TEXTWIDTH" => {
                if args.len() != 1 {
                    return Err("TEXTWIDTH takes one argument".into());
                }
                let text = args[0].clone().coerce_str();
                self.measure
                    .text_width(&text)
                    .map(Value::Num)
                    .ok_or_else(|| "TEXTWIDTH: no text metrics bridge".to_string())
            }
            "TEXTHEIGHT" => {
                if args.len() != 2 {
                    return Err("TEXTHEIGHT takes two arguments".into());
                }
                let text = args[0].clone().coerce_str();
                let width = arg(&args, 1)?;
                self.measure
                    .text_height(&text, width)
                    .map(Value::Num)
                    .ok_or_else(|| "TEXTHEIGHT: no text metrics bridge".to_string())
            }
            // ---- #122/FS-033 breadth slice ----
            // Math: the corpus composes ABS/ATAN2/CEILING/INT with the
            // control functions (utilisation.vsdx master3/master36);
            // ATAN2 takes y first ("counterclockwise from the x axis",
            // the chord master's Angle=ATAN2(EndY-BeginY,EndX-BeginX));
            // (0,0) is 0, not a domain error.
            "ABS" => self.call_1(name, &args, |x| Ok(x.abs())),
            "SIGN" => self.call_1(name, &args, |x| {
                Ok(if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                })
            }),
            "INT" => self.call_1(name, &args, |x| Ok(x.floor())),
            "EXP" => self.call_1(name, &args, okf(f64::exp)),
            "LN" => self.call_1(name, &args, |x| {
                if x <= 0.0 {
                    Err("LN of a non-positive value".into())
                } else {
                    Ok(x.ln())
                }
            }),
            "COS" => self.call_1(name, &args, okf(f64::cos)),
            "SIN" => self.call_1(name, &args, okf(f64::sin)),
            "TAN" => self.call_1(name, &args, okf(f64::tan)),
            "ACOS" => self.call_1(name, &args, |x| {
                (-1.0..=1.0)
                    .contains(&x)
                    .then(|| x.acos())
                    .ok_or_else(|| "ACOS out of range".to_string())
            }),
            "ASIN" => self.call_1(name, &args, |x| {
                (-1.0..=1.0)
                    .contains(&x)
                    .then(|| x.asin())
                    .ok_or_else(|| "ASIN out of range".to_string())
            }),
            "ATAN" => self.call_1(name, &args, okf(f64::atan)),
            "DEG" => self.call_1(name, &args, |rad| Ok(rad * 180.0 / std::f64::consts::PI)),
            "RAD" => self.call_1(name, &args, |deg| Ok(deg * std::f64::consts::PI / 180.0)),
            "PI" => {
                if !args.is_empty() {
                    return Err("PI takes no arguments".into());
                }
                Ok(Value::Num(std::f64::consts::PI))
            }
            "ATAN2" => {
                if args.len() != 2 {
                    return Err("ATAN2 takes two arguments".into());
                }
                let (y, x) = (arg(&args, 0)?, arg(&args, 1)?);
                Ok(Value::Num(if y == 0.0 && x == 0.0 {
                    0.0
                } else {
                    y.atan2(x)
                }))
            }
            "CEILING" | "FLOOR" => {
                if args.is_empty() || args.len() > 2 {
                    return Err(format!("{name} takes one or two arguments"));
                }
                let x = arg(&args, 0)?;
                let m = args.get(1).and_then(|v| v.as_f64());
                multiple_round(x, m, upper_name == "CEILING")
            }
            "ROUND" => {
                if args.is_empty() || args.len() > 2 {
                    return Err("ROUND takes one or two arguments".into());
                }
                let x = arg(&args, 0)?;
                let digits = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                Ok(Value::Num(decimal_round(x, digits)))
            }
            "MOD" => {
                if args.len() != 2 {
                    return Err("MOD takes two arguments".into());
                }
                let (x, y) = (arg(&args, 0)?, arg(&args, 1)?);
                if y == 0.0 {
                    return Err("MOD of zero".into());
                }
                // Visio's remainder keeps the MAGNITUDE (the ADS_M
                // Scratch.A1 witness: −π/4 mod 2π = π/4).
                Ok(Value::Num((x % y).abs()))
            }
            "LOG" => {
                if args.is_empty() || args.len() > 2 {
                    return Err("LOG takes one or two arguments".into());
                }
                let x = arg(&args, 0)?;
                let base = args.get(1).and_then(|v| v.as_f64()).unwrap_or(10.0);
                if x <= 0.0 || base <= 0.0 || base == 1.0 {
                    return Err("LOG out of domain".into());
                }
                Ok(Value::Num(x.log(base)))
            }
            // String functions (#122 names the set; LEFT/LEN/MID/RIGHT
            // are corpus-evidenced in utilisation.vsdx). Positions are
            // 1-based and character-counted like LEN.
            "LEN" => {
                if args.len() != 1 {
                    return Err("LEN takes one argument".into());
                }
                Ok(Value::Num(
                    args[0].clone().coerce_str().chars().count() as f64
                ))
            }
            "LEFT" | "RIGHT" => {
                if args.len() != 2 {
                    return Err(format!("{name} takes two arguments"));
                }
                let text = args[0].clone().coerce_str();
                let n = arg(&args, 1)?;
                if n < 0.0 {
                    return Err(format!("{name} of a negative count"));
                }
                let n = (n as usize).min(text.chars().count());
                let s: String = if upper_name == "LEFT" {
                    text.chars().take(n).collect()
                } else {
                    let skip = text.chars().count() - n;
                    text.chars().skip(skip).collect()
                };
                Ok(Value::Str(s))
            }
            "MID" => {
                if args.len() != 3 {
                    return Err("MID takes three arguments".into());
                }
                let text = args[0].clone().coerce_str();
                let start = arg(&args, 1)?;
                let count = arg(&args, 2)?;
                if start < 1.0 || count < 0.0 {
                    return Err("MID out of domain".into());
                }
                Ok(Value::Str(
                    text.chars()
                        .skip((start - 1.0) as usize)
                        .take(count as usize)
                        .collect(),
                ))
            }
            "TRIM" => {
                if args.len() != 1 {
                    return Err("TRIM takes one argument".into());
                }
                let text = args[0].clone().coerce_str();
                // Visio's TRIM: single spaces between words, none
                // leading/trailing.
                Ok(Value::Str(
                    text.split_whitespace().collect::<Vec<_>>().join(" "),
                ))
            }
            "UPPER" => {
                if args.len() != 1 {
                    return Err("UPPER takes one argument".into());
                }
                Ok(Value::Str(args[0].clone().coerce_str().to_uppercase()))
            }
            "LOWER" => {
                if args.len() != 1 {
                    return Err("LOWER takes one argument".into());
                }
                Ok(Value::Str(args[0].clone().coerce_str().to_lowercase()))
            }
            "SUBSTITUTE" => {
                if args.len() != 3 {
                    return Err("SUBSTITUTE takes three arguments".into());
                }
                let text = args[0].clone().coerce_str();
                let old = args[1].clone().coerce_str();
                let new = args[2].clone().coerce_str();
                if old.is_empty() {
                    return Ok(Value::Str(text));
                }
                Ok(Value::Str(text.replace(&old, &new)))
            }
            "FIND" => {
                if args.len() < 2 || args.len() > 4 {
                    return Err("FIND takes two to four arguments".into());
                }
                let needle = args[0].clone().coerce_str();
                let haystack = args[1].clone().coerce_str();
                let start = args.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0);
                let ignore_case = args
                    .get(3)
                    .and_then(|v| v.as_f64())
                    .map(|v| v != 0.0)
                    .unwrap_or(false);
                // The start range checks the ORIGINAL haystack; the
                // search bounds off the SEARCHED string's char count —
                // case folding can change the count (İ), and a needle
                // longer than the haystack is simply not found (#218
                // review: `saturating_sub` + a slice could panic).
                if start < 1.0 || start > haystack.chars().count() as f64 {
                    return Err("FIND start out of range".into());
                }
                let skip = (start - 1.0) as usize;
                let (hay, needle) = if ignore_case {
                    (haystack.to_lowercase(), needle.to_lowercase())
                } else {
                    (haystack, needle)
                };
                let hay: Vec<char> = hay.chars().collect();
                let needle: Vec<char> = needle.chars().collect();
                if needle.is_empty() {
                    return Ok(Value::Num(start));
                }
                // `checked_sub` makes a needle longer than the
                // haystack an empty search range (never a slice panic).
                let at = hay.len().checked_sub(needle.len()).and_then(|last| {
                    (skip..=last).find(|&i| hay[i..i + needle.len()] == needle[..])
                });
                match at {
                    Some(i) => Ok(Value::Num(i as f64 + 1.0)),
                    None => Err("FIND: text not found".into()),
                }
            }
            "VALUE" => {
                if args.len() != 1 {
                    return Err("VALUE takes one argument".into());
                }
                let text = args[0].clone().coerce_str();
                text.trim()
                    .parse::<f64>()
                    .map(Value::Num)
                    .map_err(|_| "VALUE of a non-numeric string".to_string())
            }
            // Color functions over the #122 color model: a color is a
            // number — 0–23 the fixed [MS-VSDX] palette, ≥24 a packed
            // 0xRRGGBB literal. HSL's integer path reproduces the
            // legacy 240-step grid exactly (HSL(144,116,125) =
            // #4979C0, witnessed against a shipped stencil master);
            // TINT/SHADE apply the measured endpoint rule.
            "RGB" | "HSL" => {
                if args.len() != 3 {
                    return Err(format!("{name} takes three arguments"));
                }
                let (a, b, c) = (arg(&args, 0)?, arg(&args, 1)?, arg(&args, 2)?);
                pack_color(if upper_name == "RGB" {
                    rgb_from_channels(a, b, c)?
                } else {
                    hsl_to_channels(a, b, c)?
                })
            }
            "RED" | "GREEN" | "BLUE" => {
                if args.len() != 1 {
                    return Err(format!("{name} takes one argument"));
                }
                let ch = match color_channels(arg(&args, 0)?) {
                    Some([r, g, b]) => match upper_name.as_str() {
                        "RED" => f64::from(r),
                        "GREEN" => f64::from(g),
                        _ => f64::from(b),
                    },
                    None => 0.0, // an invalid color extracts as 0
                };
                Ok(Value::Num(ch))
            }
            "HUE" | "SAT" | "LUM" => {
                if args.len() != 1 {
                    return Err(format!("{name} takes one argument"));
                }
                let v = match color_channels(arg(&args, 0)?) {
                    Some(rgb) => hsl_components(rgb),
                    None => [0.0; 3],
                };
                let slot = match upper_name.as_str() {
                    "HUE" => 0,
                    "SAT" => 1,
                    _ => 2,
                };
                let mut out = v[slot].round();
                if slot == 0 {
                    out = out.rem_euclid(240.0);
                }
                Ok(Value::Num(out))
            }
            "TINT" | "SHADE" => {
                if args.len() != 2 {
                    return Err(format!("{name} takes two arguments"));
                }
                let color = arg(&args, 0)?;
                let amount = arg(&args, 1)?.trunc();
                // SHADE is TINT with the opposite signed adjustment —
                // positive moves toward black.
                let amount = if upper_name == "SHADE" {
                    -amount
                } else {
                    amount
                };
                let Some(rgb) = color_channels(color) else {
                    return Err(format!("{name} of an invalid color"));
                };
                let [h, s, l] = hsl_components(rgb);
                let (h, s, l) = (
                    h.round().rem_euclid(240.0),
                    s.round(),
                    l.round().clamp(0.0, 240.0),
                );
                if amount == 0.0 {
                    return pack_color(rgb);
                }
                // The measured endpoint rule: within 100 steps of the
                // approached endpoint the adjustment scales by the
                // remaining distance, then truncates.
                let distance = if amount > 0.0 { 240.0 - l } else { l };
                let adjustment =
                    (amount.clamp(-240.0, 240.0) * distance.min(100.0) / 100.0).trunc();
                let l2 = (l + adjustment).clamp(0.0, 240.0);
                pack_color(hsl_to_channels(h, s, l2)?)
            }
            // No theme engine: the measured no-theme branch (inherited
            // 65534 sentinel, no page theme) is false.
            "ISTHEMED" => {
                if !args.is_empty() {
                    return Err("ISTHEMED takes no arguments".into());
                }
                Ok(Value::Num(0.0))
            }
            // Guide algebra (#122 parity evidence): the intersection of
            // the line (x1,y1)-(x2,y2) with the line (x3,y3)-(x4,y4),
            // treated as infinite lines. Parallel lines error so the
            // caller falls back to the cached V.
            "INTERSECTX" | "INTERSECTY" => {
                if args.len() != 8 {
                    return Err(format!("{name} takes eight arguments"));
                }
                let v: Vec<f64> = (0..8).map(|i| arg(&args, i)).collect::<Result<_, _>>()?;
                let (x1, y1, x2, y2, x3, y3, x4, y4) =
                    (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
                let denom = (x2 - x1) * (y4 - y3) - (y2 - y1) * (x4 - x3);
                if denom.abs() < 1e-12 {
                    return Err(format!("{name}: parallel lines"));
                }
                let t = ((x3 - x1) * (y4 - y3) - (y3 - y1) * (x4 - x3)) / denom;
                let (ix, iy) = (x1 + t * (x2 - x1), y1 + t * (y2 - y1));
                Ok(Value::Num(if upper_name == "INTERSECTX" { ix } else { iy }))
            }
            // ---- #267 field set ----
            // INDEX(index, list [, delimiter] [, fallback]): 0-based
            // token pick — the corpus's
            // `INDEX(0,INDEX(0,MASTERNAME(),":"),".")` master-name
            // split. The omitted/empty delimiter is ";" and an
            // out-of-range index serves the optional fourth-argument
            // fallback (empty when omitted) instead of erroring
            // ([MS-Learn] INDEX function; CodeRabbit PR review).
            "INDEX" => {
                if args.len() < 2 || args.len() > 4 {
                    return Err("INDEX takes two to four arguments".into());
                }
                let idx = arg(&args, 0)? as i64;
                let list = args[1].clone().coerce_str();
                let delim = args
                    .get(2)
                    .map(|v| v.clone().coerce_str())
                    .unwrap_or_else(|| ";".to_string());
                let delim = if delim.is_empty() {
                    ";".to_string()
                } else {
                    delim
                };
                let fallback = args
                    .get(3)
                    .map(|v| v.clone().coerce_str())
                    .unwrap_or_default();
                let tok = match list.split(delim.as_str()).nth(idx.max(0) as usize) {
                    Some(t) => t.trim().to_string(),
                    None => fallback,
                };
                Ok(Value::Str(tok))
            }
            // STRSAME(a, b): case-insensitive equality.
            "STRSAME" => {
                if args.len() != 2 {
                    return Err("STRSAME takes two arguments".into());
                }
                let a = args[0].clone().coerce_str().to_lowercase();
                let b = args[1].clone().coerce_str().to_lowercase();
                Ok(Value::Num(f64::from((a == b) as u32)))
            }
            // Page/document facts — served from the FieldCtx; without
            // one (a master contact sheet) they stay unresolvable so
            // the cached V serves.
            "MASTERNAME" => {
                if !args.is_empty() {
                    return Err("MASTERNAME takes no arguments".into());
                }
                self.fctx
                    .map(|c| {
                        Value::Str(
                            c.master_name
                                .clone()
                                .unwrap_or_else(|| "<no master>".to_string()),
                        )
                    })
                    .ok_or_else(|| "MASTERNAME: no field context".to_string())
            }
            "PAGENAME" => {
                if !args.is_empty() {
                    return Err("PAGENAME takes no arguments".into());
                }
                self.fctx
                    .and_then(|c| c.page_name.clone().map(Value::Str))
                    .ok_or_else(|| "PAGENAME: no field context".to_string())
            }
            "PAGENUMBER" => {
                if !args.is_empty() {
                    return Err("PAGENUMBER takes no arguments".into());
                }
                self.fctx
                    .and_then(|c| c.page_number)
                    .map(Value::Num)
                    .ok_or_else(|| "PAGENUMBER: no field context".to_string())
            }
            "DOCLASTSAVE" | "DOCLASTEDIT" => {
                if !args.is_empty() {
                    return Err(format!("{name} takes no arguments"));
                }
                self.fctx
                    .and_then(|c| c.doc_last_save.clone().map(Value::Str))
                    .ok_or_else(|| format!("{name}: no field context"))
            }
            "DOCCREATION" => {
                if !args.is_empty() {
                    return Err("DOCCREATION takes no arguments".into());
                }
                self.fctx
                    .and_then(|c| c.doc_creation.clone().map(Value::Str))
                    .ok_or_else(|| "DOCCREATION: no field context".to_string())
            }
            // The Document Info set (the dialog's category): served
            // from the core-properties facts; unresolvable without a
            // context so the cached V serves.
            "CREATOR" | "TITLE" | "SUBJECT" | "KEYWORDS" | "DESCRIPTION" => {
                if !args.is_empty() {
                    return Err(format!("{name} takes no arguments"));
                }
                let v = self.fctx.and_then(|c| match upper_name.as_str() {
                    "CREATOR" => c.doc_creator.clone(),
                    "TITLE" => c.doc_title.clone(),
                    "SUBJECT" => c.doc_subject.clone(),
                    "KEYWORDS" => c.doc_keywords.clone(),
                    _ => c.doc_description.clone(),
                });
                v.map(Value::Str)
                    .ok_or_else(|| format!("{name}: no field context"))
            }
            // NOW() is deliberately unresolvable: renders stay
            // deterministic (a live clock would move goldens).
            "NOW" => Err("NOW(): time-dependent, unresolvable".into()),
            // DATE(y, m, d [, h [, m [, s]]]): Excel day serial.
            "DATE" => {
                if args.len() < 3 || args.len() > 6 {
                    return Err("DATE takes three to six arguments".into());
                }
                let y = arg(&args, 0)?;
                let mo = arg(&args, 1)?;
                let d = arg(&args, 2)?;
                let h = args.get(3).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let mi = args.get(4).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let s = args.get(5).and_then(|v| v.as_f64()).unwrap_or(0.0);
                // Range-validate BEFORE the cast: `DATE(1E30,1,1)`
                // saturates at i64::MAX and overflows the civil-date
                // arithmetic instead of erroring back to the cached V
                // (CodeRabbit round 3). The supported calendar span
                // (years 0–9999) bounds every intermediate.
                if !(-1.0..10_000.0).contains(&y)
                    || !(1.0..=12.0).contains(&mo)
                    || !(1.0..=31.0).contains(&d)
                    || !(0.0..24.0).contains(&h)
                    || !(0.0..60.0).contains(&mi)
                    || !(0.0..60.0).contains(&s)
                    || !y.is_finite()
                {
                    return Err("DATE: argument out of the supported calendar range".into());
                }
                let days = days_from_civil(y as i64, mo as i64, d as i64);
                Ok(Value::Num(
                    days as f64 + 25569.0 + (h * 3600.0 + mi * 60.0 + s) / 86400.0,
                ))
            }
            // FORMAT(value, picture): the format-picture engine.
            "FORMAT" => {
                if args.len() != 2 {
                    return Err("FORMAT takes two arguments".into());
                }
                let v = args[0].clone().coerce_str();
                let pic = args[1].clone().coerce_str();
                let units = self
                    .fctx
                    .map_or(crate::fieldfmt::UnitSystem::Us, |c| c.units);
                Ok(Value::Str(crate::fieldfmt::format_value(
                    &v, None, &pic, units,
                )))
            }
            // FORMATEX(value, picture [, unit-in] [, unit-out] [, lcid]):
            // FORMAT with explicit unit conversion — the value is
            // expressed in `unit-in` and displayed in `unit-out`
            // (both length units; dates ignore them, matching the
            // corpus's FORMATEX chains). The lcid is out of scope
            // (English month names everywhere).
            "FORMATEX" => {
                if args.len() < 2 || args.len() > 5 {
                    return Err("FORMATEX takes two to five arguments".into());
                }
                let v = args[0].clone().coerce_str();
                let pic = args[1].clone().coerce_str();
                let unit_of = |args: &[Value], i: usize| -> Option<String> {
                    match args.get(i) {
                        Some(Value::Omitted) | None => None,
                        Some(v) => Some(v.clone().coerce_str()),
                    }
                };
                let unit_in = unit_of(&args, 2);
                let unit_out = unit_of(&args, 3);
                let units = self
                    .fctx
                    .map_or(crate::fieldfmt::UnitSystem::Us, |c| c.units);
                let converted = match (&unit_in, &unit_out) {
                    // Only the input side converts here — to INTERNAL
                    // INCHES. The output-unit hint below performs the
                    // single remaining conversion; pre-converting to
                    // the output unit made format_value convert twice
                    // (FORMATEX(1,"0.00u","in","cm") → 6.45cm).
                    (Some(i), Some(_)) => convert_units(&v, i),
                    // An OMITTED destination displays in the source
                    // unit ([MS] FORMATEX; CodeRabbit PR review):
                    // convert to inches here and hint the source unit
                    // below — FORMATEX(1,"0.00u","ft") → `1.00ft.`.
                    (Some(i), None) => convert_units(&v, i),
                    _ => v,
                };
                let hint: Option<String> = match (&unit_out, &unit_in) {
                    (Some(o), _) => Some(o.to_ascii_uppercase()),
                    (None, Some(i)) => Some(i.to_ascii_uppercase()),
                    (None, None) => None,
                };
                let hint = hint.as_deref();
                Ok(Value::Str(crate::fieldfmt::format_value(
                    &converted, hint, &pic, units,
                )))
            }
            // FIELDPICTURE(n): the built-in picture string.
            "FIELDPICTURE" => {
                if args.len() != 1 {
                    return Err("FIELDPICTURE takes one argument".into());
                }
                Ok(Value::Str(crate::fieldfmt::field_picture(
                    arg(&args, 0)? as i64
                )))
            }
            other => Err(format!("function '{other}' not supported")),
        }
    }
}
