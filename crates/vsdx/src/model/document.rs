//! The parsed document and resolution through master and style
//! inheritance.

use std::borrow::Cow;

use crate::opc::Package;

use super::cell::{Cell, Row, Section};
use super::master::Master;
use super::page::Page;
use super::raw::{Attrs, RawEl};
use super::shape::{find_shape_by_id, find_shape_by_name_u, Shape};
use super::sheet::Sheet;
use super::style::{builtin_defaults, StyleSheet};

/// A child of `<VisioDocument>`, in document order (round-trip fidelity:
/// unknown children are kept as [`RawEl`] and re-emitted in place).
#[derive(Debug, Clone, PartialEq)]
pub enum DocItem {
    StyleSheets,
    Colors,
    DocumentSheet,
    Raw(RawEl),
}

/// A media part added at runtime (GitHub #113, picture insertion):
/// the package part (`visio/media/imageN.png`), its bytes, the OWNING
/// page id, and the relationship id that page's `ForeignData`
/// references — the writer materializes the part, the page-part rels
/// entry, and the content-type default.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaRef {
    pub part: String,
    pub bytes: Vec<u8>,
    pub page: i64,
    pub rid: String,
    /// `png` / `jpeg` — extension and content-type family.
    pub ext: String,
}

/// A fully parsed VSDX-family package.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub pages: Vec<Page>,
    /// Runtime-added media parts (#113): empty for parsed-only docs;
    /// the write path merges them (retained-package documents only).
    pub extra_media: Vec<MediaRef>,
    /// Masters in masters-index order.
    pub masters: Vec<Master>,
    /// Stylesheets in document order.
    pub stylesheets: Vec<StyleSheet>,
    /// Color table entries (`Colors`) in document order.
    pub colors: Vec<(i64, String)>,
    /// `<VisioDocument>` children in document order.
    pub items: Vec<DocItem>,
    /// Document-level sheet (`DocumentSheet`).
    pub sheet: Sheet,
    /// `<VisioDocument>` opening-tag attributes (namespaces included).
    pub root_attrs: Attrs,
    /// `<Pages>`/`<Masters>` index root attributes.
    pub pages_attrs: Attrs,
    pub masters_attrs: Attrs,
    /// Main document part name (usually `visio/document.xml`).
    pub doc_part: String,
    /// Pages-index part name (usually `visio/pages/pages.xml`).
    pub pages_part: String,
    /// Masters-index part name, when the document has masters.
    pub masters_part: Option<String>,
    /// The raw source package, retained so unmodeled parts round-trip
    /// byte-for-byte. `None` for documents built in code.
    pub package: Option<Package>,
    /// The theme's variant colors, read once from `package` on first
    /// use: style resolution asks for them on every THEMEVAL cell, and
    /// re-reading the theme XML each time made big pages crawl.
    pub theme_variants: std::sync::OnceLock<Vec<String>>,
}

/// Text Block Format cells without a `Txt`/`Text` prefix. They belong
/// to the text style in Visio, so the style cascade must start at the
/// shape's `TextStyle`, not its line or fill style.
fn is_text_block_cell(name: &str) -> bool {
    matches!(
        name,
        "VerticalAlign"
            | "LeftMargin"
            | "RightMargin"
            | "TopMargin"
            | "BottomMargin"
            | "DefaultTabStop"
    )
}

impl Document {
    pub fn master(&self, id: i64) -> Option<&Master> {
        self.masters.iter().find(|m| m.id == id)
    }
    pub fn stylesheet(&self, id: i64) -> Option<&StyleSheet> {
        self.stylesheets.iter().find(|s| s.id == id)
    }
    pub fn color(&self, ix: i64) -> Option<&String> {
        self.colors.iter().find(|(i, _)| *i == ix).map(|(_, c)| c)
    }

