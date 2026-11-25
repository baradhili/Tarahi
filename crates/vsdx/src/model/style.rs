//! Stylesheets and the built-in "No Style" defaults.

use super::raw::Attrs;
use super::sheet::Sheet;

/// A stylesheet from `document.xml`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleSheet {
    pub id: i64,
    pub name_u: Option<String>,
    pub attrs: Attrs,
    pub sheet: Sheet,
}

/// Built-in defaults observed in shipped stylesheets ("No Style", ID 0).
pub fn builtin_defaults() -> Vec<(&'static str, f64)> {
    vec![
        // (name, value in internal units: inches / radians / ints)
        ("LineWeight", 0.01041666666666667),
        ("LinePattern", 1.0),
        ("LineColor", 0.0),
        ("FillPattern", 1.0),
        ("FillForegnd", 1.0),
        ("FillBkgnd", 0.0),
        ("BeginArrow", 0.0),
        ("EndArrow", 0.0),
        ("Rounding", 0.0),
    ]
}
