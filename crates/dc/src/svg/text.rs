//! Text: block resolution, greeking, shaping-driven wrap, run
//! styling, and SVG emission, plus the Character/font cell cascade.
use std::fmt::Write as _;
use super::*;
/// Text renders as glyphs only while its on-screen em height clears
/// this many pixels; below it the canvas layer path emits gray greek
/// bars instead (GitHub #124 item 2 — Visio greeks text at low zoom,
/// and the bars skip the font resolve / glyph-outline work the usvg
/// parse would otherwise pay for text nobody can read). Export, print,
/// and stencil thumbnails never greek: their calls pass no ppi.
pub const TEXT_GREEK_PX: f64 = 4.0;

/// Count of the recorded text sizes (inches, one entry per text block
/// a layer serialization met) that would greek at `ppi`. The canvas
/// layer cache compares this across ppi-bucket changes: equal counts
/// mean the same blocks greeked on both sides (greeked-ness is
/// monotone in size, so the greeked set is always a prefix), i.e. the
/// retained SVG/tree serializes identically and the re-render can
/// skip the re-parse (#124 item 2 + item 6.3).
pub fn greeked_count(sizes: &[f64], ppi: f32) -> usize {
    sizes
        .iter()
        .filter(|&&s| s * (ppi as f64) < TEXT_GREEK_PX)
        .count()
}

/// Greeking context for canvas-layer serializations (#124 item 2).
/// `Some` only on the `render_shape_layer_opts` path; the ppi decides
/// per text block whether it emits glyphs or bars, and every text
/// size met is recorded so the layer cache can build its re-serialize
/// profile (see [`greeked_count`]).
pub(crate) struct GreekCtx {
    ppi: f32,
    pub(crate) sizes: std::cell::RefCell<Vec<f64>>,
}

impl GreekCtx {
    pub(crate) fn new(ppi: f32) -> Self {
        Self {
            ppi,
            sizes: std::cell::RefCell::new(Vec::new()),
        }
    }

    fn greeks(&self, size_in: f64) -> bool {
        size_in * (self.ppi as f64) < TEXT_GREEK_PX
    }
}

/// Text: laid out inside the TxtPin/TxtLocPin/TxtWidth/TxtHeight block —
/// word-wrapped to the block width, horizontally aligned per the Paragraph
/// row's HorizAlign, vertically per VertAlign, with the first Character
/// row's size/color/bold. Rotated by TxtAngle around the pin (Visio
/// vertical sidebars use TxtAngle 90).
///
/// `sx`/`sy` are the flip factors the caller's shape transform applied
/// (±1). Visio flips mirror geometry but never the text block: the block
/// keeps its TxtPin placement inside the flipped frame, yet the glyphs
/// must stay un-mirrored, and a TxtAngle rotation inside a mirrored frame
/// inverts its page-space sense — the connector masters author
/// `TxtAngle = IF(BITXOR(FlipX,FlipY),1,-1)*Angle` against exactly that,
/// so the page-space net is `Angle + sx*sy*TxtAngle` (0 = horizontal
/// labels, issue #19).
// The eight parameters are the full text-emission context; grouping
// them would just move the long call sites into a struct nobody else
// shares.
#[allow(clippy::too_many_arguments)]
/// A shape's resolved text block: placement in the shape's LOCAL frame
/// (inches, y-up) plus the styling the renderer paints it with — the
/// single source of truth shared by [`emit_text`] (the SVG backend),
/// the GUI's in-place editor overlay (GitHub #22), and tests, so the
/// editing view cannot drift from the rendered result.
///
/// Placement follows the sheet's `Txt*` cells with the same defaults
/// `emit_text` applies: pin at the shape's center, size = the shape's
/// Width/Height, LocPin centered in the block. Styling resolves
/// through the instance → master chain exactly like the renderer
/// (Character `Font`/`Size`/`Color`/`Style`, Paragraph `HorzAlign`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextBlock {
    /// `TxtPinX`/`TxtPinY` — the block's pin in the shape's local frame.
    pub pin: (f64, f64),
    /// `TxtWidth`/`TxtHeight` (clamped to ≥ 0.02 in like `emit_text`).
    pub size: (f64, f64),
    /// `TxtLocPinX`/`TxtLocPinY` — the pin's offset from the block's
    /// lower-left corner.
    pub loc_pin: (f64, f64),
    /// `TxtAngle` (radians, local).
    pub angle: f64,
    /// Text-block insets (left, right, top, bottom) — Visio defaults
    /// 0.1 in / 0.1 in / 0.0555 in / 0.0555 in.
    pub insets: (f64, f64, f64, f64),
    /// Line height in inches (`LineSpacing` × font size, default 1.2).
    pub line_height: f64,
    /// Character `Font` (family name) if any cell resolves one.
    pub font: Option<String>,
    /// Character `Size` in inches (0.13889 in = 10 pt); default 10 pt.
    pub font_size: f64,
    /// Resolved Character `Color` as `#rrggbb` (palette/theme aware).
    pub color: String,
    /// Character `Style` bit 1.
    pub bold: bool,
    /// Character `Style` bit 2.
    pub italic: bool,
    /// Paragraph `HorzAlign`: 0 left, 1 center (Visio default), 2
    /// right, 3 justify.
    pub halign: i32,
    /// `VertAlign`: 1 top, 2 middle (default), 3 bottom.
    pub valign: i32,
    /// Character `ComplexScriptFont` family (#257 text-direction):
    /// the face complex-script (RTL-base) paragraphs paint with; the
    /// spec's zero/absent semantics resolve to `None` and the Latin
    /// `Font` chain stays. Paragraph granularity — the per-run `<cp>`
    /// model stays out until a fixture shows one (see
    /// Analysis/rtl-text-direction.md).
    pub cs_font: Option<String>,
    /// Effective `ComplexScriptSize` in inches (the spec's `0 → Size`,
    /// `<0 → |v|·Size/100` semantics already applied; `None` = Size).
    pub cs_size: Option<f64>,
    /// Character `Pos` of the first row (0 normal, 1 superscript, 2
    /// subscript — #230). Run-granular Pos (`<cp>` markers, stripped
    /// by the parser) rides the run work in #267; the whole block
    /// follows the first row like every other first-row cell.
    pub pos: i32,
    /// Paragraph `IndFirst` in inches (first-line indent; negative =
    /// hanging indent, #231). Corpus default 0.
    pub ind_first: f64,
    /// Paragraph `IndLeft` in inches — wrapped lines start here
    /// (#231). Corpus default 0.
    pub ind_left: f64,
    /// #231 bullet layout (first Paragraph row granularity);
    /// `None` = not bulleted.
    pub bullet: Option<BulletSpec>,
    /// #229 text background: the resolved fill color and its alpha
    /// (0–255, from TextBkgndTrans's fractional transparency). `None`
    /// paints nothing — TextBkgnd 0/255, or 100 % transparency.
    pub text_bkgnd: Option<(String, u8)>,
}

/// A bulleted block's glyph spec (#231): the built-in round styles
/// 1–7 draw as paths (viewer-proof — a text glyph changes face with
/// the viewer's fonts); a custom bullet string draws as text in its
/// own face.
#[derive(Debug, Clone, PartialEq)]
pub enum BulletGlyph {
    /// Built-in style 1–7 (Word's round set, drawn as • o ▪ ❑ ❖ ➢ ✓).
    Round(u8),
    /// A custom bullet string (drawn as a text glyph).
    Str(String),
}

/// The cells behind one bulleted block (#231, first Paragraph row).
#[derive(Debug, Clone, PartialEq)]
pub struct BulletSpec {
    pub glyph: BulletGlyph,
    /// BulletFont — the bullet's own face family; `None` inherits the
    /// paragraph font (the measured rule: BulletFont 0 inherits).
    pub font: Option<String>,
    /// Bullet size in inches: BulletFontSize < 0 is a percentage of
    /// the text size (default −100 % = the text size); ≥ 0 is an
    /// absolute size in Char.Size inches (the > 100 % case stays an
    /// open question on #231 — unwitnessed).
    pub size: f64,
    /// `TextPosAfterBullet` in inches, ABSOLUTE from the line start —
    /// Visio 2021 does not add it to IndFirst despite the documented
    /// formula (measured on #231: IndFirst 0.25 + TextPosAfterBullet
    /// 0.1 starts the text at the 0.5 tab stop, not 0.35).
    pub pos_after: f64,
    /// `DefaultTabStop` (document sheet; 0.5 in default) — the
    /// tab-stop grid the after-bullet text snaps to.
    pub tab_stop: f64,
}