    /// Resolve a page-layout cell (the PageSheet's PageLayout section —
    /// `LineJumpFactorX`, `LineToLineX`, …) through the page's style
    /// inheritance: the PageSheet's own cell first, then the chain its
    /// `LineStyle` attribute opens. Visio writes pages with
    /// `LineStyle="0"` and **no inline layout cells** — the "No Style"
    /// stylesheet in document.xml carries the values, which is how
    /// Visio itself resolves them (GitHub #294: the shipped repro's
    /// page sets nothing and Visio still hops with `LineJumpFactorX =
    /// 0.6667` from stylesheet 0; reading the PageSheet alone fell
    /// back to 1.0). `None` when nothing in the chain carries the cell.
    pub fn resolve_page_layout_cell(&self, page: &Page, name: &str) -> Option<Cell> {
        let sheet = page.sheet()?;
        if let Some(c) = sheet.cell(name) {
            if !c.is_inherited() {
                return Some(c.clone());
            }
        }
        let style_id = sheet
            .attrs
            .iter()
            .find(|(n, _)| n == "LineStyle")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0);
        self.stylesheet_cascade_cell(style_id, name)
    }

    /// The page's resolved `LineToLineX` (inches — the horizontal
    /// clearance between parallel connectors), falling back to Visio's
    /// 0.125 in default when neither the PageSheet nor its style chain
    /// carries it (#294).
    pub fn page_line_to_line_x(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineToLineX")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.125)
    }

    /// The page's resolved `LineToLineY` (inches — the vertical
    /// clearance), falling back to Visio's 0.125 in default like the X
    /// twin (#294; PR #299 review — vertical stored jumps are authored
    /// at the Y spacing, so the Y cells need their own resolver).
    pub fn page_line_to_line_y(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineToLineY")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.125)
    }

    /// The page's resolved `LineJumpFactorX` (Visio's jump-size
    /// multiplier), falling back to Visio's 2/3 default (#294 — the
    /// shipped corpus's "No Style" and the #294 repro both carry
    /// 0.66666666666667; the old PageSheet-only read defaulted to 1.0).
    /// An EXPLICIT 0 is valid ([MS-Learn] LineJumpFactorX cell) and
    /// means zero-size hops — only missing or non-finite/negative
    /// values fall back (#294 review).
    pub fn page_line_jump_factor(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineJumpFactorX")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(2.0 / 3.0)
    }

    /// The page's resolved `LineJumpFactorY` — the vertical hop-size
    /// multiplier, same fallback contract as the X twin (PR #299
    /// review).
    pub fn page_line_jump_factor_y(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineJumpFactorY")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(2.0 / 3.0)
    }

    /// Deep model equality ignoring the retained raw package (part bytes
    /// legitimately differ after a write).
    pub fn same_model(&self, other: &Document) -> bool {
        self.pages == other.pages
            && self.masters == other.masters
            && self.stylesheets == other.stylesheets
            && self.colors == other.colors
            && self.items == other.items
            && self.sheet == other.sheet
            && self.root_attrs == other.root_attrs
            && self.doc_part == other.doc_part
            && self.pages_part == other.pages_part
            && self.masters_part == other.masters_part
    }

    /// The master-side shape whose sheet an instance inherits from.
    ///
    /// `Master='8'` resolves inside master 8 (optionally to its `MasterShape`
    /// sub-shape). A shape carrying only `MasterShape` inherits from that
    /// sub-shape of the nearest enclosing group's master — the standard
    /// layout for group instances. A bare sub-shape of a master-backed group
    /// falls back to ID matching inside that master.
    pub fn instance_source<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
    ) -> Option<&'a Shape> {
        if let Some(mid) = shape.master {
            let m = self.master(mid)?;
            return match shape.master_shape {
                Some(msid) => find_shape_by_id(&m.shapes, msid),
                None => m.shapes.first(),
            };
        }
        // Nearest ancestor that instantiates a master.
        let enclosing = ancestors.iter().rev().find(|a| a.master.is_some())?;
        let m = self.master(enclosing.master?)?;
        if let Some(msid) = shape.master_shape {
            return find_shape_by_id(&m.shapes, msid);
        }
        // Bare sub-shape: match by ID inside the master tree. Visio
        // renumbers inline copies (`Name.NNN` suffix), so fall back to the
        // NameU base when the ID does not exist in the master.
        find_shape_by_id(&m.shapes, shape.id).or_else(|| {
            let name = shape.name_u.as_deref()?;
            let (base, sfx) = name.rsplit_once('.')?;
            if sfx.is_empty() || !sfx.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            find_shape_by_name_u(&m.shapes, base)
        })
    }

    /// Resolve a cell for `shape` walking the inheritance chain:
    /// shape → master root shape (or master sub-shape via `MasterShape`)
    /// → stylesheet (shape's LineStyle/FillStyle/TextStyle default 0) →
    /// built-in defaults.
    /// Like [`resolve_cell`](Self::resolve_cell) but returns `None` when
    /// no explicit value exists on the shape, its master, or the style
    /// cascade — i.e. stops before the document-sheet and builtin
    /// fallbacks. Used to spot values that are only defaults (e.g. 1-D
    /// connectors must not inherit the builtin FillPattern=1).
    pub fn resolve_cell_explicit(
        &self,
        shape: &Shape,
        ancestors: &[&Shape],
        name: &str,
    ) -> Option<Cell> {
        if let Some(c) = shape.sheet.cell(name) {
            if c.v.is_some() && is_xform_cell(name) {
                return Some(c.clone());
            }
            if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                return Some(c.clone());
            }
            if c.is_theme_eval() {
                return self.stylesheet_cascade_cell(0, name);
            }
        }
        if let Some(t) = self.instance_source(shape, ancestors) {
            if let Some(c) = t.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                if c.is_theme_eval() {
                    return self.stylesheet_cascade_cell(0, name);
                }
            }
        }
        let style_id = self.shape_style_for(shape, ancestors, name);
        self.stylesheet_cascade_cell(style_id, name)
    }

    /// Variant colors from `visio/theme/theme1.xml` (DrawingML scheme),
    /// indexed by `THEMEVAL("VariantColorN")` formulas (N = 1…8).
    pub fn theme_variant_colors(&self) -> Vec<String> {
        self.theme_variants_cached().to_vec()
    }

    /// [`Document::theme_variant_colors`] without the copy, parsed once.
    fn theme_variants_cached(&self) -> &[String] {
        self.theme_variants
            .get_or_init(|| self.parse_theme_variant_colors())
    }

    fn parse_theme_variant_colors(&self) -> Vec<String> {
        let Some(pkg) = self.package.as_ref() else {
            return Vec::new();
        };
        let Some(raw) = pkg.part("visio/theme/theme1.xml") else {
            return Vec::new();
        };
        let xml = String::from_utf8_lossy(raw).into_owned();
        let mut out = Vec::new();
        // <vt:varColorN><a:srgbClr val="RRGGBB"/>...</vt:varColorN>
        for n in 1..=8 {
            let tag = format!("varColor{n}");
            let Some(pos) = xml.find(&tag) else { continue };
            let window = &xml[pos..(pos + 300).min(xml.len())];
            if let Some(vpos) = window.find("srgbClr val=\"") {
                let rest = &window[vpos + 13..];
                if let Some(end) = rest.find('"') {
                    if end == 6 {
                        out.push(format!("#{}", &rest[..end]));
                        continue;
                    }
                }
            }
            break;
        }
        out
    }

    /// A Themed `THEMEVAL("VariantColorN")` cell that this document's
    /// theme actually evaluates: it is a real value (the renderer reads
    /// the variant color from the formula), not a theme-less No Style
    /// fallback (Greptile on PR #335).
    fn theme_evaluates(&self, c: &Cell) -> bool {
        let Some(f) = c.f.as_deref() else {
            return false;
        };
        if !c.is_theme_eval() || !f.contains("THEMEVAL") {
            return false;
        }
        let variants = self.theme_variants_cached().len();
        (1..=variants).any(|n| f.contains(&format!("VariantColor{n}")))
    }

    /// The stylesheet a shape's `name` cell cascades into: the shape's
    /// own LineStyle/FillStyle/TextStyle attribute, else its master
    /// source's — a master instance (or sub-shape) written without
    /// style attributes uses the master shape's styles, not No Style
    /// (Visio 2021 witness: network-stencil sub-shapes take their
    /// gradient stops from "Tower Front" & co.).
    pub fn shape_style_for(&self, shape: &Shape, ancestors: &[&Shape], name: &str) -> i64 {
        let pick = |s: &Shape| match name {
            n if n.starts_with("Line") => s.line_style,
            n if n.starts_with("Fill") => s.fill_style,
            n if n.starts_with("Txt") | n.starts_with("Text") | is_text_block_cell(n) => {
                s.text_style
            }
            _ => s.line_style.or(s.fill_style),
        };
        pick(shape)
            .or_else(|| self.instance_source(shape, ancestors).and_then(pick))
            .unwrap_or(0)
    }

    /// The shared stylesheet walk (GitHub #294): `start_id`'s sheet first,
    /// then up the style chain, 8 links deep — the linking `LineStyle`
    /// cell when a style carries one, else the style's own parent
    /// attribute for this kind of cell (`FillStyle` for Fill cells,
    /// `TextStyle` for text-block cells, `LineStyle` otherwise; the
    /// schema carries the link in both places). A `V='Themed'` value is
    /// no value without an evaluated theme ([`Cell::is_themed`]), so the
    /// walk continues past it — the shipped stencils chain
    /// Normal → Theme → No Style through attributes, and the Theme
    /// style's `THEMEVAL()` cells used to stop the walk (or, before the
    /// attribute link was followed, the cell-only walk ended early) and
    /// paint theme-less drawings solid black.
    fn stylesheet_cascade_cell(&self, start_id: i64, name: &str) -> Option<Cell> {
        let mut cur = self.stylesheet(start_id)?;
        // The style's own parent for THIS kind of cell (a style's
        // FillStyle/LineStyle/TextStyle attributes name three parents).
        let parent_attr = match name {
            n if n.starts_with("Fill") => "FillStyle",
            n if n.starts_with("Txt") | n.starts_with("Text") | is_text_block_cell(n) => {
                "TextStyle"
            }
            _ => "LineStyle",
        };
        for _ in 0..8 {
            if let Some(c) = cur.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                // THEMEVAL() with no evaluated theme: the No Style value.
                if c.is_theme_eval() && cur.id != 0 {
                    if let Some(no_style) = self.stylesheet(0) {
                        if let Some(c) = no_style.sheet.cell(name) {
                            if !c.is_themed() {
                                return Some(c.clone());
                            }
                        }
                    }
                    return None;
                }
            }
            // A style without its own value — absent, `Inh`, or Themed
            // with no evaluated theme — inherits from its parent style
            // for this kind of cell: the linking cell when present,
            // else the style's FillStyle/LineStyle/TextStyle attribute
            // (the only link the stencils' Normal → Theme → No Style
            // chain carries).
            let next_id = cur
                .sheet
                .cell("LineStyle")
                .and_then(|c| c.numeric())
                .map(|v| v as i64)
                .or_else(|| {
                    cur.attrs
                        .iter()
                        .find(|(n, _)| n == parent_attr)
                        .and_then(|(_, v)| v.parse::<i64>().ok())
                });
            match next_id {
                Some(nid) if nid != cur.id => match self.stylesheet(nid) {
                    Some(next) => cur = next,
                    None => break,
                },
                _ => break,
            }
        }
        None
    }

    pub fn resolve_cell(&self, shape: &Shape, ancestors: &[&Shape], name: &str) -> Option<Cell> {
        if let Some(c) = shape.sheet.cell(name) {
            // Xform/placement cells keep an instance-local snapshot under
            // F='Inh': Visio caches the resolved position in `V` when the
            // master is instanced (the master's own value is its canvas
            // placement, meaningless on the page). Style cells
            // (LineWeight, FillForegnd, …) live-inherit: master wins.
            if c.v.is_some() && is_xform_cell(name) {
                return Some(c.clone());
            }
            if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                return Some(c.clone());
            }
            // THEMEVAL() with no evaluated theme: the No Style value, else
            // the terminal defaults — never the master's or the style's
            // value (CodeRabbit on PR #335).
            if c.is_theme_eval() {
                return self
                    .stylesheet_cascade_cell(0, name)
                    .or_else(|| self.terminal_default(name));
            }
        }
        // Master inheritance.
        if let Some(t) = self.instance_source(shape, ancestors) {
            if let Some(c) = t.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                if c.is_theme_eval() {
                    return self
                        .stylesheet_cascade_cell(0, name)
                        .or_else(|| self.terminal_default(name));
                }
            }
        }
        // Stylesheet cascade.
        let style_id = self.shape_style_for(shape, ancestors, name);
        if let Some(c) = self.stylesheet_cascade_cell(style_id, name) {
            return Some(c);
        }
        self.terminal_default(name)
    }

    /// The last resort below the style cascade: the document sheet's
    /// own value, else the built-in default.
    fn terminal_default(&self, name: &str) -> Option<Cell> {
        if let Some(c) = self.sheet.cell(name) {
            if !c.is_inherited() {
                return Some(c.clone());
            }
        }
        builtin_defaults()
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| Cell {
                v: Some(v.to_string()),
                u: None,
                f: None,
                attrs: Default::default(),
                raw_inner: None,
            })
    }

    /// The shape (or master shape) whose sheet geometry should be rendered
    /// for `shape` — i.e. the effective sheet after master inheritance.
    pub fn effective_sheet<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
    ) -> Cow<'a, Sheet> {
        if shape.sheet.has_sections() {
            return Cow::Borrowed(&shape.sheet);
        }
        if let Some(src) = self.instance_source(shape, ancestors) {
            return Cow::Borrowed(&src.sheet);
        }
        Cow::Borrowed(&shape.sheet)
    }

    /// Resolve a named User row for `shape` across the master chain: the
    /// instance sheet first, then the master root shape's sheet — the
    /// same layering [`resolve_cell`](Self::resolve_cell) uses for
    /// cells. `effective_sheet` cannot serve this: it is either/or (a
    /// sheet with ANY local sections returns only that sheet), so a real
    /// Visio-authored instance — which virtually always carries local
    /// Geometry/Character snapshots — would hide the master root's
    /// `User.msvStructureType` and friends, breaking container
    /// recognition for exactly the containers that carry the row
    /// (#191).
    pub fn effective_user_row<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
        name: &str,
    ) -> Option<&'a Row> {
        if let Some(r) = shape
            .sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row(name))
        {
            return Some(r);
        }
        let src = self.instance_source(shape, ancestors)?;
        src.sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row(name))
    }

    /// Resolve a section (by name and IX) for `shape` across the master
    /// chain: the instance sheet first, then the master root's — the
    /// same layering [`effective_user_row`](Self::effective_user_row)
    /// uses for rows. Like that accessor, `effective_sheet` cannot
    /// serve this: it is either/or (a sheet with ANY local sections
    /// returns only that sheet), so a real Visio-authored instance
    /// would hide the master root's other sections — the seed
    /// `SetRowCell` materializes a missing section from (#196).
    pub fn effective_section<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
        name: &str,
        ix: i64,
    ) -> Option<&'a Section> {
        if let Some(s) = shape.sheet.section(name, ix) {
            return Some(s);
        }
        let src = self.instance_source(shape, ancestors)?;
        src.sheet.section(name, ix)
    }
}

/// Cells whose `F='Inh'` marks an instance-time snapshot rather than live
/// master inheritance (see `resolve_cell`).
fn is_xform_cell(name: &str) -> bool {
    matches!(
        name,
        "PinX"
            | "PinY"
            | "LocPinX"
            | "LocPinY"
            | "Width"
            | "Height"
            | "Angle"
            | "FlipX"
            | "FlipY"
            | "TxtPinX"
            | "TxtPinY"
            | "TxtLocPinX"
            | "TxtLocPinY"
            | "TxtWidth"
            | "TxtHeight"
            | "TxtAngle"
            | "BeginX"
            | "BeginY"
            | "EndX"
            | "EndY"
    )
}
