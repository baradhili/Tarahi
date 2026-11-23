//! Enhanced Metafile (EMF) → SVG, for `ForeignData ForeignType='EnhMetaFile'`
//! pictures (network-stencil icons and pasted clip art are EMF in Visio
//! files). The GDI record stream is replayed into plain SVG elements in the
//! shape's local box, so the picture stays vector all the way into the PDF
//! export.
//!
//! Scope: the record set the Visio corpus actually uses (dual EMF+/EMF files
//! carry a complete GDI fallback, which is what is played here) — world
//! transforms, the mapping modes, SAVEDC/RESTOREDC, pens/brushes/fonts and
//! the stock objects, path brackets (fill/stroke/clip), the polygon,
//! polyline, polybezier and poly-poly families (16- and 32-bit), rectangles
//! and ellipses, EXTTEXTOUTW text, and STRETCHDIBITS/BITBLT bitmaps.
//! Unknown records are skipped; EMF+ records inside GDI comments are ignored.

use std::fmt::Write as _;

/// A 2-D affine in GDI's row-vector convention: `x' = x·m11 + y·m21 + dx`,
/// `y' = x·m12 + y·m22 + dy`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Xf {
    m11: f64,
    m12: f64,
    m21: f64,
    m22: f64,
    dx: f64,
    dy: f64,
}

impl Xf {
    const ID: Xf = Xf {
        m11: 1.0,
        m12: 0.0,
        m21: 0.0,
        m22: 1.0,
        dx: 0.0,
        dy: 0.0,
    };
    fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        (
            p.0 * self.m11 + p.1 * self.m21 + self.dx,
            p.0 * self.m12 + p.1 * self.m22 + self.dy,
        )
    }
    /// `self` first, then `o` (row vectors: `self · o`).
    fn then(&self, o: &Xf) -> Xf {
        Xf {
            m11: self.m11 * o.m11 + self.m12 * o.m21,
            m12: self.m11 * o.m12 + self.m12 * o.m22,
            m21: self.m21 * o.m11 + self.m22 * o.m21,
            m22: self.m21 * o.m12 + self.m22 * o.m22,
            dx: self.dx * o.m11 + self.dy * o.m21 + o.dx,
            dy: self.dx * o.m12 + self.dy * o.m22 + o.dy,
        }
    }
    fn scale(&self) -> f64 {
        (self.m11 * self.m22 - self.m12 * self.m21).abs().sqrt()
    }
}

#[derive(Clone, Debug)]
enum Obj {
    Pen {
        color: (u8, u8, u8),
        width: f64,
        null: bool,
        cosmetic: bool,
        cap: &'static str,
        join: &'static str,
    },
    Brush {
        color: (u8, u8, u8),
        null: bool,
    },
    Font {
        height: f64,
        bold: bool,
        italic: bool,
        face: String,
        escapement: f64,
    },
}

#[derive(Clone, Debug)]
struct Dc {
    world: Xf,
    map_mode: u32,
    win_org: (f64, f64),
    win_ext: (f64, f64),
    vp_org: (f64, f64),
    vp_ext: (f64, f64),
    pen: Obj,
    brush: Obj,
    font: Obj,
    text_color: (u8, u8, u8),
    text_align: u32,
    even_odd: bool,
    clip: Option<String>,
    cur: (f64, f64),
}

fn stock(n: u32) -> Option<Obj> {
    let brush = |c: u8| Obj::Brush {
        color: (c, c, c),
        null: false,
    };
    let pen = |c: u8, null: bool| Obj::Pen {
        color: (c, c, c),
        width: 0.0,
        null,
        cosmetic: true,
        cap: "round",
        join: "round",
    };
    Some(match n {
        0 => brush(255),
        1 => brush(192),
        2 => brush(128),
        3 => brush(64),
        4 => brush(0),
        5 => Obj::Brush {
            color: (0, 0, 0),
            null: true,
        },
        6 => pen(255, false),
        7 => pen(0, false),
        8 => pen(0, true),
        10..=17 => default_font(),
        _ => return None,
    })
}

fn default_font() -> Obj {
    Obj::Font {
        height: 12.0,
        bold: false,
        italic: false,
        face: "Arial".into(),
        escapement: 0.0,
    }
}

struct Rd<'a>(&'a [u8]);
impl Rd<'_> {
    fn u32(&self, o: usize) -> Option<u32> {
        self.0
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&self, o: usize) -> Option<i32> {
        self.u32(o).map(|v| v as i32)
    }
    fn i16(&self, o: usize) -> Option<i16> {
        self.0
            .get(o..o + 2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
    }
    fn f32(&self, o: usize) -> Option<f64> {
        self.u32(o).map(|v| f64::from(f32::from_bits(v)))
    }
    fn color(&self, o: usize) -> Option<(u8, u8, u8)> {
        self.0.get(o..o + 3).map(|b| (b[0], b[1], b[2]))
    }
    fn xform(&self, o: usize) -> Option<Xf> {
        Some(Xf {
            m11: self.f32(o)?,
            m12: self.f32(o + 4)?,
            m21: self.f32(o + 8)?,
            m22: self.f32(o + 12)?,
            dx: self.f32(o + 16)?,
            dy: self.f32(o + 20)?,
        })
    }
}

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

struct Player {
    out: String,
    defs: String,
    dc: Dc,
    saved: Vec<Dc>,
    objs: Vec<Option<Obj>>,
    /// The open path bracket (box coordinates), if any.
    path: Option<String>,
    /// Device rect of the picture frame → the target box.
    dev_to_box: Xf,
    dev_px_per_mm: (f64, f64),
    clip_seq: usize,
}