/// Resolve one shape's [`TextBlock`] through the instance → master
/// chain (GitHub #22): the same cell reads and defaults `emit_text`
/// performs, hoisted so the GUI overlay and the renderer agree.
pub fn text_block(doc: &Document, page: &vsdx::Page, shape_id: i64) -> Option<TextBlock> {
    let chain = dc_core::scope::shape_chain(doc, page.id, shape_id).ok()?;
    let s = *chain.last()?;
    let env = shape_env(doc, s, &chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    Some(resolve_text_block(doc, s, &chain, width, height))
}

/// The shape's text block in PAGE coordinates (inches, y-up, origin
/// bottom-left — [`shape_page_rect`]'s convention): the block's local
/// rect mapped through the ancestor XForm chain. The shape's own
/// `Angle`/`TxtAngle` are not applied (v1 of the in-place editor,
/// GitHub #22 — a rotated shape yields the axis-aligned box).
pub fn text_block_page_rect(
    doc: &Document,
    page: &vsdx::Page,
    shape_id: i64,
) -> Option<(f64, f64, f64, f64)> {
    let page_h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);
    let chain = dc_core::scope::shape_chain(doc, page.id, shape_id).ok()?;
    let s = *chain.last()?;
    let env = shape_env(doc, s, &chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    let tb = resolve_text_block(doc, s, &chain, width, height);
    // Shape-local → parent-local (v1, unrotated: the same
    // pin-anchored placement `shape_page_rect` uses for the shape
    // rect) → page through the ancestor chain.
    let (pin_x, pin_y) = (get("PinX").unwrap_or(0.0), get("PinY").unwrap_or(0.0));
    let (loc_x, loc_y) = (
        get("LocPinX").unwrap_or(width / 2.0),
        get("LocPinY").unwrap_or(height / 2.0),
    );
    let m = ancestors_affine(doc, &chain, page_h);
    let nested = chain.len() >= 2;
    let corner = |x: f64, y: f64| -> (f64, f64) {
        let iy = if nested { -y } else { page_h - y };
        let (sx, sy) = m.apply(x, iy);
        (sx, page_h - sy)
    };
    let (tx, ty) = tb.pin;
    let (tw, th) = tb.size;
    let (lpx, lpy) = tb.loc_pin;
    let bx = pin_x - loc_x + tx - lpx;
    let by = pin_y - loc_y + ty - lpy;
    let pts = [
        corner(bx, by),
        corner(bx + tw, by),
        corner(bx, by + th),
        corner(bx + tw, by + th),
    ];
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    let (min_x, max_x) = (
        xs.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
    );
    let (min_y, max_y) = (
        ys.clone().fold(f64::INFINITY, f64::min),
        ys.fold(f64::NEG_INFINITY, f64::max),
    );
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}

/// The complex-script size an RTL line's glyphs actually paint with
/// (#257 text-direction, PR #269 review): the resolved
/// ComplexScriptSize (negative-percent semantics already applied)
/// when it EXCEEDS the Latin size — a smaller CS size keeps the
/// block's Latin slot (Visio's LineSpacing is a block cell; only
/// larger glyphs need a taller slot or adjacent lines overlap).
pub(crate) fn effective_cs_size(tb: &TextBlock) -> f64 {
    tb.cs_size.unwrap_or(tb.font_size).max(tb.font_size)
}

/// One wrapped line's vertical slot: the block's line height, scaled
/// by the line's effective size when its glyphs paint larger than the
/// Latin size (RTL lines under a bigger ComplexScriptSize — PR #269
/// review: default spacing is 1.2 em, so 2 em glyphs in 1.2 em slots
/// overlapped).
pub(crate) fn line_slot_height(tb: &TextBlock, rtl: bool) -> f64 {
    if rtl {
        tb.line_height * (effective_cs_size(tb) / tb.font_size)
    } else {
        tb.line_height
    }
}

/// The first baseline's offset below the text-block's TOP edge, in
/// the block's local inches (y-down) — the single placement authority
/// shared by `emit_text` and the GUI's in-place editor overlay, so
/// the editing caret sits on the very line the renderer draws (the
/// #22 review's "no translation between edit and render"). Subsequent
/// lines sit their [`line_slot_height`] lower. `total_h` is the
/// wrapped text's height (the SUM of the line slots), and
/// `first_line_size` is the first line's effective glyph size — a
/// first line of larger complex-script glyphs ascends from a higher
/// baseline than the Latin ascent.
pub fn text_first_baseline(tb: &TextBlock, total_h: f64, first_line_size: f64) -> f64 {
    let (_ins_l, _ins_r, ins_t, ins_b) = tb.insets;
    let th = tb.size.1;
    match tb.valign {
        1 => ins_t + first_line_size * 0.8,                // top
        3 => th - ins_b - total_h + first_line_size * 0.8, // bottom
        _ => (th - total_h) / 2.0 + first_line_size * 0.8, // middle
    }
}

/// The cell resolution behind [`TextBlock`] — shared by `emit_text`
/// (which paints from it) and the public [`text_block`] wrapper.
pub(crate) fn resolve_text_block(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    width: f64,
    height: f64,
) -> TextBlock {
    let env = shape_env(doc, s, chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let tx = get("TxtPinX").unwrap_or(width / 2.0);
    let ty = get("TxtPinY").unwrap_or(height / 2.0);
    let tw = get("TxtWidth").unwrap_or(width).max(0.02);
    let th = get("TxtHeight").unwrap_or(height).max(0.02);
    let angle = get("TxtAngle").unwrap_or(0.0);
    // Char.Size lives in the Character section's first row, stored in
    // inches (0.13889 in = 10 pt); the viewBox unit is inches, so the
    // value is the SVG font-size directly.
    let font_size = char_size(doc, s, chain).unwrap_or(0.13888).max(0.02);
    // First Character row extras: color (palette/theme aware) and the
    // Style bitmask (1 = bold, 2 = italic). Both resolve per-cell through
    // `char_cell`, so a LangID-only instance row picks them up from the
    // master/style rows instead of silently taking the built-in defaults.
    let color = char_cell(doc, s, chain, "Color")
        .map(|c| color_value(doc, c.v.as_deref().unwrap_or(""), c.f.as_deref()))
        .unwrap_or_else(|| "#000000".into());
    let style_bits = char_cell(doc, s, chain, "Style")
        .and_then(|c| c.numeric())
        .unwrap_or(0.0) as u32;

    // Paragraph alignment: Visio HorzAlign 0 = left, 1 = center, 2 = right,
    // 3 = justify (rendered as left). Absent cell → Visio default 1 (center).
    // Sections inherit separately from cells: a sub-shape that carries e.g.
    // Character but no Paragraph still takes Paragraph from its master shape
    // (`MasterShape` link) — effective_sheet stops at the instance sheet
    // once it has any sections, so consult the source explicitly.
    let para_row = s
        .sheet
        .sections_named("Paragraph")
        .into_iter()
        .next()
        .or_else(|| {
            doc.instance_source(s, chain)
                .and_then(|src| src.sheet.sections_named("Paragraph").into_iter().next())
        })
        .and_then(|sec| sec.rows.first());
    let halign = para_row
        .and_then(|r| r.cell("HorzAlign"))
        .and_then(|c| c.numeric())
        .unwrap_or(1.0) as i32;
    // The ShapeSheet cell is `VerticalAlign` (Text Block Format
    // section): 0 top, 1 middle (the Normal-style default), 2 bottom.
    // The block keeps its own 1 top / 2 middle / 3 bottom encoding,
    // so shift by one. (There is no `VertAlign` cell — reading that
    // name always fell back to middle.)
    let valign = get("VerticalAlign").map_or(2, |v| (v.round() as i32).clamp(0, 2) + 1);

    // Text-block insets (Visio defaults: 0.1 in left/right, 0.0555 in
    // top/bottom) and 120% line spacing.
    let ins_l = get("LeftMargin").unwrap_or(0.1);
    let ins_r = get("RightMargin").unwrap_or(0.1);
    let ins_t = get("TopMargin").unwrap_or(0.0555);
    let ins_b = get("BottomMargin").unwrap_or(0.0555);
    let line_height = get("LineSpacing")
        .map(|l| l * font_size)
        .unwrap_or(font_size * 1.2)
        .max(font_size);

    // Complex-script character cells (#257 text-direction, paragraph
    // granularity — see Analysis/rtl-text-direction.md). Size
    // semantics per [MS-VSDX] §2.4.4.62: >0 = the value, 0 = the
    // sibling Size, <0 = |v|·Size/100.
    let cs_font = char_cell(doc, s, chain, "ComplexScriptFont")
        .and_then(|c| c.v.clone())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.parse::<f64>().is_err());
    let cs_size_raw = char_cell(doc, s, chain, "ComplexScriptSize").and_then(|c| c.numeric());
    let cs_size = cs_size_raw.map(|v| {
        if v > 0.0 {
            v
        } else if v < 0.0 {
            v.abs() * font_size / 100.0
        } else {
            font_size
        }
    });

    // Character.Pos (#230): first-row granularity — the parser strips
    // the `<cp>` run markers, so per-run Pos arrives with the run work
    // (#267) and the whole block follows row 0 like color/bold/italic.
    let pos = char_cell(doc, s, chain, "Pos")
        .and_then(|c| c.numeric())
        .unwrap_or(0.0) as i32;

    // Paragraph indents (#231) resolved PER CELL through the
    // instance → master inheritance (PR #284 review): an instance
    // Paragraph row carrying only HorzAlign must not shadow the
    // master's IndFirst/IndLeft/bullet cells, the way a whole-row
    // pick would.
    let para_cell = |name: &str| -> Option<Cell> {
        // The FIRST row of the FIRST Paragraph section per side (PR
        // #284 review, round 2): later sections/rows belong to OTHER
        // paragraphs (the <pp> markers) — scanning them would blend
        // paragraph 2's bullet into paragraph 0's settings. Per-cell
        // instance -> master inheritance is preserved, then the
        // TEXT-STYLE stylesheet cascade follows — the same walk
        // `char_cell` gives Character cells (local CodeRabbit round
        // 3: a text style's IndLeft/Bullet must reach paragraphs the
        // same way its Font/Size reach characters). INHERITED cells
        // (F="Inh") never satisfy a level — their cached V is a
        // snapshot that can be stale against the parent, so the walk
        // skips them exactly like `resolve_cell` does (Greptile PR
        // #284 round 7) and the live parent value wins.
        let first_cell = |sheet: &Sheet| {
            sheet
                .sections_named("Paragraph")
                .first()
                .and_then(|sec| sec.rows.first())
                .and_then(|r| r.cell(name))
                .filter(|c| !c.is_inherited())
                .cloned()
        };
        if let Some(c) = first_cell(&s.sheet) {
            return Some(c);
        }
        let source = doc.instance_source(s, chain);
        if let Some(src) = source {
            if let Some(c) = first_cell(&src.sheet) {
                return Some(c);
            }
        }
        let style_id = s
            .text_style
            .or_else(|| source.and_then(|src| src.text_style))?;
        let mut cur = doc.stylesheet(style_id)?;
        for _ in 0..8 {
            if let Some(c) = first_cell(&cur.sheet) {
                return Some(c);
            }
            let next_id = cur
                .attrs
                .iter()
                .find(|(k, _)| k == "TextStyle")
                .and_then(|(_, v)| v.parse::<i64>().ok())
                .or_else(|| {
                    cur.sheet
                        .cell("TextStyle")
                        .and_then(|c| c.numeric())
                        .map(|v| v as i64)
                })?;
            if next_id == cur.id {
                return None;
            }
            cur = doc.stylesheet(next_id)?;
        }
        None
    };
    let ind_first = para_cell("IndFirst")
        .and_then(|c| c.numeric())
        .unwrap_or(0.0);
    let ind_left = para_cell("IndLeft")
        .and_then(|c| c.numeric())
        .unwrap_or(0.0);

    // #231 bullet machinery: a Bullet cell 1–7 (Word's round set) or a
    // non-numeric string (custom bullet) arms it; 0/empty stays off.
    let bullet = para_cell("Bullet").and_then(|b| {
        let v = b.v.as_deref().unwrap_or("").trim().to_string();
        if v.is_empty() || v == "0" {
            return None;
        }
        // A custom BulletString takes PRECEDENCE over the numeric
        // style ([MS-VSDX], PR #284 review round 3): the string
        // can ride along a nonzero numeric Bullet, and the round
        // set must not swallow it.
        let custom = para_cell("BulletString")
            .and_then(|c| c.v.clone())
            .map(|st| st.trim().to_string())
            .filter(|st| !st.is_empty());
        // Numeric forms classify BEFORE the custom-string
        // fallback (CodeRabbit local review): "256" overflows u8
        // and "1.0"/"0.0" fail an integer parse — either way the
        // old path painted the literal as a custom bullet.
        // Integral 1–7 → the round set; any other NUMERIC value
        // (final CodeRabbit pass: including negatives like "-1"
        // and fractionals like "1.5" — a number that failed the
        // style filter is an unsupported style, not custom text)
        // → no bullet; genuinely non-numeric text → a custom
        // bullet.
        let parsed = v.parse::<f64>().ok();
        let glyph = match (custom, parsed) {
            (Some(st), _) => BulletGlyph::Str(st),
            (None, Some(n)) if n.fract() == 0.0 && (1.0..=7.0).contains(&n) => {
                BulletGlyph::Round(n as u8)
            }
            (None, Some(_)) => return None, // unsupported numerics: no bullet
            (None, None) => BulletGlyph::Str(v),
        };
        // BulletFont: a family name, else inherit the paragraph
        // font (numeric legacy indexes and 0 both inherit).
        let font = para_cell("BulletFont")
            .and_then(|c| c.v.clone())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && v.parse::<f64>().is_err());
        // BulletFontSize: negative = percent of the text size
        // (default −100 %), ≥ 0 = absolute inches.
        let pct = para_cell("BulletFontSize")
            .and_then(|c| c.numeric())
            .unwrap_or(-100.0);
        let size = if pct < 0.0 {
            pct.abs() / 100.0 * font_size
        } else if pct == 0.0 {
            // 0 = the size of the first character after the bullet
            // ([MS-VSDX] BulletFontSize — PR #284 review round 3):
            // the paragraph's own size, not a 0.02 in sliver.
            font_size
        } else {
            pct.max(0.02)
        };
        let pos_after = para_cell("TextPosAfterBullet")
            .and_then(|c| c.numeric())
            .unwrap_or(0.0);
        // The tab-stop grid lives on the document sheet.
        let tab_stop = doc
            .sheet
            .cell("DefaultTabStop")
            .and_then(|c| c.numeric())
            .unwrap_or(0.5)
            .max(0.01);
        Some(BulletSpec {
            glyph,
            font,
            size,
            pos_after,
            tab_stop,
        })
    });

    // #229 TextBkgnd: 0 and 255 paint nothing; 1–24 select palette
    // entries 0–23; larger values index the document palette through
    // the +1 encoding; non-numeric values go through the shared color
    // mapping (theme formulas fall to the cached V, the usual
    // convention). TextBkgndTrans is a fraction like every *Trans
    // cell: the file stores 50 % as V='0.5' (Visio 2021 witness:
    // 0 / 0.5 / 0.8 paint opaque / half / mostly see-through), which
    // renders as 128/255 alpha.
    let text_bkgnd = doc.resolve_cell(s, chain, "TextBkgnd").and_then(|c| {
        let v = c.v.as_deref().unwrap_or("");
        let color = match v.trim().parse::<f64>() {
            Ok(n) => {
                let n = n as i64;
                if n <= 0 || n >= 255 {
                    return None;
                }
                doc.color(n - 1).cloned().unwrap_or_else(|| palette(n - 1))
            }
            Err(_) => color_value(doc, v, c.f.as_deref()),
        };
        let trans = doc
            .resolve_cell(s, chain, "TextBkgndTrans")
            .and_then(|c| c.numeric())
            .unwrap_or(0.0);
        let alpha = ((1.0 - trans).clamp(0.0, 1.0) * 255.0).round() as i64;
        if alpha <= 0 {
            None
        } else {
            Some((color, alpha.min(255) as u8))
        }
    });

    TextBlock {
        pin: (tx, ty),
        size: (tw, th),
        loc_pin: (
            get("TxtLocPinX").unwrap_or(tw / 2.0),
            get("TxtLocPinY").unwrap_or(th / 2.0),
        ),
        angle,
        insets: (ins_l, ins_r, ins_t, ins_b),
        line_height,
        font: char_font(doc, s, chain),
        font_size,
        color,
        bold: style_bits & 1 != 0,
        italic: style_bits & 2 != 0,
        halign,
        valign,
        cs_font,
        cs_size,
        pos,
        ind_first,
        ind_left,
        bullet,
        text_bkgnd,
    }
}

