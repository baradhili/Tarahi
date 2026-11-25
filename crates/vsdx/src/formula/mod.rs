//! Minimal ShapeSheet formula evaluator.
//!
//! Supports the subset needed for geometry/size evaluation (Analysis doc 06)
//! plus the #122/FS-033 breadth slice: numeric literals, cell references
//! (`Width`, `Geometry1.X1`, `Sheet.5!Width` cross-sheet refs), `+ - * / ^`,
//! comparison operators (`= <> < <= > >=`), parentheses, unary minus, string
//! literals with `&` concatenation, `&<n>` number-to-string coercion, the
//! control functions master XForms lean on (`IF`, `OR`, `AND`, `NOT`,
//! `GUARD`, `THEMEGUARD`), the math set the corpus composes with them
//! (`ABS`, `ATAN2`, `CEILING`, …), string functions (`LEFT`, `LEN`, …),
//! the color functions over the #122 color model (`RGB`, `HSL`, `TINT`,
//! `SHADE`, …), and the guide-algebra pair `INTERSECTX`/`INTERSECTY`.
//! Still-unevaluated functions (`THEMEVAL`, `SETF`, …) error so callers
//! fall back to the cached `V` value.
//!
//! Layout: [`env`] carries the cell environments, field context, and the
//! text-measurement bridge; [`value`] the evaluated-value model; [`lex`]/
//! [`parser`] the tokenizer and recursive-descent reader; [`eval`] the
//! public entry points; and [`numeric`]/[`color`]/[`datetime`] the helper
//! sets the function dispatch leans on.

mod color;
mod datetime;
mod env;
mod eval;
mod lex;
mod numeric;
mod parser;
mod value;

pub use env::{Env, FieldCtx, StrEnv, TextMeasure};
pub use eval::{eval, eval_field, eval_field_measured, eval_measured};
pub use value::Value;

#[cfg(test)]
mod tests;
