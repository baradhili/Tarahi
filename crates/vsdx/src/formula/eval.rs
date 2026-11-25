//! The evaluation entry points: plain [`eval`], the measured
//! [`eval_measured`], and the field-layer [`eval_field_measured`].

use super::env::{Env, FieldCtx, NoText, StrEnv, TextMeasure};
use super::lex::tokenize;
use super::parser::Parser;
use super::value::Value;

/// Evaluate a formula. Returns `Err` for anything unsupported so callers
/// can fall back to the cached value. Text functions
/// (`TEXTWIDTH`/`TEXTHEIGHT`/`TheText`) are unsupported here — use
/// [`eval_measured`] with a [`TextMeasure`] implementation.
pub fn eval(expr: &str, env: &Env) -> Result<Value, String> {
    eval_measured(expr, env, &NoText)
}

/// [`eval`] with a metrics bridge: `TheText` resolves to the sheet's
/// text and `TEXTWIDTH`/`TEXTHEIGHT` measure through `m` (#147). Any
/// measurement the bridge cannot supply still errors, so callers fall
/// back to the cached `V` exactly as before.
pub fn eval_measured(expr: &str, env: &Env, m: &dyn TextMeasure) -> Result<Value, String> {
    let empty = StrEnv::new();
    eval_field_measured(expr, env, &empty, None, m)
}

/// The FIELD evaluation entry point (GitHub #267): [`eval_measured`]
/// plus the string-cell environment and the page/document context the
/// corpus's Field formulas compose (`User.*`/`Prop.*` string refs,
/// `INDEX`, `STRSAME`, `MASTERNAME`, `PAGENAME`/`PAGENUMBER`,
/// `DOCLASTSAVE`/`DOCLASTEDIT`/`DOCCREATION`, `DATE`, `FORMAT`,
/// `FORMATEX`, `FIELDPICTURE`). `NOW()` stays deliberately
/// unresolvable — renders must stay deterministic, so those fields
/// serve their cached `V`.
pub fn eval_field_measured(
    expr: &str,
    env: &Env,
    strs: &StrEnv,
    fctx: Option<&FieldCtx>,
    m: &dyn TextMeasure,
) -> Result<Value, String> {
    let toks = tokenize(expr)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        env,
        strs,
        fctx,
        measure: m,
    };
    let v = p.expr()?;
    if p.pos != toks.len() {
        return Err(format!("trailing tokens at {}", p.pos));
    }
    Ok(v)
}

/// [`eval_field_measured`] without a metrics bridge.
pub fn eval_field(
    expr: &str,
    env: &Env,
    strs: &StrEnv,
    fctx: Option<&FieldCtx>,
) -> Result<Value, String> {
    eval_field_measured(expr, env, strs, fctx, &NoText)
}