/// The greedy wrap both the renderer (`emit_text`) and the
/// `TEXTHEIGHT` measurement (#147) run — one wrap, so block-size
/// caches and the rendered lines can never disagree.
///
/// Visio stores line breaks as CRLF: breaking on each of \r and \n
/// separately would insert an empty paragraph between every pair of
/// lines, so normalize CRLF (and lone CR) to one break first.
/// Character-exact lines (#144): each paragraph is reconstructed
/// verbatim — `split(' ')` words joined back with their separator
/// spaces — so a paragraph-leading space run (empty first words)
/// survives into the first line, interior runs stay, and a
/// whitespace-only paragraph emits its spaces instead of an empty
/// line. `started` distinguishes "no words consumed yet" from "an
/// empty word consumed" (a pending leading space): a bare
/// `cur.is_empty()` test cannot tell them apart, which is how the
/// leading run used to vanish.
/// One wrapped line plus its paragraph's base direction (#257
/// text-direction): alignment and emission order follow the base, so
/// the wrap must carry it per line (a block can mix LTR and RTL
/// paragraphs). `first_of_para` tags each paragraph's opening line —
/// the #231 bullet and the IndFirst indent ride it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WrapLine {
    pub text: String,
    pub rtl: bool,
    pub first_of_para: bool,
}

/// The Right-to-Left Text toggle's transform (#257 text-direction,
/// PR #269 review): `to_rtl` prepends an explicit U+202B (RLE) to
/// every paragraph; the reverse strips the leading direction marks
/// and then pins paragraphs whose first-strong scan still reads RTL
/// with an explicit U+202A (LRE) — "off" must really read LTR even
/// on naturally-RTL text, or the toggle is a visual no-op for it.
/// CRLF input normalizes to LF like the renderer's own wrap.
pub fn toggle_rtl_marks(text: &str, to_rtl: bool) -> String {
    let paras = text.replace("\r\n", "\n").replace('\r', "\n");
    // A text with NO content at all still takes the mark — it
    // PRE-SETS the direction for the first paragraph typed into an
    // empty shape (PR #269 review); only the trailing empty of a
    // NON-empty text stays bare.
    let pre_empty = paras.is_empty();
    let mut out: Vec<String> = Vec::new();
    for l in paras.split('\n') {
        // Both directions REPLACE the leading mark — stacking an RLE
        // onto an existing LRE pin would leave two overrides.
        let stripped = l.trim_start_matches(['\u{202A}', '\u{202B}', '\u{202C}']);
        if stripped.is_empty() && !pre_empty {
            // An EMPTY paragraph takes no mark: a mark on the trailing
            // terminator's empty paragraph is content to the wrapper
            // (a direction mark is not whitespace, so the renderer's
            // `trim_end` keeps it) and "hello\r\n" would gain a second
            // TEXTHEIGHT line — the corpus's trailing-break-adds-no-
            // line gold (#148) must survive a direction toggle (PR
            // #269 review).
            out.push(String::new());
        } else if to_rtl {
            out.push(format!("\u{202B}{stripped}"));
        } else {
            out.push(if paragraph_base_rtl(stripped) {
                format!("\u{202A}{stripped}")
            } else {
                stripped.to_string()
            });
        }
    }
    out.join("\n")
}

/// A paragraph's base direction (Analysis/rtl-text-direction.md):
/// explicit U+202A/U+202B overrides first, then the first strong
/// bidi character (R/AL → RTL, L → LTR, neutrals and numbers
/// skipped); no strong character → LTR. Public for dc-ui's
/// Right-to-Left Text toggle, which needs the renderer's exact rule
/// when it pins a stripped paragraph LTR (PR #269 review).
pub fn paragraph_base_rtl(para: &str) -> bool {
    let mut chars = para.chars().peekable();
    // Leading explicit embedding marks decide outright.
    while let Some(&c) = chars.peek() {
        match c {
            '\u{202B}' => return true,  // RLE
            '\u{202A}' => return false, // LRE
            '\u{200E}' | '\u{200F}' | '\u{202C}' => {
                chars.next();
            }
            _ => break,
        }
    }
    for c in chars {
        use unicode_bidi::BidiClass as BC;
        match unicode_bidi::bidi_class(c) {
            BC::R | BC::AL => return true,
            BC::L => return false,
            _ => {} // neutrals, EN/AN, marks: keep scanning
        }
    }
    false
}

/// The greedy wrap both the renderer (`emit_text`) and the
/// `TEXTHEIGHT` measurement (#147) run — one wrap, so block-size
/// caches and the rendered lines can never disagree.
///
/// Visio stores line breaks as CRLF: breaking on each of \r and \n
/// separately would insert an empty paragraph between every pair of
/// lines, so normalize CRLF (and lone CR) to one break first.
/// Character-exact lines (#144): each paragraph is reconstructed
/// verbatim — `split(' ')` words joined back with their separator
/// spaces — so a paragraph-leading space run (empty first words)
/// survives into the first line, interior runs stay, and a
/// whitespace-only paragraph emits its spaces instead of an empty
/// line. `started` distinguishes "no words consumed yet" from "an
/// empty word consumed" (a pending leading space): a bare
/// `cur.is_empty()` test cannot tell them apart, which is how the
/// leading run used to vanish. Since #282 the "words" are
/// `textmetrics::wrap_tokens` tokens — the same space-separated
/// words plus CJK break opportunities (see `wrap_block_text`).
/// One wrapped line plus its paragraph's base direction (#257
/// paragraph paints with — an RTL-base paragraph resolves the
/// complex-script cells (`cs_face`/`cs_size`, the Latin pair when
/// absent), so the wrap decision and the raster agree by construction
/// per paragraph, not just per block.
///
/// #282 zh-Hans: BOTH directions measure run-by-run through
/// `fb_chain` — usvg's per-cluster fallback paints a run the primary
/// face lacks with the first chain face covering it (Vazirmatn for
/// the Arabic script, Noto Sans SC for Han), so an LTR paragraph
/// carrying CJK must measure those runs with the same advances or
/// the wrap and the raster disagree. Latin the primary face covers
/// never splits (the chain fast path keeps those widths
/// byte-identical). The wrap itself tokenizes through
/// `textmetrics::wrap_tokens` — spaces as before, plus break
/// opportunities between ideographs under the kinsoku rules.
///
/// #231 paragraph indents: a paragraph's FIRST line wraps at
/// `usable_w − first_off` (the after-bullet tab offset for bulleted
/// paragraphs, `max(IndFirst, 0)` otherwise) and its wrapped lines at
/// `usable_w − left_off` (IndLeft). #235 terminal-NBSP hang: a
/// trailing no-break space never wraps its word — the overflow checks
/// measure the line without the terminal NBSP's advance while the
/// NBSP stays in the line's character-exact content.
#[allow(clippy::too_many_arguments)] // the pair faces/sizes + the chain + the two indent offsets
pub(crate) fn wrap_block_text(
    face: &crate::textmetrics::ResolvedFace,
    text: &str,
    size: f64,
    usable_w: f64,
    cs_face: &crate::textmetrics::ResolvedFace,
    cs_size: f64,
    fb_chain: &[&crate::textmetrics::ResolvedFace],
    first_off: f64,
    left_off: f64,
) -> Vec<WrapLine> {
    wrap_block_text_scaled(
        face, text, size, usable_w, cs_face, cs_size, fb_chain, first_off, left_off, None,
    )
}

/// Character runs for the wrap (see [`wrap_block_text_scaled`]): per
/// character of the text, the run's size over the block's and the
/// run's face (`None` = the block face).
pub(crate) struct WrapRuns {
    pub factor: Vec<f64>,
    pub face: Vec<Option<usize>>,
    pub faces: Vec<crate::textmetrics::ResolvedFace>,
}

/// [`wrap_block_text`] with character runs. An LTR paragraph that
/// carries runs measures each stretch of equal size and face with that
/// run's face at that run's size, so a label whose tail is set smaller
/// fits where Visio fits it (the network LLD's P2MP labels: 10 pt +
/// 8 pt measure 3.55 in against a 3.58 in block; measured all at 10 pt
/// they wrapped), a paragraph set wholly in another size wraps at that
/// size, and a bold run measures bold (Greptile on PR #350). Without
/// runs, or in a paragraph where every character keeps the block's
/// size and face, the measurement is exactly as before.
#[allow(clippy::too_many_arguments)] // wrap_block_text's arguments + the runs
pub(crate) fn wrap_block_text_scaled(
    face: &crate::textmetrics::ResolvedFace,
    text: &str,
    size: f64,
    usable_w: f64,
    cs_face: &crate::textmetrics::ResolvedFace,
    cs_size: f64,
    fb_chain: &[&crate::textmetrics::ResolvedFace],
    first_off: f64,
    left_off: f64,
    runs: Option<&WrapRuns>,
) -> Vec<WrapLine> {
    let mut lines: Vec<WrapLine> = Vec::new();
    // CRLF / lone CR normalize to one break; the run keys follow the
    // characters they belong to.
    let mut norm = String::with_capacity(text.len());
    let mut keys: Vec<(f64, Option<usize>)> = Vec::new();
    let mut chars = text.chars().enumerate().peekable();
    while let Some((i, c)) = chars.next() {
        let key = runs.map_or((1.0, None), |r| {
            (
                r.factor.get(i).copied().unwrap_or(1.0),
                r.face.get(i).copied().flatten(),
            )
        });
        if c == '\r' {
            if chars.peek().is_some_and(|&(_, n)| n == '\n') {
                continue;
            }
            norm.push('\n');
        } else {
            norm.push(c);
        }
        keys.push(key);
    }
    let same =
        |a: (f64, Option<usize>), b: (f64, Option<usize>)| (a.0 - b.0).abs() <= 1e-9 && a.1 == b.1;
    let mut para_off = 0usize;
    for para in norm.split('\n') {
        let para_len = para.chars().count();
        let para_keys = &keys[para_off.min(keys.len())..(para_off + para_len).min(keys.len())];
        para_off += para_len + 1;
        let styled = runs.is_some() && para_keys.iter().any(|k| !same(*k, (1.0, None)));
        let para_chars: Vec<char> = if styled {
            para.chars().collect()
        } else {
            Vec::new()
        };
        // Where the current line starts its search: lines are exact,
        // in-order substrings of the paragraph.
        let cursor = std::cell::Cell::new(0usize);
        let locate = |s: &str| -> Option<usize> {
            let sc: Vec<char> = s.chars().collect();
            (cursor.get()..=para_chars.len().checked_sub(sc.len())?)
                .find(|&k| para_chars[k..k + sc.len()] == sc[..])
        };
        let advance = |s: &str| {
            if styled {
                if let Some(p) = locate(s) {
                    cursor.set(p + s.chars().count());
                }
            }
        };
        let rtl = paragraph_base_rtl(para);
        // Run-by-run measurement with the faces the raster paints
        // with (PR #269 review for RTL; #282 extended it to LTR for
        // the CJK runs): the paragraph's own face/size pair over the
        // shared fallback chain. A pure-script paragraph the primary
        // face covers measures as one run through the same single-
        // face path as before.
        let measure = |k: (f64, Option<usize>), seg: &str| {
            let f =
                k.1.and_then(|i| runs.and_then(|r| r.faces.get(i)))
                    .unwrap_or(face);
            f.width_in_runs(fb_chain, seg, size * k.0)
        };
        let width_in = |s: &str| {
            if s.is_empty() {
                // Nothing to measure — and no position to look up: an
                // emitted whole token leaves the cursor at the paragraph
                // end (Greptile on PR #350).
                0.0
            } else if rtl {
                cs_face.width_in_runs(fb_chain, s, cs_size)
            } else if let Some(p) = styled.then(|| locate(s)).flatten() {
                // Equal-key stretches, each with its own face and size.
                let mut total = 0.0;
                let mut seg = String::new();
                let mut seg_k = para_keys[p];
                for (k, c) in s.chars().enumerate() {
                    let key = para_keys[p + k];
                    if !same(key, seg_k) {
                        total += measure(seg_k, &seg);
                        seg.clear();
                        seg_k = key;
                    }
                    seg.push(c);
                }
                total + measure(seg_k, &seg)
            } else {
                face.width_in_runs(fb_chain, s, size)
            }
        };
        // #235: the hang measure — a terminal no-break space's
        // advance is excluded (it paints past the margin); anything
        // else measures whole.
        fn hang_trim(s: &str) -> &str {
            s.strip_suffix('\u{a0}').unwrap_or(s)
        }
        // Per-line usable width: the first line takes first_off (the
        // bullet tab offset / first-line indent), wrapped lines take
        // left_off (the hanging indent) — #231.
        let mut wrapped = false;
        let line_usable = |wrapped: bool| usable_w - if wrapped { left_off } else { first_off };
        let mut cur = String::new();
        let mut started = false;
        // #282 kinsoku: the emergency splitter can emit a whole
        // token as its own (overflowing) line, leaving `cur` empty —
        // the paragraph's closing push must not then add a phantom
        // EMPTY line (it would inflate TEXTHEIGHT by a slot). An
        // empty paragraph still emits its one empty line (below).
        let mut para_has_lines = false;
        for token in crate::textmetrics::wrap_tokens(para) {
            let candidate = if started {
                format!("{cur}{}{}", token.sep, token.tok)
            } else {
                token.tok.to_string()
            };
            let usable = line_usable(wrapped);
            if started && width_in(hang_trim(&candidate)) > usable {
                if cur.is_empty() {
                    // The break lands on a pending separator space (an
                    // authored space run or a leading indent too wide
                    // to share a line with the word): the space is
                    // content (#144 review) — it becomes a line of its
                    // own, never an empty line, never dropped. An
                    // EMPTY separator (a CJK boundary) follows a token
                    // the hard split below already emitted whole —
                    // pushing it would add a phantom empty line
                    // (`图，`, ``, `上`, `色。`) (CodeRabbit PR
                    // review).
                    if !token.sep.is_empty() {
                        advance(token.sep);
                        lines.push(WrapLine {
                            text: token.sep.to_string(),
                            rtl,
                            first_of_para: !wrapped,
                        });
                        para_has_lines = true;
                    }
                } else {
                    advance(&cur);
                    lines.push(WrapLine {
                        text: std::mem::take(&mut cur),
                        rtl,
                        first_of_para: !wrapped,
                    });
                    para_has_lines = true;
                }
                cur = token.tok.to_string();
                wrapped = true;
            } else {
                cur = candidate;
            }
            started = true;
            // A single token wider than the line: hard-split at the
            // longest character prefix that fits (always ≥1 char, so
            // the loop makes progress). #282 kinsoku: the split never
            // leaves a line-head-forbidden char to OPEN the rest —
            // the kept prefix extends over consecutive forbidden
            // chars (oikomi-style; the token may then overflow its
            // band rather than break before 。，）… and a token that
            // is all forbidden-headed tail emits whole).
            let mut usable = line_usable(wrapped);
            while width_in(hang_trim(&cur)) > usable && cur.chars().count() > 1 {
                let mut split_at = cur
                    .char_indices()
                    .nth(1)
                    .map(|(i, _)| i)
                    .unwrap_or(cur.len());
                for (i, _) in cur.char_indices().skip(1) {
                    if width_in(hang_trim(&cur[..i])) <= usable {
                        split_at = i;
                    } else {
                        break;
                    }
                }
                // The boundary rules run to a FIXPOINT, not in a
                // fixed order: glue (U+00A0 and friends — a boundary
                // adjacent to glue is never a grapheme edge, keeping
                // `图⍽表` intact even when the joined token then
                // overflows its line, CodeRabbit PR review) can move
                // the split PAST the position the forbidden-head
                // pass already cleared, stranding a closer at the
                // head of the rest (`表⍽，！` must not open with `！`,
                // CodeRabbit/Greptile round 5). A combining mark at
                // the boundary attaches to the kept prefix — the
                // emergency splitter never splits `カ` from its
                // dakuten (CodeRabbit round 5) — WITHOUT consuming
                // the following base, so a normal break before `ク`
                // stays available.
                loop {
                    let mut moved = false;
                    while split_at < cur.len() {
                        match cur[split_at..].chars().next() {
                            Some(c) if crate::textmetrics::line_head_forbidden(c) => {
                                split_at += c.len_utf8();
                                moved = true;
                            }
                            _ => break,
                        }
                    }
                    if split_at < cur.len() {
                        let prev_is_glue = cur[..split_at]
                            .chars()
                            .next_back()
                            .is_some_and(crate::textmetrics::is_glue_pub);
                        let next_is_glue = cur[split_at..]
                            .chars()
                            .next()
                            .is_some_and(crate::textmetrics::is_glue_pub);
                        if prev_is_glue || next_is_glue {
                            split_at += cur[split_at..]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8);
                            moved = true;
                        }
                        if cur[split_at..]
                            .chars()
                            .next()
                            .is_some_and(crate::textmetrics::is_combining_pub)
                        {
                            split_at += cur[split_at..]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8);
                            moved = true;
                        }
                    }
                    if !moved {
                        break;
                    }
                }
                let rest = cur.split_off(split_at);
                advance(&cur);
                lines.push(WrapLine {
                    text: std::mem::replace(&mut cur, rest),
                    rtl,
                    first_of_para: !wrapped,
                });
                para_has_lines = true;
                wrapped = true;
                usable = line_usable(wrapped);
            }
        }
        if !cur.is_empty() || !para_has_lines {
            lines.push(WrapLine {
                text: cur,
                rtl,
                first_of_para: !wrapped,
            });
        }
    }
    lines
}

