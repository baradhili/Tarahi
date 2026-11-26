//! SVG rendering backend (the first `Canvas` implementation).
//!
//! Coordinate mapping (Analysis doc 06): VSDX stores inches with origin
//! bottom-left and Y up; SVG is Y down. Each shape becomes a `<g>` with the
//! XForm transform
//!   translate(PinX, PageH-PinY) rotate(-Angle) scale(FlipX,FlipY)
//!   translate(-LocPinX, +LocPinY)
//! applied to geometry whose local Y is negated at path-build time.

use vsdx::model::Sheet;
use vsdx::{Cell, Document, Row, Shape};

const PX_PER_IN: f64 = 96.0;

mod affine;
mod ink;
mod jumps;
mod master;
mod page;
mod shape_layer;
#[cfg(test)]
mod tests;
mod text;

// Stable re-exports: every path under `svg::` keeps resolving exactly as
// before the split — the named `pub use`s are the public API, and the
// crate-level globs carry the crate-internal helpers to the sibling
// modules (a `pub` glob cannot re-export less-visible items).
// (ink, master, and page carry no crate-only items — their `pub`
// items are re-exported by name below.)
pub(crate) use affine::*;
pub(crate) use jumps::*;
pub(crate) use shape_layer::*;
pub(crate) use text::*;

pub use affine::{page_point_to_parent_local, parent_local_point_to_page, shape_page_rect, Affine};
pub use ink::{shape_ink_bounds, shape_ink_bounds_focused, subtree_ink_boxes};
pub use jumps::{backdrop_color_at, jump_normal};
pub use master::{render_master, render_masters};
pub use page::{
    render_page, render_page_export, render_page_no_text, render_page_scoped, render_page_with,
};
pub use shape_layer::{
    render_shape_layer, render_shape_layer_opts, render_shape_layer_opts_culled,
};
pub use text::{
    greeked_count, paragraph_base_rtl, text_block, text_block_page_rect, text_first_baseline,
    toggle_rtl_marks, BulletGlyph, BulletSpec, TextBlock, TEXT_GREEK_PX,
};

// Shared leaf helpers used across the submodules.
/// Resolve a color-ish cell to `#rrggbb`.
/// Resolve a color cell for rendering: literal hex / document palette,
/// or — when the cell's formula is a THEMEVAL("VariantColorN") — the
/// theme's variant color (Visio evaluates the formula, not the cached
/// palette-index V, for themed shapes).
/// One color cell's stored value as `#rrggbb` — the shared mapping
/// behind the fill, line, and TEXT color paths (#73): a `#…` hex
/// string verbatim, a palette/document-table index, or a THEMEVAL
/// variant formula. Unresolvable values fall to black.
fn color_value(doc: &Document, v: &str, f: Option<&str>) -> String {
    if let Some(f) = f {
        if f.contains("THEMEVAL") {
            for n in 1..=8 {
                if f.contains(&format!("VariantColor{n}")) {
                    let variants = doc.theme_variant_colors();
                    if let Some(col) = variants.get(n - 1) {
                        return col.clone();
                    }
                }
            }
            // THEMEVAL with no variant we model: fall through to V
        }
    }
    let v = v.trim();
    if v.starts_with('#') {
        v.to_string()
    } else {
        match v.parse::<i64>() {
            Ok(idx) => doc.color(idx).cloned().unwrap_or_else(|| palette(idx)),
            Err(_) => "#000000".into(),
        }
    }
}

/// Standard color table for indices 0–23, verbatim from [MS-VSDX]
/// "Color Table": any integer color value ≤ 23 resolves here; ≥ 24 resolves
/// through the document `Colors` table.
fn palette(idx: i64) -> String {
    match idx {
        0 => "#000000",
        1 => "#FFFFFF",
        2 => "#FF0000",
        3 => "#00FF00",
        4 => "#0000FF",
        5 => "#FFFF00",
        6 => "#FF00FF",
        7 => "#00FFFF",
        8 => "#800000",
        9 => "#008000",
        10 => "#000080",
        11 => "#808000",
        12 => "#800080",
        13 => "#008080",
        14 => "#C0C0C0",
        15 => "#E6E6E6",
        16 => "#CDCDCD",
        17 => "#B3B3B3",
        18 => "#9A9A9A",
        19 => "#808080",
        20 => "#666666",
        21 => "#4D4D4D",
        22 => "#333333",
        23 => "#1A1A1A",
        _ => "#000000",
    }
    .into()
}

fn num(cell: Option<&Cell>) -> Option<f64> {
    cell.and_then(|c| c.numeric())
}

fn cell_num(cell: &Cell, env: &vsdx::formula::Env) -> Option<f64> {
    if let Some(v) = cell.numeric() {
        return Some(v);
    }
    let f = cell.f.as_deref()?;
    if f == "Inh" || f == "No Formula" {
        return None;
    }
    vsdx::formula::eval(f, env).ok()?.as_f64()
}

