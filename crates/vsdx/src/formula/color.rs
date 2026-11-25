//! The #122 color model: a color is an f64 — 0–23 the fixed [MS-VSDX]
//! standard palette, ≥24 a packed 0xRRGGBB literal. Fractions are
//! invalid.

use super::value::Value;

/// The specification-fixed standard palette (indices 0–23); custom
/// colors start at 24.
const STANDARD_PALETTE: [[u8; 3]; 24] = [
    [0, 0, 0],
    [255, 255, 255],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [255, 255, 0],
    [255, 0, 255],
    [0, 255, 255],
    [128, 0, 0],
    [0, 128, 0],
    [0, 0, 128],
    [128, 128, 0],
    [128, 0, 128],
    [0, 128, 128],
    [192, 192, 192],
    [230, 230, 230],
    [205, 205, 205],
    [179, 179, 179],
    [154, 154, 154],
    [128, 128, 128],
    [102, 102, 102],
    [77, 77, 77],
    [51, 51, 51],
    [26, 26, 26],
];

/// The RGB channels of a color number, or None when invalid.
pub(crate) fn color_channels(color: f64) -> Option<[u8; 3]> {
    if !color.is_finite() || color < 0.0 || color.fract() != 0.0 || color > f64::from(u32::MAX) {
        return None;
    }
    let n = color as u32;
    if n < 24 {
        Some(STANDARD_PALETTE[n as usize])
    } else {
        Some([
            ((n >> 16) & 0xFF) as u8,
            ((n >> 8) & 0xFF) as u8,
            (n & 0xFF) as u8,
        ])
    }
}

/// Pack 0xRRGGBB channels into the color number.
pub(crate) fn pack_color(rgb: [u8; 3]) -> Result<Value, String> {
    Ok(Value::Num(f64::from(
        (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]),
    )))
}

/// Clamped 0–255 channels from RGB()'s arguments.
pub(crate) fn rgb_from_channels(r: f64, g: f64, b: f64) -> Result<[u8; 3], String> {
    let cl = |v: f64| -> Result<u8, String> {
        if !v.is_finite() {
            return Err("RGB of a non-finite value".into());
        }
        Ok(v.round().clamp(0.0, 255.0) as u8)
    };
    Ok([cl(r)?, cl(g)?, cl(b)?])
}

/// HSL(h, s, l) on the 0–240 scale → RGB channels. Integral triples go
/// through the legacy 240-step integer grid (HSL(144,116,125) =
/// #4979C0, witnessed against a shipped stencil master); fractional
/// triples use the continuous conversion with rounded channels.
pub(crate) fn hsl_to_channels(h: f64, s: f64, l: f64) -> Result<[u8; 3], String> {
    if !(0.0..240.0).contains(&h) || !(0.0..=240.0).contains(&s) || !(0.0..=240.0).contains(&l) {
        return Err("HSL out of range".into());
    }
    if h.fract() == 0.0 && s.fract() == 0.0 && l.fract() == 0.0 {
        return Ok(integer_hsl(h as u32, s as u32, l as u32));
    }
    let (h, s, l) = (h / 40.0, s / 240.0, l / 240.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let rgb = if h < 1.0 {
        [c, x, 0.0]
    } else if h < 2.0 {
        [x, c, 0.0]
    } else if h < 3.0 {
        [0.0, c, x]
    } else if h < 4.0 {
        [0.0, x, c]
    } else if h < 5.0 {
        [x, 0.0, c]
    } else {
        [c, 0.0, x]
    };
    Ok(rgb.map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8))
}

/// The legacy 240-step integer interpolation: upper/lower luminosity
/// rails, then per-channel interpolation at the hue offsets
/// (R: h+80, G: h, B: h+160 mod 240), each scaled back to 0–255.
fn integer_hsl(h: u32, s: u32, l: u32) -> [u8; 3] {
    if s == 0 {
        return [((l * 255 + 120) / 240) as u8; 3];
    }
    let upper = if l <= 120 {
        (l * (240 + s) + 120) / 240
    } else {
        l + s - (l * s + 120) / 240
    };
    let lower = 2 * l - upper;
    let channel = |h: u32| {
        let h = h % 240;
        let c = if h < 40 {
            lower + ((upper - lower) * h + 20) / 40
        } else if h < 120 {
            upper
        } else if h < 160 {
            lower + ((upper - lower) * (160 - h) + 20) / 40
        } else {
            lower
        };
        ((c * 255 + 120) / 240) as u8
    };
    [channel(h + 80), channel(h), channel(h + 160)]
}

/// An RGB triple's HSL components on the 0–240 scale (HUE/SAT/LUM and
/// the TINT/SHADE endpoint rule decompose through this).
pub(crate) fn hsl_components(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    let l = (max + min) / 2.0;
    if c == 0.0 {
        return [0.0, 0.0, l * 240.0];
    }
    let h = if max == r {
        ((g - b) / c).rem_euclid(6.0)
    } else if max == g {
        (b - r) / c + 2.0
    } else {
        (r - g) / c + 4.0
    };
    [
        h * 40.0,
        c / (1.0 - (2.0 * l - 1.0).abs()) * 240.0,
        l * 240.0,
    ]
}