impl Player {
    /// Logical → page (world transform) → device (mapping mode) → box.
    fn total(&self) -> Xf {
        let d = &self.dc;
        let page_to_dev = match d.map_mode {
            7 | 8 => {
                let sx = if d.win_ext.0.abs() > 1e-12 {
                    d.vp_ext.0 / d.win_ext.0
                } else {
                    1.0
                };
                let mut sy = if d.win_ext.1.abs() > 1e-12 {
                    d.vp_ext.1 / d.win_ext.1
                } else {
                    1.0
                };
                let sx = if d.map_mode == 7 {
                    // MM_ISOTROPIC keeps the aspect: the smaller scale wins.
                    let s = sx.abs().min(sy.abs());
                    sy = s * sy.signum();
                    s * sx.signum()
                } else {
                    sx
                };
                Xf {
                    m11: sx,
                    m12: 0.0,
                    m21: 0.0,
                    m22: sy,
                    dx: d.vp_org.0 - d.win_org.0 * sx,
                    dy: d.vp_org.1 - d.win_org.1 * sy,
                }
            }
            2..=6 => {
                // Metric/English modes: y up, fixed unit sizes.
                let mm = match d.map_mode {
                    2 => 0.1,
                    3 => 0.01,
                    4 => 0.254,
                    5 => 0.0254,
                    _ => 25.4 / 1440.0,
                };
                let (px, py) = (mm * self.dev_px_per_mm.0, mm * self.dev_px_per_mm.1);
                Xf {
                    m11: px,
                    m12: 0.0,
                    m21: 0.0,
                    m22: -py,
                    dx: d.vp_org.0 - d.win_org.0 * px,
                    dy: d.vp_org.1 + d.win_org.1 * py,
                }
            }
            _ => Xf {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: d.vp_org.0 - d.win_org.0,
                dy: d.vp_org.1 - d.win_org.1,
            },
        };
        d.world.then(&page_to_dev).then(&self.dev_to_box)
    }

    fn pt(&self, p: (f64, f64)) -> String {
        let q = self.total().apply(p);
        format!("{:.4} {:.4}", q.0, q.1)
    }

