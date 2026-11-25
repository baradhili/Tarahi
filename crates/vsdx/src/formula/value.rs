//! The evaluated [`Value`] model: numbers, strings, and Visio's
//! omitted-argument slot, with the coercions the parser composes.

/// An evaluated formula value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Str(String),
    /// An omitted argument slot (`FORMATEX(1,\"{{DD}}\",,,1033)`) —
    /// the corpus's locale probes leave unit arguments empty. Reads
    /// as no-value: `as_f64`/`coerce_str` fail empty so callers'
    /// optional-argument handling treats it as `None`.
    Omitted,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Omitted => None,
        }
    }
    pub(crate) fn coerce_str(self) -> String {
        match self {
            Value::Num(n) => format_num(n),
            Value::Str(s) => s,
            Value::Omitted => String::new(),
        }
    }
}

pub(crate) fn format_num(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}
