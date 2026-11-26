//! Line-jump arc overlay: the page-space paddings of the canvas
//! overlay's screen-px jump extras and their arc emission.
use std::fmt::Write as _;
/// The page-space paddings of the canvas overlay's screen-px extras
/// (2 px gap weight, 1 px under extension), pinned at 96 dpi.
const JUMP_GAP_EXTRA_IN: f64 = 2.0 / 96.0;
const JUMP_UNDER_EXT_IN: f64 = 1.0 / 96.0;

/// One hop in page-space SVG (y-down: `sy = page_h − y`), the canvas
/// overlay's three strokes — the white gap across the jumper, the
/// under line redrawn straight through it, and the hop arc bulging
/// page-up (`canvas.rs`'s screen-up flip, computed here in page
/// space so rotated jumpers stay correct). `radius` is the page's
/// resolved jump radius (#294 — LineToLineX × LineJumpFactorX / 2).
pub(crate) fn append_jump_scaled(
    svg: &mut String,
    page_h: f64,
    j: &crate::geom::Jump,
    radius: f64,
) {
    let (r, w, uw) = (radius, j.weight, j.under_weight);
    let sy = |y: f64| page_h - y;
    let pt = |p: (f64, f64)| format!("{:.5} {:.5}", p.0, sy(p.1));
    let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    // 1. the gap — paper-white, slightly wider than the jumper.
    let g1 = (j.at.0 - j.dir.0 * r, j.at.1 - j.dir.1 * r);
    let g2 = (j.at.0 + j.dir.0 * r, j.at.1 + j.dir.1 * r);
    let white = "#ffffff";
    let _ = writeln!(
        svg,
        r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{white}" stroke-width="{gw:.4}"/>"#,
        x1 = g1.0,
        y1 = sy(g1.1),
        x2 = g2.0,
        y2 = sy(g2.1),
        gw = w + JUMP_GAP_EXTRA_IN,
    );
    // 2. the under line passes straight through the gap.
    let ext = r + uw + JUMP_UNDER_EXT_IN;
    let u1 = (j.at.0 - j.under.0 * ext, j.at.1 - j.under.1 * ext);
    let u2 = (j.at.0 + j.under.0 * ext, j.at.1 + j.under.1 * ext);
    let _ = writeln!(
        svg,
        r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-width="{uw:.4}"/>"#,
        x1 = u1.0,
        y1 = sy(u1.1),
        x2 = u2.0,
        y2 = sy(u2.1),
        c = hex(j.under_color),
        uw = uw,
    );
    // 3. the hop arc — a 9-point semicircle from g1 around to g2
    // through at + n·r. Visio's default hop directions
    // (PageLineJumpDirX/Y = 0): a horizontal-class jumper bulges UP,
    // a vertical-class one bulges LEFT (the b01 witness) — flipping
    // toward page-up alone drew sideways zigzags on steep lines
    // (#322).
    let mut n = (-j.dir.1, j.dir.0);
    let horizontal = j.dir.0.abs() >= j.dir.1.abs();
    if (horizontal && n.1 < 0.0) || (!horizontal && n.0 > 0.0) {
        n = (-n.0, -n.1);
    }
    let mut path = String::new();
    for k in 0..=8 {
        let t = k as f64 / 8.0;
        let ang = std::f64::consts::PI * t;
        let local = (
            -j.dir.0 * r * ang.cos() + n.0 * r * ang.sin(),
            -j.dir.1 * r * ang.cos() + n.1 * r * ang.sin(),
        );
        let p = (j.at.0 + local.0, j.at.1 + local.1);
        path.push_str(if k == 0 { "M " } else { "L " });
        path.push_str(&pt(p));
        path.push(' ');
    }
    let _ = writeln!(
        svg,
        r#"<path d="{path}" fill="none" stroke="{c}" stroke-width="{w:.4}"/>"#,
        c = hex(j.color),
        w = w,
    );
}
