//! Snap & glue settings stored in `<DocumentSettings>`.

use super::document::{DocItem, Document};
use super::raw::RawEl;

/// Snap & glue configuration stored in `<DocumentSettings>` (GitHub
/// #109). Bit values follow the Visio automation enums so documents
/// interoperate with the shipped app. Snap: GRID 1, RULER_SUBDIVISIONS
/// 2, GUIDES 4, SELECTION_HANDLES 8, VERTICES 16, CONNECTION_POINTS 32,
/// GEOMETRY 64, ALIGNMENT_BOXES 128, SHAPE_EXTENSIONS 256,
/// SHAPE_INTERSECTIONS 512. Glue: GUIDES 1, SHAPE_HANDLES 2,
/// SHAPE_VERTICES 4, CONNECTION_POINTS 8. The shipped Visio 2016 corpus
/// defaults are SnapSettings 295 (grid, ruler subdivisions, guides,
/// connection points, shape extensions), GlueSettings 9 (guides,
/// connection points), DynamicGridEnabled 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocSnapGlue {
    pub snap: u32,
    pub glue: u32,
    pub dynamic_grid: bool,
}

impl DocSnapGlue {
    /// The shipped Visio 2016 corpus defaults (see the struct docs).
    pub const CORPUS_DEFAULT: DocSnapGlue = DocSnapGlue {
        snap: 295,
        glue: 9,
        dynamic_grid: true,
    };
}

/// `SnapSettings` bit values (Visio automation `VisSnapSettings` /
/// `VisSnapSettings2` enums).
pub mod snap_bits {
    pub const GRID: u32 = 1;
    pub const RULER_SUBDIVISIONS: u32 = 2;
    pub const GUIDES: u32 = 4;
    pub const SELECTION_HANDLES: u32 = 8;
    pub const VERTICES: u32 = 16;
    pub const CONNECTION_POINTS: u32 = 32;
    pub const GEOMETRY: u32 = 64;
    pub const ALIGNMENT_BOXES: u32 = 128;
    pub const SHAPE_EXTENSIONS: u32 = 256;
    pub const SHAPE_INTERSECTIONS: u32 = 512;
}

/// `GlueSettings` bit values (Visio automation `VisGlueSettings`).
pub mod glue_bits {
    pub const GUIDES: u32 = 1;
    pub const SHAPE_HANDLES: u32 = 2;
    pub const SHAPE_VERTICES: u32 = 4;
    pub const CONNECTION_POINTS: u32 = 8;
}

/// Canonical `<DocumentSettings>` child order (the corpus shape):
/// a missing snap/glue child is inserted before the first child that
/// follows it in this order, so a patched element keeps Visio's layout.
const SETTINGS_CHILD_ORDER: &[&str] = &[
    "GlueSettings",
    "SnapSettings",
    "SnapExtensions",
    "SnapAngles",
    "DynamicGridEnabled",
    "ProtectStyles",
    "ProtectShapes",
    "ProtectMasters",
    "ProtectBkgnds",
];

/// Text of `<Name>value</Name>` inside `inner`, when present.
fn read_settings_child(inner: &str, name: &str) -> Option<u32> {
    let open = format!("<{name}>");
    let p = inner.find(&open)? + open.len();
    let rest = &inner[p..];
    let end = rest.find(&format!("</{name}>"))?;
    rest[..end].trim().parse().ok()
}

/// Replace (or insert, in canonical position) one settings child.
pub(crate) fn set_settings_child(inner: &str, name: &str, value: &str) -> String {
    let full = format!("<{name}>{value}</{name}>");
    // Paired form: swap the inner text, keep everything else verbatim.
    if let Some(p) = inner.find(&format!("<{name}>")) {
        if let Some(end) = inner[p..].find(&format!("</{name}>")) {
            let close = p + end + name.len() + 3;
            let mut out = String::with_capacity(inner.len() + value.len());
            out.push_str(&inner[..p]);
            out.push_str(&full);
            out.push_str(&inner[close..]);
            return out;
        }
    }
    // Self-closing form: expand in place.
    let sc = format!("<{name}/>");
    if let Some(p) = inner.find(&sc) {
        let mut out = String::with_capacity(inner.len() + value.len());
        out.push_str(&inner[..p]);
        out.push_str(&full);
        out.push_str(&inner[p + sc.len()..]);
        return out;
    }
    // Absent: insert before the first canonically-later child, else
    // append at the end.
    let later = &SETTINGS_CHILD_ORDER[SETTINGS_CHILD_ORDER
        .iter()
        .position(|n| *n == name)
        .map_or(0, |i| i + 1)..];
    for cand in later {
        for form in [format!("<{cand}>"), format!("<{cand}/>")] {
            if let Some(p) = inner.find(&form) {
                let mut out = String::with_capacity(inner.len() + full.len());
                out.push_str(&inner[..p]);
                out.push_str(&full);
                out.push_str(&inner[p..]);
                return out;
            }
        }
    }
    format!("{inner}{full}")
}

impl Document {
    /// The document's snap & glue settings, straight from a
    /// `<DocumentSettings>` element. `None` when the document carries
    /// no such element (a fresh blank package); a present element with
    /// an absent child reads that child as its shipped default.
    pub fn doc_snap_glue(&self) -> Option<DocSnapGlue> {
        let raw = self.items.iter().find_map(|it| match it {
            DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r),
            _ => None,
        })?;
        let d = DocSnapGlue::CORPUS_DEFAULT;
        Some(DocSnapGlue {
            snap: read_settings_child(&raw.inner, "SnapSettings").unwrap_or(d.snap),
            glue: read_settings_child(&raw.inner, "GlueSettings").unwrap_or(d.glue),
            dynamic_grid: read_settings_child(&raw.inner, "DynamicGridEnabled")
                .map(|v| v != 0)
                .unwrap_or(d.dynamic_grid),
        })
    }

    /// Write the snap & glue cells into `<DocumentSettings>`, creating
    /// the element (first child of `VisioDocument`, corpus order) when
    /// the document has none. Only the three modeled children are
    /// touched; attributes and every other child — `SnapExtensions`,
    /// `SnapAngles`, `Protect*` — stay verbatim.
    pub fn set_doc_snap_glue(&mut self, s: DocSnapGlue) {
        let dg = if s.dynamic_grid { "1" } else { "0" };
        if let Some(ix) = self
            .items
            .iter()
            .position(|it| matches!(it, DocItem::Raw(r) if r.name == "DocumentSettings"))
        {
            if let DocItem::Raw(r) = &mut self.items[ix] {
                r.inner = set_settings_child(&r.inner, "GlueSettings", &s.glue.to_string());
                r.inner = set_settings_child(&r.inner, "SnapSettings", &s.snap.to_string());
                r.inner = set_settings_child(&r.inner, "DynamicGridEnabled", dg);
            }
            return;
        }
        // No element yet (the blank package): synthesize one with the
        // corpus child set — SnapExtensions 34 and an empty SnapAngles
        // are the shipped defaults, written once and then verbatim.
        let mut inner = String::new();
        inner = set_settings_child(&inner, "GlueSettings", &s.glue.to_string());
        inner = set_settings_child(&inner, "SnapSettings", &s.snap.to_string());
        inner = set_settings_child(&inner, "SnapExtensions", "34");
        inner.push_str("<SnapAngles/>");
        inner = set_settings_child(&inner, "DynamicGridEnabled", dg);
        self.items.insert(
            0,
            DocItem::Raw(RawEl {
                name: "DocumentSettings".to_string(),
                attrs: Vec::new(),
                inner,
            }),
        );
    }
}
