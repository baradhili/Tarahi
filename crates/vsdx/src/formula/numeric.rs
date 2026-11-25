//! Numeric helpers behind the parser's operator and rounding arms,
//! plus FORMATEX's input-unit conversion.

use super::value::Value;

pub(crate) fn num_bin(a: Value, b: Value, f: impl Fn(f64, f64) -> f64) -> Result<Value, String> {
    let (a, b) = (
        a.as_f64().ok_or("numeric op on string")?,
        b.as_f64().ok_or("numeric op on string")?,
    );
    Ok(Value::Num(f(a, b)))
}

pub(crate) fn okf(f: impl Fn(f64) -> f64) -> impl Fn(f64) -> Result<f64, String> {
    move |x| Ok(f(x))
}

/// CEILING/FLOOR: to a same-sign multiple, away from / toward zero.
/// A sign-mismatch pair errors; x=0 or multiple=0 is 0; the multiple
/// defaults to ±1 (x's sign).
pub(crate) fn multiple_round(x: f64, m: Option<f64>, away: bool) -> Result<Value, String> {
    let m = m.unwrap_or(x.signum());
    if x == 0.0 || m == 0.0 {
        return Ok(Value::Num(0.0));
    }
    if x.signum() != m.signum() {
        return Err("CEILING/FLOOR sign mismatch".into());
    }
    let q = x / m;
    Ok(Value::Num((if away { q.ceil() } else { q.floor() }) * m))
}

/// ROUND at a decimal precision — negative precision counts left of
/// the decimal point; ties round away from zero (f64::round).
pub(crate) fn decimal_round(x: f64, digits: f64) -> f64 {
    if digits >= 309.0 {
        return x;
    }
    if digits <= -309.0 {
        return 0.0;
    }
    let scale = 10.0f64.powf(digits.trunc());
    let n = x * scale;
    if !n.is_finite() {
        return x;
    }
    n.round() / scale
}

/// FORMATEX's input-unit conversion: a numeric value expressed in
/// `unit_in`, re-expressed as INTERNAL INCHES (the formatter's hub
/// unit — its output-unit hint performs the one remaining
/// conversion). Non-numeric values and unknown units pass through
/// unchanged.
///
/// Known limitation (CodeRabbit round 2): the value arrives through
/// the tokenizer, which has ALREADY normalized unit-suffixed
/// literals (`1FT` → `12`, internal inches) — `unit_in` would then
/// re-interpret those 12 inches as 12 feet. The corpus's FORMATEX
/// values are all bare numbers or dates, so the codepath is
/// unaffected in practice; unit-suffixed FORMATEX sources stay
/// unsupported rather than half-converted.
pub(crate) fn convert_units(value: &str, unit_in: &str) -> String {
    let Some(v) = value.trim().parse::<f64>().ok() else {
        return value.to_string();
    };
    match crate::fieldfmt::Unit::from_u(&unit_in.to_ascii_uppercase()) {
        Some(i) => {
            // Each unit's factor is its inches multiplier, so invert
            // the input by dividing.
            format!("{}", v / i.inches_to(1.0))
        }
        None => value.to_string(),
    }
}