/// `HideText` (Miscellaneous): the shape keeps its text but does not
/// show it (#397). Formula first, as [`shape_layer::section_flag`]
/// reads NoShow, so a layout switched in the app takes effect without
/// a fresh cache. A formula the instance inherits names its MASTER's
/// shape IDs (the box masters' panel reads `Sheet.5!User.ShapeLayout>1`,
/// 5 being the master root) and evaluates in the master-local scope the
/// fields use (#378). Without a usable formula the instance's OWN cache
/// comes before the master's: Visio stores the value it computed for the
/// instance as `V` under `F='Inh'`, while the master's cache holds its
/// own layout's 0.
pub(crate) fn text_hidden(doc: &Document, s: &Shape, chain: &[&Shape]) -> bool {
    let Some(cell) = doc.resolve_cell(s, chain, "HideText") else {
        return false;
    };
    let own = s.sheet.cell("HideText");
    // The envs are built only for a live formula: most shapes carry a
    // plain `0` (or no cell), and this runs for every text block.
    let by_formula = cell
        .f
        .as_deref()
        .filter(|f| *f != "Inh" && *f != "No Formula")
        .and_then(|f| {
            let env = shape_env(doc, s, chain);
            let inherited = own.is_none_or(|c| c.f.as_deref() == Some("Inh"));
            let scope = if inherited {
                crate::fields::master_scope(doc, s, chain)
            } else {
                None
            };
            match &scope {
                Some(scope) => vsdx::formula::eval_field(f, &env, scope, None).ok(),
                None => vsdx::formula::eval(f, &env).ok(),
            }
        })
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite());
    by_formula
        .or_else(|| own.and_then(Cell::numeric))
        .or_else(|| cell.numeric())
        .is_some_and(|v| v != 0.0)
}

