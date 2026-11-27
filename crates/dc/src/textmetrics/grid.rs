/// Conservative advance for a glyph no loaded face covers (wide-ish
/// Latin; CJK full-width runs wider, but coverage gaps are rare in the
/// corpus and never wrap-decisive at this precision).
pub(super) const UNCOVERED_EM: f64 = 0.55;

/// One step of the 18-fractional-bit inch composition grid (#234):
/// 1/2^18 in ≈ 0.0000038 in. Visio quantizes COMPOSED metrics — font
/// sizes, then per-glyph advances — onto this grid before summing them
/// (the measured 120 %-spacing height corrections at 6/8/10/12 pt are
/// exactly the round-to-nearest residues of the quantized size). Only
/// measurement quantizes; SVG painting keeps the raw size (#234).
const GRID: f64 = 262_144.0; // 2^18

/// Round `v` (inches) to the nearest 18-fractional-bit grid step —
/// the quantization Visio applies to composed font sizes (#234).
pub fn quantize18(v: f64) -> f64 {
    (v * GRID).round() / GRID
}

/// Truncate `v` (inches) toward zero on the grid — the reduction
/// Visio applies to a script run's drawn size before its glyph
/// advances round on the grid (#230: "the reduced em is truncated on
/// the 18-fractional-bit inch composition grid").
pub fn trunc18(v: f64) -> f64 {
    (v * GRID).trunc() / GRID
}

/// Round `v` (inches) onto the grid — the per-glyph advance rounding
/// (#234 terminal-run advance, #230 script advances).
pub(super) fn round18(v: f64) -> f64 {
    (v * GRID).round() / GRID
}