/// Resolved (Width, Height) for geometry scaling.
pub(crate) fn shape_size(doc: &Document, s: &Shape, chain: &[&Shape]) -> (f64, f64) {
    let env = shape_env(doc, s, chain);
    let get = |n: &str| {
        doc.resolve_cell(s, chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    (get("Width").unwrap_or(1.0), get("Height").unwrap_or(1.0))
}

/// Environment for evaluating geometry-row formulas: the shape's resolved
/// named cells plus this geometry section's own row cells.
fn shape_env_for(doc: &Document, s: &Shape, chain: &[&Shape], rows: &[Row]) -> vsdx::formula::Env {
    let mut env = shape_env(doc, s, chain);
    for row in rows.iter().filter(|r| r.t.is_some()) {
        for (cname, cell) in &row.cells {
            if let Some(v) = cell.numeric() {
                env.insert(format!("{}.{}", cname, row.ix), v);
            }
        }
    }
    env
}

/// Build an evaluation environment for a shape: resolved cells by name,
/// two passes (values first, then formulas against those values).
pub fn shape_env(doc: &Document, s: &Shape, chain: &[&Shape]) -> vsdx::formula::Env {
    let names = [
        "PinX",
        "PinY",
        "Width",
        "Height",
        "LocPinX",
        "LocPinY",
        "Angle",
        "TxtPinX",
        "TxtPinY",
        "TxtWidth",
        "TxtHeight",
        "LineWeight",
        "Char.Size",
    ];
    let mut env = vsdx::formula::Env::new();
    for n in names {
        if let Some(c) = doc.resolve_cell(s, chain, n) {
            if let Some(v) = c.numeric() {
                env.insert(n.to_string(), v);
            }
        }
    }
    // User-defined cells (master geometry formulas reference
    // `User.Corner` and friends for rounded-corner rectangles).
    // Instance-local rows override the master's: inline copies keep
    // only sparse User caches (e.g. a heading's HTxtWidth) while the
    // rest — CStyle, hidden, … — inherit from the master source, so
    // style-gated section cells like NoShow re-evaluate per instance.
    let own = doc.effective_sheet(s, chain);
    let mut user_sheets: Vec<&Sheet> = Vec::new();
    if let Some(src) = doc.instance_source(s, chain) {
        if !std::ptr::eq(&src.sheet, own.as_ref()) {
            user_sheets.push(&src.sheet);
        }
    }
    user_sheets.push(own.as_ref());
    for sheet in user_sheets {
        for user in sheet.sections_named("User") {
            for row in user.rows.iter().filter(|r| r.n.is_some()) {
                let key = format!("User.{}", row.n.as_deref().unwrap_or(""));
                let inh = row
                    .cell("Value")
                    .and_then(|c| c.f.as_deref())
                    .is_some_and(|f| f == "Inh");
                match row.cell("Value").and_then(|c| c.numeric()) {
                    // The master sheet seeds first; an Inh-marked
                    // local row's numeric cache is the STALE snapshot
                    // and must not override it (CodeRabbit agent
                    // review — the string-side twin of the round-5
                    // render_field rule).
                    Some(v) => {
                        if inh && env.contains_key(&key) {
                            continue;
                        }
                        env.insert(key, v);
                    }
                    // A row whose Value cache is EXPLICITLY emptied
                    // (V='') clears the inherited number: the string
                    // env seeds the empty binding, but the numeric
                    // layer kept the master's 42 for the evaluator to
                    // read FIRST (CodeRabbit PR review). An Inh-marked
                    // EMPTY cache inherits instead of clearing. No
                    // cache at all (a sparse F-only row) still
                    // inherits.
                    None if row
                        .cell("Value")
                        .is_some_and(|c| c.v.is_some() && c.f.as_deref() != Some("Inh")) =>
                    {
                        env.remove(&key);
                    }
                    None => {}
                }
            }
        }
        // Scratch + Controls numeric cells (#249): the connector
        // stencil's band/knee masters derive their geometry rows
        // through them (`LineTo(F:Scratch.X2*Scratch.C1)`,
        // `Controls.Row_1.Y`) — without them every such row fell to
        // its authored cached pose and the ink never tracked the
        // endpoints. Scratch references are `Scratch.X1` (row 1's X
        // cell); Controls references carry the row's name
        // (`Controls.Row_1.Y`).
        for scratch in sheet.sections_named("Scratch") {
            for row in &scratch.rows {
                for (name, cell) in &row.cells {
                    if let Some(v) = cell.numeric() {
                        // Visio's Scratch references are 1-BASED
                        // (`Scratch.X1` is the first row's X).
                        env.insert(format!("Scratch.{}{}", name, row.ix + 1), v);
                    }
                }
            }
        }
        for controls in sheet.sections_named("Controls") {
            for row in &controls.rows {
                let rn = row.n.clone().unwrap_or_else(|| format!("Row_{}", row.ix));
                for (name, cell) in &row.cells {
                    if let Some(v) = cell.numeric() {
                        env.insert(format!("Controls.{}.{}", rn, name), v);
                    }
                }
            }
        }
    }
    // Geometry row coordinates (Geometry1.X1 style references).
    for (i, geo) in own.sections_named("Geometry").iter().enumerate() {
        let gname = format!("Geometry{}", i + 1);
        for row in geo.rows.iter().filter(|r| r.t.is_some()) {
            for (cname, cell) in &row.cells {
                if let Some(v) = cell.numeric() {
                    env.insert(format!("{}.{}{}", gname, cname, row.ix), v);
                }
            }
        }
    }
    // Second pass: formulas may reference the first-pass values.
    for n in names {
        if let Some(c) = doc.resolve_cell(s, chain, n) {
            if c.numeric().is_none() {
                if let Some(f) = c.f.as_deref() {
                    if let Some(v) = vsdx::formula::eval(f, &env).ok().and_then(|v| v.as_f64()) {
                        env.insert(n.to_string(), v);
                    }
                }
            }
        }
    }
    env
}

pub(crate) fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