#[allow(clippy::too_many_arguments)] // placement + flip axes + the #22 skip + #124 greek ctx + #267 fields
pub(crate) fn emit_text(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    width: f64,
    height: f64,
    sx: f64,
    sy: f64,
    skip: bool,
    greek: Option<&GreekCtx>,
    fields: Option<&crate::fields::FieldsCtx<'_>>,
    out: &mut String,
) {
    if skip || text_hidden(doc, s, chain) {
        return;
    }
    // Authored leading spacing is content (#144): trim only the END —
    // the first paragraph's leading space run must survive into its
    // line. The trailing trim is now GOLD-BACKED (#148, 2026-10-30):
    // the shipped corpus carries Visio's own cached
    // `TEXTHEIGHT(TheText,TxtWidth)` values, and 25 one-visual-line
    // texts ending in the corpus-standard CRLF (DBUML_M/U "PK\r\n",
    // TITLE_M/U "\r\n", ELETEL_M, OFFEQP…) cache exactly ONE line
    // height (1.2003–1.2004 em at the canonical 120% spacing) — Visio
    // does NOT count the trailing break as a line, so dropping it
    // here keeps the block's line count and vertical centering where
    // Visio puts them. The trim set is explicit breaks/spaces/tab
    // (#235): a trailing NO-BREAK SPACE is content — it composes and
    // hangs past the margin in Visio, so the blanket `str::trim_end`
    // (whose whitespace set includes U+00A0) must not eat it.
    const TRAILING_TRIM: [char; 4] = ['\r', '\n', ' ', '\t'];
    // #267: a `<fld>`-bearing text renders its SUBSTITUTED string —
    // evaluated, picture-formatted, laid out. The fast path returns
    // None for the marker-free shape, keeping its plain text (and
    // the marker-free packages) byte-identical.
    let substituted = crate::fields::substituted_text(doc, s, chain, fields);
    let Some(text) = substituted
        .as_deref()
        .or(crate::fields::text_sheet(doc, s, chain).text.as_deref())
        .map(|t| t.trim_end_matches(TRAILING_TRIM))
        .filter(|t| !t.is_empty())
    else {
        return;
    };
    let tb = resolve_text_block(doc, s, chain, width, height);
    // Character runs (`<cp IX='n'/>` markers → Character rows) and the
    // layout text: U+2028 is a line break, a tab advances like four
    // spaces (Visio 2021 witness c03 — tab stops are not modeled).
    // Blocks without differing runs keep their exact former output.
    let runs = if substituted.is_none() {
        text_runs(doc, s, chain, &tb)
    } else {
        None
    };
    let layout_text: String;
    let text = if text.contains(['\u{2028}', '\t']) {
        layout_text = text.replace('\u{2028}', "\n").replace('\t', "    ");
        layout_text.as_str()
    } else {
        text
    };
    let (tx, ty) = tb.pin;
    let (tw, th) = tb.size;
    let angle = tb.angle;
    let size = tb.font_size;
    let color = tb.color.clone();
    let style_bits = (tb.bold as u32) | ((tb.italic as u32) << 1);
    // Character.Pos (#230): the measured reduced em + baseline shift
    // for the block's family (None = unwitnessed family → nominal
    // draw). The paint size shrinks; the LINE slots and the baseline
    // ascent math stay nominal — Visio's TEXTHEIGHT retains the
    // nominal size and the shift rides the nominal baseline.
    // Character.Pos metrics come from the family that PAINTS each
    // direction (CodeRabbit local review): LTR lines paint the Latin
    // font, RTL lines the complex-script font (or the Latin chain
    // when no CS font resolves) — an Arial block with a Times CS
    // font must script with Times's numbers on RTL lines, and an
    // unwitnessed CS family keeps the nominal draw there.
    let script_latin = script_metrics(tb.font.as_deref(), tb.pos);
    let script_l = script_latin;
    let reduce = script_latin.map_or(1.0, |(r, _)| r);
    let paint_size = size * reduce;
    let font_attrs = format!(
        r#"font-size="{s:.4}"{ff} fill="{c}"{b}{i}"#,
        s = paint_size,
        ff = font_family_attr(tb.font.as_deref()),
        // Document-supplied color text (e.g. a `#…` value) is attribute
        // content — escape it like every other attribute.
        c = escape_attr(&color),
        b = if style_bits & 1 != 0 {
            r#" font-weight="bold""#
        } else {
            ""
        },
        i = if style_bits & 2 != 0 {
            r#" font-style="italic""#
        } else {
            ""
        },
    );
    let halign = tb.halign;
    let valign = tb.valign;
    let (ins_l, ins_r, ins_t, ins_b) = tb.insets;
    let line_h = tb.line_height;

    // Greedy word wrap against REAL font metrics (GitHub #21): runs
    // are measured as the sum of per-glyph advances from the face the
    // rasterizer will draw (system font where installed, vendored
    // Carlito otherwise — `textmetrics`), replacing the ~0.5 em
    // average-advance guess that re-wrapped lines the author had
    // fitted to the block.
    let usable_w = (tw - ins_l - ins_r).max(0.02);
    let face = crate::textmetrics::resolve(tb.font.as_deref(), tb.bold, tb.italic);
    // The complex-script pair (#257 text-direction): an RTL-base
    // paragraph measures and paints with ComplexScriptFont/Size —
    // falling back to the Latin chain/size per the spec's zero
    // semantics — so wrap and raster agree per paragraph. The vendored
    // Vazirmatn (dc::register_complex_script_faces) covers the glyphs when the
    // document names no CS face.
    let cs_chain = tb.cs_font.as_deref().or(tb.font.as_deref());
    let cs_face = crate::textmetrics::resolve(cs_chain, tb.bold, tb.italic);
    // The per-cluster fallback CHAIN (#282 zh-Hans grew the #257 pair
    // face to a chain): the vendored Vazirmatn for the Arabic script,
    // then the vendored CJK faces in `dc::CJK_FACE_ORDER` — SC for the
    // Han glyphs the Latin and Arabic faces lack, JP for the kana/JIS
    // Kanji the SC subset lacks, KR for Hangul — the same order
    // `dc::register_complex_script_faces` loads them into the raster
    // fontdb, which is the order usvg's per-cluster fallback picks a
    // face (see the wrap's probe note). Unregistered (the dc unit-test
    // binaries), every entry resolves like the Latin default and the
    // probe never fires.
    let mut fallback_faces = Vec::new();
    fallback_faces.push(crate::textmetrics::resolve(Some("Vazirmatn"), tb.bold, tb.italic));
    for family in crate::CJK_FACE_ORDER {
        fallback_faces.push(crate::textmetrics::resolve(Some(family), tb.bold, tb.italic));
    }
    let fb_chain: Vec<&crate::textmetrics::ResolvedFace> = fallback_faces.iter().collect();
    let cs_size = tb.cs_size.unwrap_or(size);
    let script_cs = script_metrics(cs_chain, tb.pos);
    let script_r = script_cs;
    let cs_paint_size = cs_size * script_cs.map_or(1.0, |(r, _)| r);

    // Line-band edges before the wrap — the #231 bullet layout needs
    // them for its tab-stop arithmetic (the first line's text starts
    // at the stop, so the wrap must reserve that offset).
    let left_x = tx - tb.loc_pin.0 + ins_l;
    let right_x = left_x + (tw - ins_l - ins_r);
    // #231 bullet layout: the bullet's own advance, its line-start
    // position, and the tab stop the after-bullet text snaps to. The
    // bullet itself draws at bullet_x; the measured placement rule:
    // TextPosAfterBullet when it lies past the bullet's start, else
    // the hanging indent (IndLeft when IndFirst is negative) or the
    // next DefaultTabStop multiple — whichever comes first, never
    // before the bullet's end.
    struct BulletLayout {
        spec: crate::svg::BulletSpec,
        x: f64,
        text_x: f64,
    }
    let bullet_layout = tb.bullet.as_ref().map(|b| {
        // Positive IndFirst indents the bullet; negative outdents it
        // (the hanging-indent case) past the margin. The stop rule
        // lives in `bullet_stop_offset_in` — shared with the
        // TEXTHEIGHT measure, which wraps at the same first-line
        // offset (PR #284 review) — and returns a TEXT-AREA offset in
        // INCHES; this function emits in the same inch space.
        let bullet_off = tb.ind_first;
        let stop_off = bullet_stop_offset_in(&tb).unwrap_or(bullet_off);
        BulletLayout {
            spec: b.clone(),
            x: left_x + bullet_off,
            text_x: left_x + stop_off,
        }
    });
    // Paragraph indents for the wrap: a bulleted paragraph's first
    // line starts at the tab stop; otherwise at max(IndFirst, 0).
    // Wrapped lines take IndLeft (#231). RTL paragraphs carry the same
    // offsets (unwitnessed — #231's open questions).
    let (first_off, left_off) = match &bullet_layout {
        Some(bl) => ((bl.text_x - left_x).max(0.0), tb.ind_left),
        // The unbulleted first line reserves the COMBINED indent
        // (IndLeft base + IndFirst adjustment — PR #284 review round
        // 4): the paint band and the TEXTHEIGHT measure already use
        // `unbulleted_first_offset_in`; wrapping at less would let
        // text run past the right margin and split measured line
        // counts from laid-out ones.
        None => (unbulleted_first_offset_in(&tb), tb.ind_left),
    };
    // Greeking (#124 item 2): a block whose on-screen em height falls
    // under TEXT_GREEK_PX renders as gray bars — per line, shaped only
    // at paragraph granularity (one width_in per paragraph, no greedy
    // word wrap and no <text> node for usvg to turn into glyph
    // outlines). Every met size is recorded regardless, so the layer
    // cache can tell when a ppi-bucket change flips the decision.
    let greeked = greek.is_some_and(|g| {
        g.sizes.borrow_mut().push(size);
        g.greeks(size)
    });
    let (lines, bar_widths) = if greeked {
        let mut bars = Vec::new();
        for para in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
            let w = face.width_in_runs(&fb_chain, para, size);
            if w <= usable_w {
                bars.push(w.max(usable_w * 0.2));
            } else {
                // Chunk the paragraph across an estimated wrap: full
                // bars of the usable width plus one remainder bar.
                let full = (w / usable_w).floor() as usize; // ≥ 1: w > usable_w
                let rem = w - full as f64 * usable_w;
                bars.extend(std::iter::repeat_n(usable_w, full));
                if rem > usable_w * 0.05 {
                    bars.push(rem);
                }
            }
        }
        (Vec::new(), bars)
    } else {
        // Character runs measure with their own size and face (the
        // raster paints them so).
        let wrap_runs: Option<WrapRuns> = runs.as_ref().map(|r| {
            let mut faces: Vec<crate::textmetrics::ResolvedFace> = Vec::new();
            let mut face_of: Vec<Option<usize>> = Vec::new();
            for st in &r.styles {
                let (b, i) = (st.bold.unwrap_or(tb.bold), st.italic.unwrap_or(tb.italic));
                face_of.push(if (b, i) == (tb.bold, tb.italic) {
                    None
                } else {
                    faces.push(crate::textmetrics::resolve(tb.font.as_deref(), b, i));
                    Some(faces.len() - 1)
                });
            }
            let n = text.chars().count();
            WrapRuns {
                factor: (0..n)
                    .map(|i| {
                        r.styles
                            .get(r.style_at(i))
                            .and_then(|st| st.size)
                            .filter(|z| *z > 0.0 && size > 0.0)
                            .map_or(1.0, |z| z / size)
                    })
                    .collect(),
                face: (0..n)
                    .map(|i| face_of.get(r.style_at(i)).copied().flatten())
                    .collect(),
                faces,
            }
        });
        (
            wrap_block_text_scaled(
                &face,
                text,
                paint_size,
                usable_w,
                &cs_face,
                cs_paint_size,
                &fb_chain,
                first_off,
                left_off,
                wrap_runs.as_ref(),
            ),
            Vec::new(),
        )
    };
    let n_lines = if greeked {
        bar_widths.len()
    } else {
        lines.len()
    } as f64;
    // Per-line slots (PR #269 review): an RTL line under a bigger
    // ComplexScriptSize needs a proportionally taller slot — the flat
    // n × line_h overlapped larger glyphs onto their neighbors, and
    // TEXTHEIGHT (textfunc) measured the same too-small block. Slots
    // stay NOMINAL under Character.Pos (#230: TEXTHEIGHT retains the
    // nominal size).
    // Each wrapped line's character range in the layout text (lines are
    // exact substrings, in order; the separators a break consumed are
    // skipped) — only needed when the block carries runs.
    let line_ranges: Vec<Option<(usize, usize)>> = if runs.is_some() {
        let chars: Vec<char> = text.chars().collect();
        let mut cursor = 0usize;
        lines
            .iter()
            .map(|l| {
                let lc: Vec<char> = l.text.chars().collect();
                let found = (cursor..=chars.len().saturating_sub(lc.len()))
                    .find(|&k| chars[k..k + lc.len()] == lc[..])?;
                cursor = found + lc.len();
                Some((found, found + lc.len()))
            })
            .collect()
    } else {
        Vec::new()
    };
    // A run line's segments: each style stretch with its face, paint
    // size and width — the one measure for placement, alignment and the
    // #229 background (Greptile on PR #350: the block-style width
    // misplaced centered and right-aligned mixed-size lines).
    let run_segs = |r: &TextRuns, content: &str, lo: usize| {
        let chars: Vec<char> = content.chars().collect();
        let mut segs: Vec<(String, usize, f64, f64)> = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let st = r.style_at(lo + i);
            let mut j = i + 1;
            while j < chars.len() && r.style_at(lo + j) == st {
                j += 1;
            }
            let seg: String = chars[i..j].iter().collect();
            let rs = &r.styles[st];
            let fs = rs.size.map_or(paint_size, |z| z * reduce);
            let seg_face = crate::textmetrics::resolve(
                tb.font.as_deref(),
                rs.bold.unwrap_or(tb.bold),
                rs.italic.unwrap_or(tb.italic),
            );
            let w = seg_face.width_in_runs(&fb_chain, &seg, fs);
            segs.push((seg, st, fs, w));
            i = j;
        }
        segs
    };
    // A run line's height factor: its largest run over the block size
    // (Greptile on PR #350: a 30 pt run in a 10 pt block overlapped its
    // neighbours at 12 pt spacing). A line set wholly in smaller runs is
    // that much shorter, as in Visio 2021 (the network LLD: 6 pt address
    // lines in an 8 pt block sit 6 pt apart; at 8 pt they overflowed the
    // box). An empty line keeps the block's height.
    let run_scale = |line_no: usize| -> f64 {
        match (&runs, line_ranges.get(line_no).copied().flatten()) {
            (Some(r), Some((lo, hi))) if !lines[line_no].rtl && size > 0.0 && hi > lo => (lo..hi)
                .map(|i| r.styles[r.style_at(i)].size.map_or(1.0, |z| z / size))
                .fold(0.0, f64::max),
            _ => 1.0,
        }
    };
    let slots: Vec<f64> = if greeked {
        vec![line_h; n_lines as usize]
    } else {
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| line_slot_height(&tb, l.rtl) * run_scale(i))
            .collect()
    };
    let total_h: f64 = slots.iter().sum();
    // The first line's ascent rides its own effective size (nominal
    // under Pos — the shift applies to the baseline, not the slot).
    let first_size = lines.first().map_or(size, |l| {
        if l.rtl {
            effective_cs_size(&tb)
        } else {
            size * run_scale(0)
        }
    });

    // Text-block rect from the sheet: the pin (TxtPinX/TxtPinY, y-up local)
    // sits TxtLocPinX/TxtLocPinY from the block's lower-left corner, so that
    // corner is (tx - lpx, ty - lpy) — masters can pin off-center, so no
    // centered-pin assumption. In the emitted y-negated frame the top edge
    // is -(ty - lpy + th).
    let lpy = tb.loc_pin.1;
    let top_edge = -(ty - lpy + th);
    let first_baseline = top_edge + text_first_baseline(&tb, total_h, first_size);
    let _ = (valign, ins_t, ins_b);
    // Line x per alignment: left edge of the text area, center, or right.
    // #257 text-direction: an RTL-base line reads 0/2 as start/end
    // relative to its READING order (Visio's RTL editions), so the
    // visual left edge moves with the line's base. The anchor stays
    // "start" (usvg lays glyphs from the visual left under its
    // hardcoded LTR base); the x is the line's computed visual left.
    let (anchor, x) = match halign {
        0 | 3 => ("start", left_x),
        2 => ("end", right_x),
        _ => ("middle", left_x + (tw - ins_l - ins_r) / 2.0),
    };

    let mut body = String::new();
    // #231 path bullets live OUTSIDE the <text> element (PR #284
    // review: SVG's content model allows only character data and
    // tspan/textPath inside text — usvg drops a <path> there, so the
    // built-in bullets reserved their gap but painted nothing). They
    // are emitted just before the <text>, inside the same transform
    // group; custom-string bullets stay in `body` (tspans are legal).
    let mut bullet_paths = String::new();
    // Baselines ride each line's OWN ascent: the running coordinate is
    // the next line's slot TOP (the first baseline already sits
    // `first_size × 0.8` below the block's band top), and line i's
    // baseline is its slot top plus ITS effective ascent — a larger
    // complex-script line following a Latin one rises by the larger
    // ascent instead of overlapping it (PR #269 review; uniform for
    // greek bars, whose Latin ascent/slots make this byte-identical
    // to the old advance-by-slot form).
    let mut top = first_baseline - first_size * 0.8;
    // The #229 background rect spans the LINES, not the block: its y
    // starts at the first line's slot top (PR review, wave 4 — the
    // block top would hang the rect above middle/top-aligned text).
    // Saved before the emission loop advances `top`.
    let bkgnd_top = top;
    if greeked {
        // Greek bars (#124 item 2): one neutral-gray rect per line,
        // anchored like the tspan it replaces (left/center/right).
        // Height sits in the x-height band of the line's em box.
        for (w, slot) in bar_widths.iter().zip(&slots) {
            let (bx, bw) = match anchor {
                "start" => (x, *w),
                "end" => (x - *w, *w),
                _ => (x - *w / 2.0, *w),
            };
            let _ = write!(
                body,
                r##"<rect x="{bx:.4}" y="{y:.4}" width="{bw:.4}" height="{h:.4}" fill="#7f7f7f"/>"##,
                y = top + size * 0.8 - size * 0.72,
                h = size * 0.62,
            );
            top += slot;
        }
    }
    // #229 text-background extents: the union of the laid-out lines'
    // [left, right] spans — the rect spans the widest laid-out line
    // and the union of the line heights (margins position the text
    // but add NO background padding). Accumulated during emission;
    // the rect itself is written just before the <text> element
    // (above shape geometry, below text).
    let mut bkgnd_span: Option<(f64, f64)> = None;
    let mut any_rtl = false;
    let mut deco = String::new();
    for (line_no, (line, slot)) in lines.iter().zip(&slots).enumerate() {
        let range = line_ranges.get(line_no).copied().flatten();
        let (content, line_x, line_attrs, pre, y) = if line.rtl {
            any_rtl = true;
            // The RTL line paints with the complex-script pair; its
            // visual LEFT edge per the start/end mapping, computed
            // from the line's own shaped width. The face/size ride
            // the tspan — a block may mix direction lines and the
            // element-level attrs stay Latin. (Bullets and paragraph
            // indents stay unwitnessed on RTL lines — #231 open
            // questions — so RTL keeps the plain band placement.)
            let lw = cs_face.width_in_runs(&fb_chain, &line.text, cs_paint_size);
            let content = format!("\u{2067}{}\u{2069}", line.text);
            let vx = match halign {
                0 | 3 => right_x - lw, // start (reading order) = visual right edge
                2 => left_x,           // end (reading order) = visual left edge
                _ => x,                // center: the shared center
            };
            // PR #269 review: a right-aligned (halign 2) block anchors
            // the ELEMENT "end", but this line sits at the block's
            // visual LEFT edge — the inherited "end" would swing its
            // glyphs further left, outside the block. It anchors
            // itself "start" there. `font_family_attr` returns the
            // COMPLETE attribute (leading space included) — wrapping
            // it in another font-family="" nested the quotes and
            // emitted malformed XML.
            let attrs = format!(
                "{}{}",
                if halign == 2 {
                    r#" text-anchor="start""#
                } else {
                    ""
                },
                if (cs_paint_size - paint_size).abs() > 1e-9 {
                    format!(
                        r#" font-size="{cs_paint_size:.4}"{}"#,
                        font_family_attr(cs_chain)
                    )
                } else {
                    font_family_attr(cs_chain)
                }
            );
            let line_size = effective_cs_size(&tb);
            let y = top + line_size * 0.8 - script_r.map_or(0.0, |(_, shift)| shift * line_size);
            // The RTL line's laid-out extent joins the #229 background
            // union too (PR review, wave 4) — an all-RTL block must
            // still paint its text background. The extent follows the
            // line's EFFECTIVE anchor (CodeRabbit local review): a
            // centered RTL line anchors middle at vx, so its extent
            // runs half a line width to each side; start/end anchors
            // place vx at an edge.
            let span = match halign {
                // Centered: vx is the line's middle.
                1 => (vx - lw / 2.0, vx + lw / 2.0),
                _ => (vx, vx + lw),
            };
            bkgnd_span = Some(match bkgnd_span {
                Some((a, b)) => (a.min(span.0), b.max(span.1)),
                None => span,
            });
            (content, vx, attrs, String::new(), y)
        } else {
            // This line's baseline FIRST (the bullet glyph hangs off
            // it): its slot top plus ITS OWN effective ascent
            // (0.8 × effective size — the same factor
            // `text_first_baseline` gives the first line).
            // Character.Pos (#230) then shifts the baseline by the
            // measured em offset (positive = raised) — independent of
            // the reduced glyph em, so the shift base is the line's
            // NOMINAL size.
            let line_size = size * run_scale(line_no);
            let y = top + line_size * 0.8 - script_l.map_or(0.0, |(_, shift)| shift * line_size);
            // LTR line placement: every line lays out in an indented
            // band — a bulleted paragraph's first line carries
            // bullet+gap+text as ONE unit anchored at the tab stop
            // (left-anchored override there: the stop is an absolute
            // x), other first lines take IndFirst and wrapped lines
            // take IndLeft (#231, PR review wave 4: the wrap reserves
            // exactly these offsets, so the paint must honor them).
            // Under the inherited `end` anchor a tspan's x is where
            // the glyphs END — halign 2 keeps the anchor and derives
            // the left edge from the line width.
            // Measure through the same fallback chain the LTR wrap
            // used: the primary face gives a missing-glyph estimate
            // for CJK spans it lacks, mis-placing centered/right
            // lines, bullet units, and TextBkgnd spans (CodeRabbit
            // PR review). Latin-only lines keep their width — the
            // chain falls back to `width_in` when the face covers
            // the whole line.
            let line_w = match (&runs, range) {
                (Some(r), Some((lo, _))) => run_segs(r, &line.text, lo).iter().map(|g| g.3).sum(),
                _ => face.width_in_runs(&fb_chain, &line.text, paint_size),
            };
            let bulleted = line.first_of_para && bullet_layout.is_some();
            let band_off = if bulleted {
                // The unit math below places the line outright.
                0.0
            } else if line.first_of_para {
                unbulleted_first_offset_in(&tb)
            } else {
                tb.ind_left
            };
            let band_lo = left_x + band_off;
            let (lx, line_left) = if bulleted {
                let bl = bullet_layout.as_ref().unwrap();
                // Centered/right-aligned first lines align
                // bullet+gap+text as ONE unit: shift the whole
                // [bullet .. text] run so the unit sits in the band
                // (measured rule, #231).
                let unit_w = (bl.text_x - bl.x) + line_w;
                let unit_x = match halign {
                    // Left/justify: the bullet sits where the indents
                    // put it — no unit shift.
                    0 | 3 => bl.x,
                    2 => right_x - unit_w,
                    _ => left_x + ((tw - ins_l - ins_r) - unit_w) / 2.0,
                };
                (unit_x + (bl.text_x - bl.x), unit_x)
            } else {
                match halign {
                    0 | 3 => (band_lo, band_lo),
                    2 => (right_x, right_x - line_w),
                    // Centered: the anchor is the band's midpoint —
                    // the line's LEFT edge (what the #229 background
                    // spans) sits half a line width left of it (PR
                    // #284 review: recording band_lo painted the
                    // background at the margin, leaving connector
                    // lines visible through the centered text).
                    _ => {
                        let cx = (band_lo + right_x) / 2.0;
                        (cx, cx - line_w / 2.0)
                    }
                }
            };
            // The bullet glyph itself — path bullets for the built-in
            // round set (viewer-proof, #231), a text glyph for custom
            // strings. Only LTR first lines draw one (RTL bullets are
            // an open #231 question); the bullet centers slightly
            // above the baseline like a text glyph of its own size.
            let pre = if bulleted {
                let bl = bullet_layout.as_ref().unwrap();
                let by = y - 0.28 * bl.spec.size;
                // The bullet rides the unit shift: centered and
                // right-aligned first lines re-place the whole
                // [bullet .. text] run (PR #284 review — the text
                // moved via unit_x while the bullet stayed at the
                // unshifted bl.x, stranding it at the margin).
                let bullet_x = line_left;
                match &bl.spec.glyph {
                    // Paths go outside <text> (see `bullet_paths`).
                    BulletGlyph::Round(kind) => {
                        bullet_paths.push_str(&bullet_glyph_svg(
                            *kind,
                            bullet_x,
                            by,
                            bl.spec.size,
                            &color,
                        ));
                        String::new()
                    }
                    BulletGlyph::Str(glyph) => format!(
                        // The bullet tspan anchors ITSELF "start" at
                        // bullet_x (PR #284 review round 4): left
                        // open, it would inherit the element's
                        // middle/end anchor and shift the glyph left
                        // by half (or all) of its width away from the
                        // laid-out bullet position.
                        r#"<tspan x="{bx:.4}" y="{by:.4}" text-anchor="start" font-size="{fs:.4}"{ff}>{g}</tspan>"#,
                        bx = bullet_x,
                        by = y,
                        fs = bl.spec.size,
                        // BulletFont absent → the PARAGRAPH face paints
                        // the bullet — the same face the advance was
                        // measured with (PR review, wave 4): passing
                        // bare None here would paint the Calibri
                        // default chain over a Courier New paragraph.
                        ff = font_family_attr(bl.spec.font.as_deref().or(tb.font.as_deref())),
                        g = escape_text(glyph),
                    ),
                }
            } else {
                String::new()
            };
            // The laid-out line spans bullet .. gap .. text: a
            // bulleted first line's right edge is the TEXT's end (the
            // anchor), not bullet_x + width (PR #284 review, round 2 —
            // the gap and the tab offset were left outside the #229
            // background).
            let span = if bulleted {
                (line_left, lx + line_w)
            } else {
                (line_left, line_left + line_w)
            };
            bkgnd_span = Some(match bkgnd_span {
                Some((a, b)) => (a.min(span.0), b.max(span.1)),
                None => span,
            });
            let attrs = if bulleted {
                r#" text-anchor="start""#.to_string()
            } else {
                String::new()
            };
            (line.text.clone(), lx, attrs, pre, y)
        };
        let _ = write!(body, "{pre}");
        // A bulleted first line anchors "start" at its text x (its only
        // line attribute); it keeps its runs too (Greptile on PR #350).
        let run_line = match (&runs, range) {
            (Some(r), Some(rg)) if !line.rtl && content == line.text => Some((r, rg)),
            _ => None,
        };
        if let Some((r, (lo, _))) = run_line {
            // One positioned chunk per run (its own fill/weight even in
            // the PDF export, which paints a chunk in one style);
            // underline/strike-through as real lines after the text.
            let segs = run_segs(r, &content, lo);
            let line_w: f64 = segs.iter().map(|g| g.3).sum();
            let line_anchor = if line_attrs.is_empty() {
                anchor
            } else {
                "start"
            };
            let mut x = match line_anchor {
                "start" => line_x,
                "end" => line_x - line_w,
                _ => line_x - line_w / 2.0,
            };
            for (seg, st, fs, seg_w) in &segs {
                let rs = &r.styles[*st];
                let _ = write!(
                    body,
                    r#"<tspan x="{x:.4}" y="{y:.4}" text-anchor="start"{}>{}</tspan>"#,
                    rs.attrs(reduce),
                    escape_text(seg)
                );
                let ink = rs.color.as_deref().unwrap_or(&color);
                for (on, dy) in [(rs.underline, 0.11), (rs.strike, -0.27)] {
                    if on && !seg.trim().is_empty() {
                        let _ = write!(
                            deco,
                            r#"<line x1="{x:.4}" y1="{yy:.4}" x2="{x2:.4}" y2="{yy:.4}" stroke="{c}" stroke-width="{w:.4}"/>"#,
                            yy = y + dy * fs,
                            x2 = x + seg_w,
                            c = escape_attr(ink),
                            w = fs * 0.06,
                        );
                    }
                }
                x += seg_w;
            }
        } else {
            let _ = write!(
                body,
                r#"<tspan x="{line_x:.4}" y="{y:.4}"{}>{}</tspan>"#,
                line_attrs,
                escape_text(&content)
            );
        }
        top += slot;
    }
    // Flip-aware block rotation (issue #19): the counter-scale keeps the
    // glyphs un-mirrored inside the caller's flipped frame, and the
    // rotation composes with flip parity so the page-space net is
    // Angle + sx*sy*TxtAngle.
    let flipped = sx < 0.0 || sy < 0.0;
    let net_txt_deg = -(sx * sy) * angle.to_degrees();
    // The block's tspan coordinates are pin-absolute (TxtPin-anchored),
    // so the counter-scale/rotation must pivot AT the pin: the inverse
    // translate after them keeps the text anchor fixed there. Without
    // it they would swing the whole block around the local origin.
    let mut transform = format!("translate({x:.5},{y:.5})", x = tx, y = -ty);
    if flipped {
        let _ = write!(transform, " scale({sx:.0},{sy:.0})", sx = sx, sy = sy);
    }
    if net_txt_deg.abs() > 1e-6 {
        let _ = write!(transform, " rotate({deg:.5})", deg = net_txt_deg);
    }
    if flipped || net_txt_deg.abs() > 1e-6 {
        let _ = write!(transform, " translate({x:.5},{y:.5})", x = -tx, y = ty);
    }
    // xml:space="preserve" (#144): SVG's default whitespace handling
    // strips leading/trailing spaces of text-node content, so the
    // spaces the splitter now preserves would be dropped again at
    // raster time. usvg (resvg 0.50) honors the attribute — inherited
    // by every tspan — and skips its stripping only for preserve, so
    // the authored spacing reaches the pixels. Greeked blocks (#124)
    // carry plain rects in `body`, never glyph nodes: they emit as a
    // bare <g> (a <text> wrapper's content model has no place for
    // shapes, and no font attributes are needed).
    if greeked {
        if flipped || net_txt_deg.abs() > 1e-6 {
            let _ = writeln!(out, r#"<g transform="{}">"#, escape_attr(&transform));
            let _ = writeln!(out, r#"<g>{b}</g>"#, b = body);
            let _ = writeln!(out, "</g>");
        } else {
            let _ = writeln!(out, r#"<g>{b}</g>"#, b = body);
        }
        return;
    }
    // #257 text-direction: the RTL lines' isolates order the runs
    // under ANY base; `direction="ltr"` pins the element base for
    // viewers that honor the attribute (usvg ignores it) so the
    // visual-left start anchor reads the same everywhere. LTR-only
    // blocks keep the pre-#257 byte form.
    let dir_attr = if any_rtl { r#" direction="ltr""# } else { "" };
    // #229 text background: ONE rectangle spanning the logical line
    // extents (widest laid-out line × the union of line heights),
    // margins position the text but add no padding. It uses the text
    // transform (same group), paints above shape geometry and below
    // text, and stays a rectangle in SVG. Greeking replaces the text
    // with bars, so the background stays off there.
    let bkgnd_rect = match (tb.text_bkgnd, bkgnd_span) {
        (Some((fill, alpha)), Some((lo, hi))) if hi > lo => {
            let (r, g, b) = hex_rgb(&fill);
            format!(
                r##"<rect x="{x:.4}" y="{y:.4}" width="{w:.4}" height="{h:.4}" fill="#{r:02x}{g:02x}{b:02x}" fill-opacity="{o:.3}" stroke="none"/>"##,
                x = lo,
                y = bkgnd_top,
                w = hi - lo,
                h = total_h,
                o = alpha as f64 / 255.0,
            )
        }
        _ => String::new(),
    };
    if flipped || net_txt_deg.abs() > 1e-6 {
        let _ = writeln!(out, r#"<g transform="{}">"#, escape_attr(&transform));
        let _ = write!(out, "{bkgnd_rect}");
        let _ = write!(out, "{bullet_paths}");
        let _ = writeln!(
            out,
            r#"<text{d} text-anchor="{a}" xml:space="preserve" {f}>{b}</text>"#,
            d = dir_attr,
            a = anchor,
            f = font_attrs,
            b = body
        );
        let _ = write!(out, "{deco}");
        let _ = writeln!(out, "</g>");
    } else {
        let _ = write!(out, "{bkgnd_rect}");
        let _ = write!(out, "{bullet_paths}");
        let _ = writeln!(
            out,
            r#"<text{d} text-anchor="{a}" xml:space="preserve" {f}>{b}</text>"#,
            d = dir_attr,
            a = anchor,
            f = font_attrs,
            b = body
        );
        let _ = write!(out, "{deco}");
    }
}

/// One Character row's look where it differs from the block (its
/// first row): `None` = as the block.
#[derive(Debug, Clone, PartialEq, Default)]
struct RunStyle {
    color: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    size: Option<f64>,
    underline: bool,
    strike: bool,
}

impl RunStyle {
    /// `reduce` is the Character.Pos size reduction the run was measured
    /// with: a super- or subscript run draws at the same reduced size
    /// (Greptile on PR #350).
    fn attrs(&self, reduce: f64) -> String {
        let mut a = String::new();
        if let Some(c) = &self.color {
            let _ = write!(a, r#" fill="{}""#, escape_attr(c));
        }
        if let Some(b) = self.bold {
            a.push_str(if b {
                r#" font-weight="bold""#
            } else {
                r#" font-weight="normal""#
            });
        }
        if let Some(i) = self.italic {
            a.push_str(if i {
                r#" font-style="italic""#
            } else {
                r#" font-style="normal""#
            });
        }
        if let Some(z) = self.size {
            let _ = write!(a, r#" font-size="{:.4}""#, z * reduce);
        }
        a
    }
}

/// A block's character runs: the style index of every layout-text
/// character (tabs count four, like the layout) and the styles.
struct TextRuns {
    per_char: Vec<usize>,
    styles: Vec<RunStyle>,
}

impl TextRuns {
    fn style_at(&self, i: usize) -> usize {
        self.per_char.get(i).copied().unwrap_or(0)
    }
}

/// Parse the raw text's `<cp IX='n'/>` markers into per-character
/// Character-row indices and resolve each row against the block's
/// first-row look (Visio 2021 witness c03: strike-through, red bold
/// and underlined runs authored through the Characters object).
/// `None` when no run differs (the block renders as before), when the
/// raw text does not reproduce the plain text, or it carries fields.
fn text_runs(doc: &Document, s: &Shape, chain: &[&Shape], tb: &TextBlock) -> Option<TextRuns> {
    let tsheet = crate::fields::text_sheet(doc, s, chain);
    let raw = tsheet.text_raw.as_deref()?;
    if !raw.contains("<cp") || raw.contains("<fld") {
        return None;
    }
    let mut plain = String::new();
    let mut idx: Vec<usize> = Vec::new();
    let mut cur = 0usize;
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let end = rest.find('>')?;
            let tag = &rest[..=end];
            if tag.starts_with("<cp") {
                let ix = tag
                    .split("IX=")
                    .nth(1)
                    .and_then(|v| v.trim_start_matches(['\'', '"']).split(['\'', '"']).next())
                    .and_then(|v| v.parse::<usize>().ok())?;
                cur = ix;
            }
            rest = &rest[end + 1..];
            continue;
        }
        let (ch, len) = if c == '&' {
            let end = rest.find(';')?;
            let ent = &rest[1..end];
            let ch = match ent {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                e if e.starts_with("#x") => char::from_u32(u32::from_str_radix(&e[2..], 16).ok()?)?,
                e if e.starts_with('#') => char::from_u32(e[1..].parse().ok()?)?,
                _ => return None,
            };
            (ch, end + 1)
        } else {
            (c, c.len_utf8())
        };
        plain.push(ch);
        // A tab lays out as four spaces.
        let n = if ch == '\t' { 4 } else { 1 };
        idx.extend(std::iter::repeat_n(cur, n));
        rest = &rest[len..];
    }
    let expected = tsheet.text.as_deref().unwrap_or("");
    if plain.replace("\r\n", "\n") != expected.replace("\r\n", "\n") {
        return None;
    }
    // Only the IX values the text references get a style, compacted to
    // 0..n: a document-controlled `<cp IX='1000000000'/>` must not size
    // an allocation (Greptile on PR #350). Index 0 stays the first
    // referenced row, so `style_at`'s fallback is a real style.
    let mut used: Vec<usize> = idx.clone();
    used.sort_unstable();
    used.dedup();
    for i in idx.iter_mut() {
        *i = used.binary_search(i).unwrap_or(0);
    }
    // `themed`: also take a `THEMEVAL()` cell (not `Inh`) as the row's
    // value instead of looking past it to the master's row.
    let row_cell_with = |ix: usize, name: &str, themed: bool| -> Option<Cell> {
        let find = |sh: &Sheet| {
            sh.sections_named("Character")
                .into_iter()
                .next()
                .and_then(|sec| sec.rows.iter().find(|r| r.ix == ix as i64))
                .and_then(|r| r.cell(name))
                .filter(|c| {
                    (!c.is_inherited() && !c.is_themed()) || (themed && c.is_theme_eval())
                })
                .cloned()
        };
        find(&s.sheet).or_else(|| doc.instance_source(s, chain).and_then(|m| find(&m.sheet)))
    };
    let row_cell = |ix: usize, name: &str| row_cell_with(ix, name, false);
    let styles: Vec<RunStyle> = used
        .iter()
        .map(|&ix| {
            // A run colored `THEMEVAL()` resolves like the block's own
            // color: a theme variant when the theme defines it, else the
            // No Style black. Skipping it painted the run in the first
            // row's color (the network LLD: address lines under a green
            // heading came out green, Visio draws them black).
            let color = row_cell_with(ix, "Color", true)
                .map(|c| color_value(doc, c.v.as_deref().unwrap_or(""), c.f.as_deref()))
                .filter(|c| !c.eq_ignore_ascii_case(&tb.color));
            let bits = row_cell(ix, "Style")
                .and_then(|c| c.numeric())
                .map(|v| v as u32);
            let bold = bits.map(|b| b & 1 != 0).filter(|b| *b != tb.bold);
            let italic = bits.map(|b| b & 2 != 0).filter(|i| *i != tb.italic);
            let underline = bits.is_some_and(|b| b & 4 != 0);
            let strike = row_cell(ix, "Strikethru")
                .and_then(|c| c.numeric())
                .is_some_and(|v| v != 0.0)
                || row_cell(ix, "DoubleStrikethrough")
                    .and_then(|c| c.numeric())
                    .is_some_and(|v| v != 0.0);
            let size = row_cell(ix, "Size")
                .and_then(|c| c.numeric())
                .filter(|z| (z - tb.font_size).abs() > 1e-6);
            RunStyle {
                color,
                bold,
                italic,
                size,
                underline,
                strike,
            }
        })
        .collect();
    if styles.iter().all(|st| *st == RunStyle::default()) {
        return None;
    }
    Some(TextRuns {
        per_char: idx,
        styles,
    })
}

/// `#rrggbb` (or `#rgb`) → (r, g, b) bytes; black on anything else —
/// the TextBkgnd path emits the fill as explicit channels plus a
/// fill-opacity (#229), so it never leaks an unescaped attribute.
pub(crate) fn hex_rgb(color: &str) -> (u8, u8, u8) {
    let h = color.trim().trim_start_matches('#');
    let v = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0);
    match h.len() {
        6 => (
            v(h.get(0..2).unwrap_or("00")),
            v(h.get(2..4).unwrap_or("00")),
            v(h.get(4..6).unwrap_or("00")),
        ),
        3 => {
            let (r, g, b) = (
                h.get(0..1).unwrap_or("0"),
                h.get(1..2).unwrap_or("0"),
                h.get(2..3).unwrap_or("0"),
            );
            let d = |s: &str| v(&format!("{s}{s}"));
            (d(r), d(g), d(b))
        }
        _ => (0, 0, 0),
    }
}

/// The UNBULLETED first-line indent in inches (PR #284 review round
/// 3, per the IndFirst/IndLeft docs): IndLeft is the base indent for
/// every line and IndFirst adjusts the FIRST line relative to it —
/// floored at 0. Consumed by the wrap (`first_off`), the paint band
/// and the TEXTHEIGHT measure together, so all three stay on one
/// layout.
pub(crate) fn unbulleted_first_offset_in(tb: &TextBlock) -> f64 {
    (tb.ind_left + tb.ind_first).max(0.0)
}

/// The measured #231 tab-stop rule, shared by the renderer and the
/// TEXTHEIGHT measure (PR #284 review — the measure must wrap at the
/// same first-line width the renderer lays out): TextPosAfterBullet
/// when it lies past the bullet's start (outright — Visio does not
/// take it as a minimum with the tab grid), else the hanging indent
/// (IndLeft when IndFirst is negative) and the next DefaultTabStop
/// multiple, whichever comes first; a bullet reaching past the stop
/// pushes the text to its end. Returns the stop as a TEXT-AREA
/// offset in INCHES, `None` for a non-bulleted block.
/// The bullet glyph's advance in inches — shared by the tab-stop
/// rule and the ink bounds (an outdented bullet hangs LEFT of the
/// text frame by its origin; the advance bounds the glyph itself).
pub(crate) fn bullet_advance_in(tb: &TextBlock, b: &BulletSpec) -> f64 {
    match &b.glyph {
        // The built-in round bullets draw as paths in a fixed em box
        // (see `bullet_glyph_svg`); the advance is that box's width —
        // a documented approximation.
        BulletGlyph::Round(_) => 0.36 * b.size,
        BulletGlyph::Str(glyph) => {
            let bface = crate::textmetrics::resolve(
                b.font.as_deref().or(tb.font.as_deref()),
                tb.bold,
                tb.italic,
            );
            bface.width_in(glyph, b.size)
        }
    }
}

pub(crate) fn bullet_stop_offset_in(tb: &TextBlock) -> Option<f64> {
    let b = tb.bullet.as_ref()?;
    let bullet_off = tb.ind_first;
    let adv = bullet_advance_in(tb, b);
    let bullet_end_off = bullet_off + adv;
    let stop = if b.pos_after > bullet_off {
        b.pos_after
    } else {
        let mut s = f64::INFINITY;
        if tb.ind_first < 0.0 {
            s = s.min(tb.ind_left);
        }
        let tab = (bullet_end_off / b.tab_stop).ceil() * b.tab_stop;
        s.min(tab)
    };
    Some(stop.max(bullet_end_off))
}

/// The built-in round bullet's drawn glyph as a PATH (#231): Word's
/// round set (styles 1–7) draws as •, "o", ▪, ❑, ❖, ➢, ✓ — emitted
/// as geometric paths in the TEXT fill color so no viewer font
/// substitution can change their size or style (the custom-string
/// bullets stay text). Each glyph is constructed in a 1-em local
/// frame centered on (x, y) and scaled by `size`; our own cleanroom
/// geometry, sized to read like Visio's marks.
fn bullet_glyph_svg(kind: u8, x: f64, y: f64, size: f64, color: &str) -> String {
    // The measured Visio marks, drawn from scratch (cleanroom: never
    // a copy of the Wingdings/Symbol outlines):
    let d = match kind {
        // 1 • filled dot
        1 => "M-0.14 0a0.14 0.14 0 1 0 0.28 0a0.14 0.14 0 1 0 -0.28 0Z",
        // 2 "o" — a ring (Courier New's lowercase o reads as a ring)
        2 => "M0 -0.15a0.15 0.15 0 1 0 0.001 0ZM0 -0.08a0.08 0.08 0 1 1 -0.001 0Z",
        // 3 ▪ filled square
        3 => "M-0.13 -0.13h0.26v0.26h-0.26Z",
        // 4 ❑ box with an inner void (the shadow-box reading of
        // Wingdings q)
        4 => "M-0.15 -0.15h0.30v0.30h-0.30ZM-0.08 -0.08h0.16v0.16h-0.16Z",
        // 5 ❖ four-diamond star
        5 => {
            "M0 -0.26l0.10 0.12l-0.10 0.12l-0.10 -0.12Z\
              M0 0.02l0.10 0.12l-0.10 0.12l-0.10 -0.12Z\
              M-0.19 -0.12l0.10 0.12l-0.10 0.12l-0.10 -0.12Z\
              M0.19 -0.12l0.10 0.12l-0.10 0.12l-0.10 -0.12Z"
        }
        // 6 ➢ concave arrowhead pointing right
        6 => "M-0.14 -0.15l0.29 0.15l-0.29 0.15l0.09 -0.15Z",
        // 7 ✓ a stroked check
        _ => "",
    };
    let scale = format!("translate({x:.4},{y:.4}) scale({s:.5})", s = size);
    if kind == 7 {
        format!(
            r#"<path transform="{t}" d="M-0.15 0.01l0.11 -0.13l0.21 0.23" fill="none" stroke="{c}" stroke-width="0.09" stroke-linecap="round" stroke-linejoin="round"/>"#,
            t = scale,
            c = escape_attr(color),
        )
    } else {
        format!(
            r#"<path transform="{t}" d="{d}" fill="{c}" fill-rule="evenodd"/>"#,
            t = scale,
            c = escape_attr(color),
        )
    }
}

/// A cell of the first Character row, inherited per-cell the way Visio
/// resolves character formatting: the instance's own row first (master
/// instances typically carry a LangID-only row), then the source shape's
/// row via the master link, then the text-style stylesheet cascade. A row
/// that omits a cell inherits it — sections inherit separately from
/// cells, and `effective_sheet` stops at the instance sheet as soon as it
/// has any section (the same gap the Paragraph lookup in `emit_text`
/// works around).
fn char_cell<'a>(
    doc: &'a Document,
    s: &'a Shape,
    chain: &[&'a Shape],
    name: &str,
) -> Option<&'a Cell> {
    let first_row = |sheet: &'a Sheet| {
        sheet
            .sections_named("Character")
            .into_iter()
            .next()
            .and_then(|sec| sec.rows.first())
    };
    if let Some(c) = first_row(&s.sheet).and_then(|r| r.cell(name)) {
        return Some(c);
    }
    let source = doc.instance_source(s, chain);
    if let Some(src) = source {
        if let Some(c) = first_row(&src.sheet).and_then(|r| r.cell(name)) {
            return Some(c);
        }
    }
    // Text-style stylesheet cascade. The instance usually leaves the
    // style attributes to its master root; stylesheets reference their
    // parents through a TextStyle attribute.
    let style_id = s
        .text_style
        .or_else(|| source.and_then(|src| src.text_style))?;
    let mut cur = doc.stylesheet(style_id)?;
    for _ in 0..8 {
        if let Some(c) = first_row(&cur.sheet).and_then(|r| r.cell(name)) {
            return Some(c);
        }
        let next_id = cur
            .attrs
            .iter()
            .find(|(k, _)| k == "TextStyle")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .or_else(|| {
                cur.sheet
                    .cell("TextStyle")
                    .and_then(|c| c.numeric())
                    .map(|v| v as i64)
            })?;
        if next_id == cur.id {
            return None;
        }
        cur = doc.stylesheet(next_id)?;
    }
    None
}

/// First Character-section row's Size cell (inches), per-cell inherited
/// through `char_cell`.
fn char_size(doc: &Document, s: &Shape, chain: &[&Shape]) -> Option<f64> {
    char_cell(doc, s, chain, "Size")
        .and_then(|c| c.numeric())
        .filter(|v| *v > 0.0)
}

/// First Character-section row's Font cell — the family name as stored
/// in VSDX (`<Cell N='Font' V='Arial'/>`), inherited per-cell like Size.
/// Legacy binary-format font indexes (numeric values) and the `Themed`
/// placeholder (meaning "the theme's font", rendered with Visio's default
/// family) name no family.
pub(crate) fn char_font(doc: &Document, s: &Shape, chain: &[&Shape]) -> Option<String> {
    let name = char_cell(doc, s, chain, "Font").and_then(|c| c.v.clone())?;
    let name = name.trim();
    if name.is_empty() || name.parse::<f64>().is_ok() || name.eq_ignore_ascii_case("themed") {
        return None;
    }
    Some(name.to_string())
}

/// Metric-compatible substitutes for the Microsoft core fonts (the
/// libre designer-alternative set), with the CSS generic each family
/// degrades to. Case-insensitive lookup.
fn font_substitute(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "arial" => ("Liberation Sans", "sans-serif"),
        "calibri" => ("Carlito", "sans-serif"),
        "cambria" => ("Caladea", "serif"),
        "segoe ui" => ("Carlito", "sans-serif"),
        "times new roman" => ("Liberation Serif", "serif"),
        "courier new" => ("Liberation Mono", "monospace"),
        // The other stock monospace faces: without the face installed
        // they must stay monospace (Visio 2021 witness: the network
        // LLD's Lucida Console interface labels drew in Carlito and
        // re-wrapped). Liberation Mono's 0.6 em advance matches Lucida
        // Console's; Consolas (0.55 em) is the closest the vendored
        // set offers.
        "lucida console" | "consolas" => ("Liberation Mono", "monospace"),
        _ => return None,
    })
}