    fn clip_attr(&self) -> String {
        self.dc
            .clip
            .as_ref()
            .map(|c| format!(r#" clip-path="url(#{c})""#))
            .unwrap_or_default()
    }

    fn fill_attr(&self, rule: bool) -> String {
        match &self.dc.brush {
            Obj::Brush { color, null: false } => {
                let r = if rule && self.dc.even_odd {
                    r#" fill-rule="evenodd""#
                } else {
                    ""
                };
                format!(r#"fill="{}"{r}"#, hex(*color))
            }
            _ => r#"fill="none""#.into(),
        }
    }

    fn stroke_attr(&self) -> String {
        match &self.dc.pen {
            Obj::Pen {
                color,
                width,
                null: false,
                cosmetic,
                cap,
                join,
            } => {
                // Cosmetic pens are one device pixel wide whatever the
                // transform; geometric widths scale with it.
                let s = self.total().scale();
                let dev = self.dev_to_box.scale();
                let w = if *cosmetic || *width <= 0.0 {
                    dev
                } else {
                    width * s
                };
                format!(
                    r#"stroke="{}" stroke-width="{w:.5}" stroke-linecap="{cap}" stroke-linejoin="{join}""#,
                    hex(*color)
                )
            }
            _ => r#"stroke="none""#.into(),
        }
    }

    fn emit(&mut self, d: &str, fill: bool, stroke: bool) {
        if d.trim().is_empty() {
            return;
        }
        let f = if fill {
            self.fill_attr(true)
        } else {
            r#"fill="none""#.into()
        };
        let s = if stroke {
            self.stroke_attr()
        } else {
            r#"stroke="none""#.into()
        };
        if f == r#"fill="none""# && s == r#"stroke="none""# {
            return;
        }
        let _ = writeln!(self.out, r#"<path d="{d}" {f} {s}{}/>"#, self.clip_attr());
    }

    /// Points of a poly record: count at `cnt_off`, data at `pts_off`.
    fn points(r: &Rd, cnt_off: usize, pts_off: usize, short: bool) -> Vec<(f64, f64)> {
        let n = r.u32(cnt_off).unwrap_or(0) as usize;
        let step = if short { 4 } else { 8 };
        (0..n)
            .filter_map(|i| {
                let o = pts_off + i * step;
                if short {
                    Some((f64::from(r.i16(o)?), f64::from(r.i16(o + 2)?)))
                } else {
                    Some((f64::from(r.i32(o)?), f64::from(r.i32(o + 4)?)))
                }
            })
            .collect()
    }

    fn poly_d(&self, pts: &[(f64, f64)], close: bool) -> String {
        let mut d = String::new();
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(d, "{} {} ", if i == 0 { "M" } else { "L" }, self.pt(*p));
        }
        if close && !pts.is_empty() {
            d.push('Z');
        }
        d
    }

    fn bezier_d(&self, start: Option<(f64, f64)>, pts: &[(f64, f64)]) -> String {
        let mut d = String::new();
        let rest = match start {
            Some(s) => {
                let _ = write!(d, "M {} ", self.pt(s));
                pts
            }
            None => pts,
        };
        for c in rest.chunks(3) {
            if c.len() == 3 {
                let _ = write!(
                    d,
                    "C {} {} {} ",
                    self.pt(c[0]),
                    self.pt(c[1]),
                    self.pt(c[2])
                );
            }
        }
        d
    }

    /// An elliptical arc in the rect `a`–`b` (logical), counter-clockwise
    /// from the ray through `s` to the ray through `e` (GDI's default
    /// AD_COUNTERCLOCKWISE, seen on the y-down device).
    fn arc_d(&self, a: (f64, f64), b: (f64, f64), s: (f64, f64), e: (f64, f64)) -> String {
        let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (rx, ry) = (
            ((b.0 - a.0) / 2.0).abs().max(1e-9),
            ((b.1 - a.1) / 2.0).abs().max(1e-9),
        );
        let on = |p: (f64, f64)| {
            let t = ((p.1 - cy) / ry).atan2((p.0 - cx) / rx);
            (cx + rx * t.cos(), cy + ry * t.sin(), t)
        };
        let (sx, sy, ts) = on(s);
        let (_, _, te) = on(e);
        // Counter-clockwise on a y-down device = decreasing angle.
        let mut sweep = ts - te;
        while sweep <= 0.0 {
            sweep += std::f64::consts::TAU;
        }
        // Sample the arc (transform-proof: the world/mapping transform may
        // shear or flip the ellipse).
        let n = ((sweep / std::f64::consts::TAU) * 48.0).ceil().max(2.0) as usize;
        let mut d = format!("M {} ", self.pt((sx, sy)));
        for i in 1..=n {
            let t = ts - sweep * i as f64 / n as f64;
            let _ = write!(d, "L {} ", self.pt((cx + rx * t.cos(), cy + ry * t.sin())));
        }
        d
    }

    fn add_path_or_draw(&mut self, d: String, fill: bool, stroke: bool) {
        if let Some(p) = self.path.as_mut() {
            p.push_str(&d);
        } else {
            self.emit(&d, fill, stroke);
        }
    }

    fn select_clip(&mut self, d: &str) {
        self.clip_seq += 1;
        let id = format!("emfclip{}x{:x}", self.clip_seq, crate::svg::fnv(d));
        let nested = self
            .dc
            .clip
            .as_ref()
            .map(|c| format!(r#" clip-path="url(#{c})""#))
            .unwrap_or_default();
        let rule = if self.dc.even_odd {
            r#" clip-rule="evenodd""#
        } else {
            ""
        };
        let _ = writeln!(
            self.defs,
            r#"<clipPath id="{id}"{nested}><path d="{d}"{rule}/></clipPath>"#
        );
        self.dc.clip = Some(id);
    }

    fn bitmap(
        &mut self,
        rec: &Rd,
        off_bmi: usize,
        cb_bmi: usize,
        off_bits: usize,
        cb_bits: usize,
        dest: [f64; 4],
    ) {
        let (Some(bmi), Some(bits)) = (
            rec.0.get(off_bmi..off_bmi + cb_bmi),
            rec.0.get(off_bits..off_bits + cb_bits),
        ) else {
            return;
        };
        let Some(png) = dib_to_png(bmi, bits) else {
            return;
        };
        let t = self.total();
        let a = t.apply((dest[0], dest[1]));
        let b = t.apply((dest[0] + dest[2], dest[1] + dest[3]));
        let (x, y) = (a.0.min(b.0), a.1.min(b.1));
        let (w, h) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
        let _ = writeln!(
            self.out,
            r#"<image x="{x:.4}" y="{y:.4}" width="{w:.4}" height="{h:.4}" preserveAspectRatio="none" href="data:image/png;base64,{}"{}/>"#,
            crate::svg::base64(&png),
            self.clip_attr()
        );
    }

    fn text(&mut self, r: &Rd) {
        // EMR_EXTTEXTOUTW: Bounds(16) iGraphicsMode exScale eyScale, then
        // EMRTEXT: ptlReference, nChars, offString, options, rcl, offDx.
        let (Some(x), Some(y), Some(n), Some(off)) = (r.i32(36), r.i32(40), r.u32(44), r.u32(48))
        else {
            return;
        };
        let (n, off) = (n as usize, off as usize);
        let Some(raw) = r.0.get(off..off + n * 2) else {
            return;
        };
        let units: Vec<u16> = raw
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        if s.trim().is_empty() {
            return;
        }
        let Obj::Font {
            height,
            bold,
            italic,
            face,
            escapement,
        } = self.dc.font.clone()
        else {
            return;
        };
        let t = self.total();
        let size = (height.abs() * t.scale()).max(1e-4);
        // Cell height (positive) includes internal leading; character
        // height (negative) is the em. Approximate the em for both.
        let size = if height > 0.0 { size * 0.85 } else { size };
        let p = t.apply((f64::from(x), f64::from(y)));
        let align = self.dc.text_align;
        let anchor = match align & 6 {
            6 => "middle",
            2 => "end",
            _ => "start",
        };
        // Vertical reference: TA_BASELINE (24) as is, TA_BOTTOM (8) a
        // descent up, TA_TOP (0) an ascent down.
        let dy = match align & 24 {
            24 => 0.0,
            8 => -0.2 * size,
            _ => 0.8 * size,
        };
        // y-down box: the GDI y axis may point either way after the
        // transform; text stays upright.
        let rot = -escapement / 10.0;
        let rot_attr = if rot.abs() > 1e-6 {
            format!(r#" transform="rotate({rot:.3} {:.4} {:.4})""#, p.0, p.1)
        } else {
            String::new()
        };
        let esc = s
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let _ = writeln!(
            self.out,
            r#"<text x="{:.4}" y="{:.4}" font-family="{}, sans-serif" font-size="{size:.4}"{}{} text-anchor="{anchor}" fill="{}"{rot_attr}{} xml:space="preserve">{esc}</text>"#,
            p.0,
            p.1 + dy,
            face.replace('"', ""),
            if bold { r#" font-weight="bold""# } else { "" },
            if italic {
                r#" font-style="italic""#
            } else {
                ""
            },
            hex(self.dc.text_color),
            self.clip_attr()
        );
    }
}

/// A packed DIB (BITMAPINFO + bits) → PNG bytes, through the BMP codec.
fn dib_to_png(bmi: &[u8], bits: &[u8]) -> Option<Vec<u8>> {
    let mut bmp = Vec::with_capacity(14 + bmi.len() + bits.len());
    let total = (14 + bmi.len() + bits.len()) as u32;
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&total.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&((14 + bmi.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(bmi);
    bmp.extend_from_slice(bits);
    bmp_to_png(&bmp)
}

/// A BMP file → PNG bytes (`ForeignType='Bitmap'` without a
/// CompressionType stores a plain BMP; SVG viewers cannot show BMP).
pub fn bmp_to_png(bmp: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(bmp, image::ImageFormat::Bmp).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Replay an EMF into SVG elements filling the box `(0, -h)…(w, 0)` (the
/// shape's local, y-down frame). `None` when the bytes are no EMF.
pub fn emf_to_svg(data: &[u8], w: f64, h: f64) -> Option<String> {
    let r = Rd(data);
    if r.u32(0)? != 1 || r.u32(40)? != 0x464D_4520 {
        return None;
    }
    let frame = [r.i32(24)?, r.i32(28)?, r.i32(32)?, r.i32(36)?].map(f64::from);
    let dev = (f64::from(r.i32(72)?), f64::from(r.i32(76)?));
    let mm = (f64::from(r.i32(80)?), f64::from(r.i32(84)?));
    let ppmm = (
        if mm.0 > 0.0 { dev.0 / mm.0 } else { 3.78 },
        if mm.1 > 0.0 { dev.1 / mm.1 } else { 3.78 },
    );
    // rclFrame is in 0.01 mm: the picture's device rect.
    let (fl, ft) = (frame[0] / 100.0 * ppmm.0, frame[1] / 100.0 * ppmm.1);
    let (fr, fb) = (frame[2] / 100.0 * ppmm.0, frame[3] / 100.0 * ppmm.1);
    let (fw, fh) = ((fr - fl).max(1e-6), (fb - ft).max(1e-6));
    let dev_to_box = Xf {
        m11: w / fw,
        m12: 0.0,
        m21: 0.0,
        m22: h / fh,
        dx: -fl * w / fw,
        dy: -h - ft * h / fh,
    };
    let mut p = Player {
        out: String::new(),
        defs: String::new(),
        dc: Dc {
            world: Xf::ID,
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            pen: stock(7)?,
            brush: stock(0)?,
            font: default_font(),
            text_color: (0, 0, 0),
            text_align: 0,
            even_odd: true,
            clip: None,
            cur: (0.0, 0.0),
        },
        saved: Vec::new(),
        objs: Vec::new(),
        path: None,
        dev_to_box,
        dev_px_per_mm: ppmm,
        clip_seq: 0,
    };
    let mut off = 0usize;
    let mut guard = 0usize;
    while off + 8 <= data.len() && guard < 2_000_000 {
        guard += 1;
        let ty = r.u32(off)?;
        let size = r.u32(off + 4)? as usize;
        if size < 8 || off + size > data.len() {
            break;
        }
        let rec = Rd(&data[off..off + size]);
        play(&mut p, ty, &rec);
        if ty == 14 {
            break;
        }
        off += size;
    }
    let mut svg = String::new();
    if !p.defs.is_empty() {
        let _ = writeln!(svg, "<defs>{}</defs>", p.defs);
    }
    // The picture never paints outside its frame.
    let id = format!("emfbox{:x}", crate::svg::fnv(&p.out));
    let _ = writeln!(
        svg,
        r#"<clipPath id="{id}"><rect x="0" y="{:.5}" width="{w:.5}" height="{h:.5}"/></clipPath><g clip-path="url(#{id})">"#,
        -h
    );
    svg.push_str(&p.out);
    svg.push_str("</g>\n");
    Some(svg)
}

fn play(p: &mut Player, ty: u32, r: &Rd) {
    let ip = |o: usize| -> (f64, f64) {
        (
            f64::from(r.i32(o).unwrap_or(0)),
            f64::from(r.i32(o + 4).unwrap_or(0)),
        )
    };
    match ty {
        9 => p.dc.win_ext = ip(8),
        10 => p.dc.win_org = ip(8),
        11 => p.dc.vp_ext = ip(8),
        12 => p.dc.vp_org = ip(8),
        17 => p.dc.map_mode = r.u32(8).unwrap_or(1),
        19 => p.dc.even_odd = r.u32(8) != Some(2),
        22 => p.dc.text_align = r.u32(8).unwrap_or(0),
        24 => p.dc.text_color = r.color(8).unwrap_or((0, 0, 0)),
        27 => {
            p.dc.cur = ip(8);
            let m = format!("M {} ", p.pt(p.dc.cur));
            if let Some(path) = p.path.as_mut() {
                path.push_str(&m);
            }
        }
        33 => p.saved.push(p.dc.clone()),
        34 => {
            let rel = r.i32(8).unwrap_or(-1);
            let n = if rel < 0 { (-rel) as usize } else { 1 };
            for _ in 1..n {
                p.saved.pop();
            }
            if let Some(d) = p.saved.pop() {
                p.dc = d;
            }
        }
        35 => {
            if let Some(x) = r.xform(8) {
                p.dc.world = x;
            }
        }
        36 => {
            if let Some(x) = r.xform(8) {
                p.dc.world = match r.u32(32).unwrap_or(1) {
                    1 => Xf::ID,
                    2 => x.then(&p.dc.world),
                    3 => p.dc.world.then(&x),
                    _ => x,
                };
            }
        }
        37 => {
            let ih = r.u32(8).unwrap_or(0);
            let obj = if ih & 0x8000_0000 != 0 {
                stock(ih & 0x7FFF_FFFF)
            } else {
                p.objs.get(ih as usize).cloned().flatten()
            };
            match obj {
                Some(o @ Obj::Pen { .. }) => p.dc.pen = o,
                Some(o @ Obj::Brush { .. }) => p.dc.brush = o,
                Some(o @ Obj::Font { .. }) => p.dc.font = o,
                None => {}
            }
        }
        38 | 39 | 82 | 95 => {
            let ih = r.u32(8).unwrap_or(0) as usize;
            let obj = match ty {
                38 => Obj::Pen {
                    null: r.u32(12).unwrap_or(0) & 0xF == 5,
                    width: f64::from(r.i32(16).unwrap_or(0)),
                    color: r.color(24).unwrap_or((0, 0, 0)),
                    cosmetic: r.i32(16).unwrap_or(0) <= 1,
                    cap: "round",
                    join: "round",
                },
                39 => Obj::Brush {
                    null: r.u32(12) == Some(1),
                    color: r.color(16).unwrap_or((0, 0, 0)),
                },
                82 => {
                    let face: Vec<u16> = (0..32)
                        .filter_map(|i| r.0.get(40 + i * 2..42 + i * 2))
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|&c| c != 0)
                        .collect();
                    Obj::Font {
                        height: f64::from(r.i32(12).unwrap_or(12)),
                        escapement: f64::from(r.i32(20).unwrap_or(0)),
                        bold: r.i32(28).unwrap_or(400) >= 600,
                        italic: r.0.get(32).is_some_and(|&b| b != 0),
                        face: String::from_utf16_lossy(&face),
                    }
                }
                _ => {
                    let style = r.u32(28).unwrap_or(0);
                    Obj::Pen {
                        null: style & 0xF == 5 || r.u32(36) == Some(1),
                        width: f64::from(r.u32(32).unwrap_or(0)),
                        color: r.color(40).unwrap_or((0, 0, 0)),
                        cosmetic: style & 0x000F_0000 == 0,
                        cap: match style & 0xF00 {
                            0x100 => "square",
                            0x200 => "butt",
                            _ => "round",
                        },
                        join: match style & 0xF000 {
                            0x1000 => "bevel",
                            0x2000 => "miter",
                            _ => "round",
                        },
                    }
                }
            };
            if p.objs.len() <= ih {
                p.objs.resize(ih + 1, None);
            }
            p.objs[ih] = Some(obj);
        }
        40 => {
            let ih = r.u32(8).unwrap_or(0) as usize;
            if let Some(o) = p.objs.get_mut(ih) {
                *o = None;
            }
        }
        59 => p.path = Some(String::new()),
        60 => {}
        61 => {
            if let Some(d) = p.path.as_mut() {
                d.push_str("Z ");
            }
        }
        62..=64 => {
            let d = p.path.take().unwrap_or_default();
            p.emit(&d, ty != 64, ty != 62);
        }
        67 => {
            let d = p.path.take().unwrap_or_default();
            if !d.trim().is_empty() {
                p.select_clip(&d);
            }
        }
        30 => {
            let (a, b) = (ip(8), ip(16));
            let d = p.poly_d(&[a, (b.0, a.1), b, (a.0, b.1)], true);
            p.select_clip(&d);
        }
        75 => {
            // EXTSELECTCLIPRGN with RGN_COPY and no region resets the clip.
            if r.u32(8) == Some(0) && r.u32(12) == Some(5) {
                p.dc.clip = None;
            }
        }
        54 => {
            let to = ip(8);
            let seg = format!("L {} ", p.pt(to));
            if let Some(path) = p.path.as_mut() {
                path.push_str(&seg);
            } else {
                let d = format!("M {} L {} ", p.pt(p.dc.cur), p.pt(to));
                p.emit(&d, false, true);
            }
            p.dc.cur = to;
        }
        // Polygons / polylines / beziers, 32-bit (3,4,2) and 16-bit (86,87,85).
        2 | 3 | 4 | 85 | 86 | 87 => {
            let short = ty >= 85;
            let pts = Player::points(r, 24, 28, short);
            if pts.is_empty() {
                return;
            }
            let d = match ty {
                2 | 85 => p.bezier_d(Some(pts[0]), &pts[1..]),
                3 | 86 => p.poly_d(&pts, true),
                _ => p.poly_d(&pts, false),
            };
            let fill = matches!(ty, 3 | 86);
            p.add_path_or_draw(d, fill, true);
            if let Some(l) = pts.last() {
                p.dc.cur = *l;
            }
        }
        // ...To variants continue from the current position.
        5 | 6 | 88 | 89 => {
            let short = ty >= 88;
            let pts = Player::points(r, 24, 28, short);
            if pts.is_empty() {
                return;
            }
            let in_path = p.path.is_some();
            let start = if in_path { None } else { Some(p.dc.cur) };
            let d = match ty {
                5 | 88 => p.bezier_d(start, &pts),
                _ => {
                    let mut d = String::new();
                    if let Some(s) = start {
                        let _ = write!(d, "M {} ", p.pt(s));
                    }
                    for q in &pts {
                        let _ = write!(d, "L {} ", p.pt(*q));
                    }
                    d
                }
            };
            p.add_path_or_draw(d, false, true);
            p.dc.cur = *pts.last().unwrap();
        }
        // PolyPolygon / PolyPolyline: Bounds, nPolys, cpts, counts, points.
        7 | 8 | 90 | 91 => {
            let short = ty >= 90;
            let n = r.u32(24).unwrap_or(0) as usize;
            let counts: Vec<usize> = (0..n)
                .filter_map(|i| r.u32(32 + i * 4).map(|c| c as usize))
                .collect();
            let mut o = 32 + n * 4;
            let step = if short { 4 } else { 8 };
            let close = matches!(ty, 8 | 91);
            let mut d = String::new();
            for c in counts {
                let pts: Vec<(f64, f64)> = (0..c)
                    .filter_map(|i| {
                        let q = o + i * step;
                        if short {
                            Some((f64::from(r.i16(q)?), f64::from(r.i16(q + 2)?)))
                        } else {
                            Some((f64::from(r.i32(q)?), f64::from(r.i32(q + 4)?)))
                        }
                    })
                    .collect();
                o += c * step;
                d.push_str(&p.poly_d(&pts, close));
                d.push(' ');
            }
            p.add_path_or_draw(d, close, true);
        }
        42..=44 => {
            let (a, b) = (ip(8), ip(16));
            let d = if ty == 42 {
                // Ellipse inscribed in the rect: four quarter beziers.
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let (rx, ry) = ((b.0 - a.0) / 2.0, (b.1 - a.1) / 2.0);
                let k = 0.552_284_75;
                let pts = [
                    (cx + rx, cy),
                    (cx + rx, cy + ry * k),
                    (cx + rx * k, cy + ry),
                    (cx, cy + ry),
                    (cx - rx * k, cy + ry),
                    (cx - rx, cy + ry * k),
                    (cx - rx, cy),
                    (cx - rx, cy - ry * k),
                    (cx - rx * k, cy - ry),
                    (cx, cy - ry),
                    (cx + rx * k, cy - ry),
                    (cx + rx, cy - ry * k),
                    (cx + rx, cy),
                ];
                p.bezier_d(Some(pts[0]), &pts[1..]) + "Z"
            } else {
                p.poly_d(&[a, (b.0, a.1), b, (a.0, b.1)], true)
            };
            p.add_path_or_draw(d, true, true);
        }
        81 => {
            // STRETCHDIBITS
            let dest = [
                f64::from(r.i32(24).unwrap_or(0)),
                f64::from(r.i32(28).unwrap_or(0)),
                f64::from(r.i32(72).unwrap_or(0)),
                f64::from(r.i32(76).unwrap_or(0)),
            ];
            let g = |o| r.u32(o).unwrap_or(0) as usize;
            p.bitmap(r, g(48), g(52), g(56), g(60), dest);
        }
        76 => {
            // BITBLT: a bitmap, or (no source) a pattern fill of the rect.
            let dest = [
                f64::from(r.i32(24).unwrap_or(0)),
                f64::from(r.i32(28).unwrap_or(0)),
                f64::from(r.i32(32).unwrap_or(0)),
                f64::from(r.i32(36).unwrap_or(0)),
            ];
            let g = |o| r.u32(o).unwrap_or(0) as usize;
            if g(88) > 0 {
                p.bitmap(r, g(84), g(88), g(92), g(96), dest);
            } else if r.u32(40) == Some(0x00F0_0021) {
                let (a, b) = ((dest[0], dest[1]), (dest[0] + dest[2], dest[1] + dest[3]));
                let d = p.poly_d(&[a, (b.0, a.1), b, (a.0, b.1)], true);
                p.emit(&d, true, false);
            }
        }
        84 => p.text(r),
        45 => {
            let d = p.arc_d(ip(8), ip(16), ip(24), ip(32));
            p.add_path_or_draw(d, false, true);
        }
        _ => {}
    }
}

/// Replay a Windows Metafile (WMF — Visio stores some legacy stencil
/// icons as WMF bytes under `ForeignType='EnhMetaFile'`) into the box
/// `(0,-h)…(w,0)`. The FIRST window origin/extent defines the picture
/// frame (the player's fixed viewport); later SETWINDOW* records rescale
/// against it, as GDI playback does.
pub fn wmf_to_svg(data: &[u8], w: f64, h: f64) -> Option<String> {
    let r = Rd(data);
    let mut start = 0usize;
    if r.u32(0)? == 0x9AC6_CDD7 {
        start = 22; // placeable header
    }
    let hdr_words = usize::from(r.i16(start + 2)? as u16);
    if r.i16(start)? as u16 > 2 || hdr_words != 9 {
        return None;
    }
    let recs = wmf_records(data, start + hdr_words * 2);
    // Pre-scan: the first window origin/extent.
    let first = |f: u16| {
        recs.iter()
            .find(|(func, _, _)| *func == f)
            .and_then(|(_, o, _)| Some((f64::from(r.i16(o + 8)?), f64::from(r.i16(o + 6)?))))
    };
    let (org, ext) = (first(0x020B), first(0x020C));
    let (org, ext) = (org.unwrap_or((0.0, 0.0)), ext?);
    if ext.0.abs() < 1e-9 || ext.1.abs() < 1e-9 {
        return None;
    }
    let mut p = Player {
        out: String::new(),
        defs: String::new(),
        dc: Dc {
            world: Xf::ID,
            map_mode: 8,
            win_org: org,
            win_ext: ext,
            vp_org: org,
            vp_ext: ext,
            pen: stock(7)?,
            brush: stock(0)?,
            font: default_font(),
            text_color: (0, 0, 0),
            text_align: 0,
            even_odd: true,
            clip: None,
            cur: (0.0, 0.0),
        },
        saved: Vec::new(),
        objs: Vec::new(),
        path: None,
        // Device space = the first window: map it onto the box.
        dev_to_box: Xf {
            m11: w / ext.0,
            m12: 0.0,
            m21: 0.0,
            m22: h / ext.1,
            dx: -org.0 * w / ext.0,
            dy: -h - org.1 * h / ext.1,
        },
        dev_px_per_mm: (3.78, 3.78),
        clip_seq: 0,
    };
    for (func, o, size) in recs {
        play_wmf(&mut p, func, &Rd(&data[o..o + size]));
    }
    let mut svg = String::new();
    if !p.defs.is_empty() {
        let _ = writeln!(svg, "<defs>{}</defs>", p.defs);
    }
    let id = format!("wmfbox{:x}", crate::svg::fnv(&p.out));
    let _ = writeln!(
        svg,
        r#"<clipPath id="{id}"><rect x="0" y="{:.5}" width="{w:.5}" height="{h:.5}"/></clipPath><g clip-path="url(#{id})">"#,
        -h
    );
    svg.push_str(&p.out);
    svg.push_str("</g>\n");
    Some(svg)
}

/// The WMF record list after the header: (function, offset, size).
fn wmf_records(data: &[u8], mut o: usize) -> Vec<(u16, usize, usize)> {
    let r = Rd(data);
    let mut out = Vec::new();
    while o + 6 <= data.len() && out.len() < 2_000_000 {
        let (Some(sz), Some(func)) = (r.u32(o), r.i16(o + 4)) else {
            break;
        };
        let (func, size) = (func as u16, sz as usize * 2);
        if size < 6 || o + size > data.len() {
            break;
        }
        out.push((func, o, size));
        if func == 0 {
            break;
        }
        o += size;
    }
    out
}

fn play_wmf(p: &mut Player, func: u16, r: &Rd) {
    // Parameters start at byte 6; many records list (y, x).
    let w = |k: usize| f64::from(r.i16(6 + k * 2).unwrap_or(0));
    let yx = |k: usize| (w(k + 1), w(k));
    let new_obj = |p: &mut Player, o: Option<Obj>| {
        // WMF objects take the lowest free slot.
        let slot = p
            .objs
            .iter()
            .position(Option::is_none)
            .unwrap_or(p.objs.len());
        if slot == p.objs.len() {
            p.objs.push(None);
        }
        // A placeholder keeps the slot taken for unsupported kinds.
        p.objs[slot] = Some(o.unwrap_or(Obj::Brush {
            color: (0, 0, 0),
            null: true,
        }));
    };
    match func {
        0x020B => p.dc.win_org = yx(0),
        0x020C => p.dc.win_ext = yx(0),
        0x0106 => p.dc.even_odd = w(0) as i64 != 2,
        0x0209 => p.dc.text_color = r.color(6).unwrap_or((0, 0, 0)),
        0x012E => p.dc.text_align = w(0) as u32,
        0x001E => p.saved.push(p.dc.clone()),
        0x0127 => {
            let rel = w(0) as i64;
            let n = if rel < 0 { (-rel) as usize } else { 1 };
            for _ in 1..n {
                p.saved.pop();
            }
            if let Some(d) = p.saved.pop() {
                p.dc = d;
            }
        }
        0x02FA => new_obj(
            p,
            Some(Obj::Pen {
                null: (w(0) as i64) & 0xF == 5,
                width: w(1),
                color: r.color(10).unwrap_or((0, 0, 0)),
                cosmetic: w(1) <= 1.0,
                cap: "round",
                join: "round",
            }),
        ),
        0x02FC => new_obj(
            p,
            Some(Obj::Brush {
                null: w(0) as i64 == 1,
                color: r.color(8).unwrap_or((0, 0, 0)),
            }),
        ),
        0x02FB => new_obj(
            p,
            Some(Obj::Font {
                height: w(0),
                escapement: w(2),
                bold: w(4) >= 600.0,
                italic: r.0.get(16).is_some_and(|&b| b != 0),
                face: r
                    .0
                    .get(24..56)
                    .map(|b| {
                        String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[]))
                            .into_owned()
                    })
                    .unwrap_or_else(|| "Arial".into()),
            }),
        ),
        // Other object-creating records still occupy a slot.
        0x00F7 | 0x01F9 | 0x0142 | 0x06FF => new_obj(p, None),
        0x012D => {
            let i = w(0) as usize;
            match p.objs.get(i).cloned().flatten() {
                Some(o @ Obj::Pen { .. }) => p.dc.pen = o,
                Some(o @ Obj::Brush { .. }) => p.dc.brush = o,
                Some(o @ Obj::Font { .. }) => p.dc.font = o,
                None => {}
            }
        }
        0x01F0 => {
            let i = w(0) as usize;
            if let Some(o) = p.objs.get_mut(i) {
                *o = None;
            }
        }
        0x0214 => p.dc.cur = yx(0),
        0x0213 => {
            let to = yx(0);
            let d = format!("M {} L {} ", p.pt(p.dc.cur), p.pt(to));
            p.emit(&d, false, true);
            p.dc.cur = to;
        }
        0x0324 | 0x0325 => {
            let n = w(0) as usize;
            let pts: Vec<(f64, f64)> = (0..n).map(|i| (w(1 + i * 2), w(2 + i * 2))).collect();
            let close = func == 0x0324;
            let d = p.poly_d(&pts, close);
            p.emit(&d, close, true);
        }
        0x0538 => {
            let n = w(0) as usize;
            let counts: Vec<usize> = (0..n).map(|i| w(1 + i) as usize).collect();
            let mut k = 1 + n;
            let mut d = String::new();
            for c in counts {
                let pts: Vec<(f64, f64)> =
                    (0..c).map(|i| (w(k + i * 2), w(k + 1 + i * 2))).collect();
                k += c * 2;
                d.push_str(&p.poly_d(&pts, true));
                d.push(' ');
            }
            p.emit(&d, true, true);
        }
        // RECTANGLE / ELLIPSE: bottom, right, top, left.
        0x041B | 0x0418 => {
            let (a, b) = ((w(3), w(2)), (w(1), w(0)));
            let d = if func == 0x0418 {
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let (rx, ry) = ((b.0 - a.0) / 2.0, (b.1 - a.1) / 2.0);
                let k = 0.552_284_75;
                let pts = [
                    (cx + rx, cy),
                    (cx + rx, cy + ry * k),
                    (cx + rx * k, cy + ry),
                    (cx, cy + ry),
                    (cx - rx * k, cy + ry),
                    (cx - rx, cy + ry * k),
                    (cx - rx, cy),
                    (cx - rx, cy - ry * k),
                    (cx - rx * k, cy - ry),
                    (cx, cy - ry),
                    (cx + rx * k, cy - ry),
                    (cx + rx, cy - ry * k),
                    (cx + rx, cy),
                ];
                p.bezier_d(Some(pts[0]), &pts[1..]) + "Z"
            } else {
                p.poly_d(&[a, (b.0, a.1), b, (a.0, b.1)], true)
            };
            p.emit(&d, true, true);
        }
        // ARC: yEnd, xEnd, yStart, xStart, bottom, right, top, left.
        0x0817 => {
            let d = p.arc_d((w(7), w(6)), (w(5), w(4)), (w(3), w(2)), (w(1), w(0)));
            p.emit(&d, false, true);
        }
        // INTERSECTCLIPRECT: bottom, right, top, left.
        0x0416 => {
            let (a, b) = ((w(3), w(2)), (w(1), w(0)));
            let d = p.poly_d(&[a, (b.0, a.1), b, (a.0, b.1)], true);
            p.select_clip(&d);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal EMF: header (frame 0..1000 × 0..1000 in 0.01 mm on a
    /// 100 px / 10 mm device), a red solid brush, a 16-bit polygon, EOF.
    fn tiny_emf() -> Vec<u8> {
        let mut d = Vec::new();
        let mut rec = |ty: u32, body: &[u8]| {
            let size = (8 + body.len()) as u32;
            d.extend_from_slice(&ty.to_le_bytes());
            d.extend_from_slice(&size.to_le_bytes());
            d.extend_from_slice(body);
        };
        let mut h = vec![0u8; 100];
        let put = |h: &mut Vec<u8>, o: usize, v: i32| h[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // offsets inside the record body = record offset − 8
        put(&mut h, 16, 0);
        put(&mut h, 20, 0);
        put(&mut h, 24, 1000);
        put(&mut h, 28, 1000);
        put(&mut h, 32, 0x464D_4520);
        put(&mut h, 64, 100);
        put(&mut h, 68, 100);
        put(&mut h, 72, 10);
        put(&mut h, 76, 10);
        rec(1, &h);
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes()); // ihBrush
        b.extend_from_slice(&0u32.to_le_bytes()); // BS_SOLID
        b.extend_from_slice(&[255, 0, 0, 0]); // red
        b.extend_from_slice(&0u32.to_le_bytes());
        rec(39, &b);
        rec(37, &1u32.to_le_bytes());
        let mut poly = vec![0u8; 16];
        poly.extend_from_slice(&3u32.to_le_bytes());
        for (x, y) in [(0i16, 0i16), (100, 0), (100, 100)] {
            poly.extend_from_slice(&x.to_le_bytes());
            poly.extend_from_slice(&y.to_le_bytes());
        }
        rec(86, &poly);
        rec(14, &[0u8; 12]);
        d
    }

    #[test]
    fn polygon_maps_the_frame_onto_the_box() {
        let svg = emf_to_svg(&tiny_emf(), 2.0, 1.0).expect("an EMF");
        // Device 100 px over 10 mm = 10 px/mm; frame 10 mm = 100 px →
        // the box is 2 × 1 in: (100, 100) lands at (2, 0).
        assert!(svg.contains(r##"fill="#ff0000""##), "{svg}");
        assert!(
            svg.contains("M 0.0000 -1.0000 L 2.0000 -1.0000 L 2.0000 0.0000 Z"),
            "{svg}"
        );
    }

    #[test]
    fn non_emf_bytes_are_rejected() {
        assert!(emf_to_svg(b"not an emf at all, definitely not", 1.0, 1.0).is_none());
    }

    /// A minimal WMF: header, window org (0,0) ext (100,50), a green
    /// brush, a polygon over the full window.
    #[test]
    fn wmf_window_maps_onto_the_box() {
        let mut d: Vec<u8> = Vec::new();
        for v in [1u16, 9, 0x0300] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u32.to_le_bytes()); // size (unused)
        d.extend_from_slice(&1u16.to_le_bytes()); // objects
        d.extend_from_slice(&0u32.to_le_bytes()); // max record
        d.extend_from_slice(&0u16.to_le_bytes());
        let mut rec = |func: u16, params: &[i16]| {
            let words = 3 + params.len() as u32;
            d.extend_from_slice(&words.to_le_bytes());
            d.extend_from_slice(&func.to_le_bytes());
            for p in params {
                d.extend_from_slice(&p.to_le_bytes());
            }
        };
        rec(0x020B, &[0, 0]); // SETWINDOWORG y, x
        rec(0x020C, &[50, 100]); // SETWINDOWEXT y, x
        rec(0x02FC, &[0, -256, 0, 0]); // brush: solid, COLORREF bytes 00 FF 00 00 → green
        rec(0x012D, &[0]); // select it
        rec(0x0324, &[3, 0, 0, 100, 0, 100, 50]); // polygon
        rec(0x0000, &[]);
        let svg = wmf_to_svg(&d, 2.0, 1.0).expect("a WMF");
        assert!(svg.contains(r##"fill="#00ff00""##), "{svg}");
        assert!(
            svg.contains("M 0.0000 -1.0000 L 2.0000 -1.0000 L 2.0000 0.0000 Z"),
            "{svg}"
        );
    }
}