/// The stock-font rule (GitHub #232, cleanroom-rebuilt from the
/// observed Visio behavior documented on the issue — never Microsoft's
/// 273-entry table): a font NOT in Visio's stock set draws in
/// **Calibri** on a stock Windows + Office install, whatever the
/// document's root style says — Visio's own SVG even names unknown
/// families Calibri. So a document naming Helvetica, Roboto, or an
/// invented family must lay out in our Calibri metric twin (Carlito),
/// or its TEXTWIDTH-driven wrapping diverges from Visio's (Calibri is
/// narrower than the look-alike faces a generic fallback picks).
///
/// The table is the well-known Windows/Office core set a stock Visio
/// install offers; extend it only with evidence. Case-insensitive.
fn is_stock_visio_font(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "arial"
            | "arial black"
            | "calibri"
            | "cambria"
            | "candara"
            | "comic sans ms"
            | "consolas"
            | "constantia"
            | "corbel"
            | "courier new"
            | "ebrima"
            | "franklin gothic medium"
            | "gabriola"
            | "gadugi"
            | "georgia"
            | "impact"
            | "lucida console"
            | "lucida sans unicode"
            | "malgun gothic"
            | "microsoft himalaya"
            | "microsoft jhenghei"
            | "microsoft yahei"
            | "mongolian baiti"
            | "ms gothic"
            | "ms mincho"
            | "ms pgothic"
            | "mv boli"
            | "palatino linotype"
            | "segoe script"
            | "segoe serif"
            | "segoe ui"
            | "simsun"
            | "sylfaen"
            | "symbol"
            | "tahoma"
            | "times new roman"
            | "trebuchet ms"
            | "verdana"
            | "webdings"
            | "wingdings"
            | "yu gothic"
    )
}

/// The measurement/paint infrastructure faces (#232): families THIS
/// app names itself (the vendored deterministic floor, the #257 RTL
/// fallback, the #282 CJK fallback) resolve as themselves — they never
/// appear in Visio files, so exempting them has no parity cost, and
/// the RTL fallback would regress to Carlito if the stock rule
/// swallowed them.
fn is_app_font(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "carlito"
            | "liberation sans"
            | "liberation serif"
            | "liberation mono"
            | "vazirmatn"
            | "noto sans sc"
            | "noto sans jp"
            | "noto sans kr"
    )
}

/// The measured Character.Pos metrics for a family (#230): the
/// reduced glyph em and the baseline shift in NOMINAL em (positive =
/// raised). Only Calibri and Arial were witnessed (a 36-case + 36-
/// control measurement on the issue): Calibri 0.667 reduced with a
/// 0.3-em superscript raise, Arial 1331/2048 ≈ 0.6499 with 0.477;
/// the subscript drop is −0.14 for both. Our metric twins inherit the
/// numbers (Carlito = Calibri, Liberation Sans = Arial); the #232
/// stock rule already reads non-stock names as Calibri, so a Roboto
/// label scripts with Calibri's numbers — exactly what Visio draws.
/// Every other family returns `None`: the block keeps its nominal
/// draw until that face is independently witnessed.
pub(crate) fn script_metrics(font: Option<&str>, pos: i32) -> Option<(f64, f64)> {
    if pos != 1 && pos != 2 {
        return None;
    }
    let named = font.map(str::trim).filter(|f| !f.is_empty());
    let key = match named {
        None => "calibri".to_string(), // the default family IS Calibri
        Some(n) if !is_stock_visio_font(n) && !is_app_font(n) => "calibri".to_string(),
        Some(n) => n.to_ascii_lowercase(),
    };
    let (reduced, super_shift) = match key.as_str() {
        "calibri" | "carlito" => (0.667, 0.3),
        "arial" | "liberation sans" => (1331.0 / 2048.0, 0.477),
        _ => return None,
    };
    let shift = match pos {
        1 => super_shift,
        _ => -0.14,
    };
    Some((reduced, shift))
}

/// Quote a font-family name for the CSS shorthand (single quotes when
/// it contains anything but letters, digits and dashes). Inside the
/// quotes, `'` and `\` are CSS-escaped so a document-supplied family
/// name can never break out of the quoted string.
fn quote_family(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// `font-family` fallback chain for a document font (GitHub #17):
/// the document's own family first (the best face on any viewer),
/// then its metric-compatible substitute if the alias table covers
/// it, then Carlito (the GUI's embedded face — also a sensible
/// last-name fallback for viewers), then the CSS generic class.
/// Text with no Font cell gets Visio's default family — Calibri —
/// through the same table (Calibri → Carlito). Deduplicated
/// case-insensitively.
/// The font-family fallback chain (UNQUOTED names, CSS generic
/// last) the renderer declares for a shape's resolved Character Font
/// — the same walk [`crate::textmetrics`] measures with, so the wrap
/// decision and the raster use one resolution (#21).
///
/// #232 stock-font rule: a family OUTSIDE Visio's stock set (and not
/// one of our own infrastructure faces) resolves to the Calibri chain
/// outright — named first, substituted last, like Visio's own SVG
/// which renames unknown families Calibri. Wrap and raster follow one
/// chain, so a Helvetica or Roboto label lays out exactly where Visio
/// would draw it (Carlito = Calibri metrics) instead of drifting onto
/// a wider look-alike.
pub(crate) fn family_chain(font: Option<&str>) -> Vec<String> {
    let name = match font.map(str::trim) {
        Some(f) if !f.is_empty() => f,
        _ => "Calibri",
    };
    if !is_stock_visio_font(name) && !is_app_font(name) {
        return vec![
            "Calibri".to_string(),
            "Carlito".to_string(),
            "sans-serif".to_string(),
        ];
    }
    let (substitute, class) = match font_substitute(name) {
        Some((s, c)) => (Some(s), c),
        None => (None, "sans-serif"),
    };
    let mut chain = vec![name.to_string()];
    for c in substitute.into_iter().chain(["Carlito"]) {
        if !chain.iter().any(|f| f.eq_ignore_ascii_case(c)) {
            chain.push(c.to_string());
        }
    }
    chain.push(class.to_string());
    chain
}

pub(crate) fn font_family_attr(font: Option<&str>) -> String {
    let chain: Vec<String> = family_chain(font).iter().map(|f| quote_family(f)).collect();
    // Document-supplied family names land in an XML attribute — the
    // joined CSS list is attribute content like any other.
    format!(" font-family=\"{}\"", escape_attr(&chain.join(", ")))
}

