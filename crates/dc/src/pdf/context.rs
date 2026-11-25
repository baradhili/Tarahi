//! The PDF export context — the shared record types and the
//! registries (faces, images, shadings, graphics states) every other
//! module drives.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use pdf_writer::Ref;
use resvg::usvg;
use resvg::usvg::fontdb;
use resvg::usvg::tiny_skia_path;

use super::font::face_covers;

/// One glyph as shaped by harfrust, in font units.
#[derive(Clone)]
pub(super) struct ShapedGlyph {
    pub(super) id: u16,
    /// Byte offset of the cluster start inside the shaped substring.
    pub(super) cluster: usize,
    pub(super) x_advance: f32,
    pub(super) x_offset: f32,
}

/// A run of glyphs all drawn with one face; `text` is the substring
/// the run shaped (cluster offsets index it).
pub(super) struct GlyphRun {
    pub(super) face: FaceKey,
    pub(super) text: String,
    pub(super) glyphs: Vec<ShapedGlyph>,
}

/// Identity of an embedded font: the fontdb face the chain resolved
/// to. The db is process-global, so the ID is a stable key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct FaceKey(fontdb::ID);

/// The PDF object refs of one embedded face (allocated on first use
/// during emission, dicts written at assembly — /W and ToUnicode are
/// only known once every page has been walked).
#[derive(Clone)]
pub(super) struct FaceRefs {
    pub(super) type0: Ref,
    pub(super) cid: Ref,
    pub(super) descriptor: Ref,
    pub(super) file: Ref,
    pub(super) to_unicode: Ref,
    /// PostScript name for /BaseFont.
    pub(super) postscript: String,
    /// The content-stream resource name (`F0`, `F1`, …), unique per
    /// registered face (#218 review): PostScript names collide — a
    /// system face and its vendored twin, and every empty name
    /// collapses to one string — while the /Font dict is keyed by
    /// this name. Duplicate keys would draw one face's glyph IDs
    /// with the other face's font.
    pub(super) res_name: &'static str,
}

/// One decoded image XObject; `name` is the content-stream name and
/// the resources key (leaked once — a PDF's handful of names).
pub(super) struct ImageEntry {
    pub(super) object: Ref,
    pub(super) name: &'static str,
}

/// The shading pattern of one gradient definition.
pub(super) struct ShadeEntry {
    pub(super) pattern: Ref,
    pub(super) name: &'static str,
}

/// The PDF context: registries shared by every page (one font
/// embedding per face for the whole file), a Ref counter, and the
/// caches keyed by tree-object pointer identity. Insertion-order Vecs
/// keep the assembly deterministic (HashMap iteration is not).
pub(super) type FaceBytes = Option<(Arc<Vec<u8>>, u32)>;

pub(super) struct PdfContext<'db> {
    pub(super) db: &'db Arc<fontdb::Database>,
    pub(super) next_ref: i32,
    pub(super) faces: HashMap<FaceKey, FaceRefs>,
    pub(super) face_order: Vec<FaceKey>,
    /// Glyph → nominal width (1/1000-em units) per face.
    pub(super) widths: HashMap<FaceKey, BTreeMap<u16, f32>>,
    /// Glyph → Unicode text per face (the ToUnicode CMap source).
    pub(super) to_unicode: HashMap<FaceKey, BTreeMap<u16, Vec<char>>>,
    pub(super) images: HashMap<usize, ImageEntry>,
    pub(super) image_order: Vec<usize>,
    pub(super) shadings: HashMap<(usize, [u32; 6]), ShadeEntry>,
    pub(super) shading_order: Vec<(usize, [u32; 6])>,
    /// char → fallback face for glyphs the primary face lacks.
    pub(super) fallbacks: HashMap<char, Option<FaceKey>>,
    /// Face bytes, read once per face (#218 review round 3): the
    /// fallback scan probes every database face per uncovered char,
    /// and file-backed system fonts re-read megabytes each time.
    pub(super) face_cache: HashMap<FaceKey, FaceBytes>,
    /// Owned shaper fonts, one per face (#278 review): the 0.15 API's
    /// `Font` owns its byte copy where the old `FontRef` zero-copy
    /// borrowed the cached bytes, so it is built once per face — a
    /// cached `None` remembers a face that cannot load. Clones share
    /// the instance (read-fonts model fonts), so handing one out
    /// re-copies nothing.
    pub(super) shaper_fonts: HashMap<FaceKey, Option<harfrust::Font>>,
    pub(super) image_counter: u32,
    pub(super) shade_counter: u32,
    /// Constant-alpha graphics states, keyed by (fill, stroke) alpha in
    /// 1/1000 steps: one /ExtGState per distinct pair, in first-use
    /// order (deterministic assembly).
    pub(super) gstates: HashMap<(u16, u16), &'static str>,
    pub(super) gstate_order: Vec<((u16, u16), Ref, &'static str)>,
    /// The product of the enclosing groups' `opacity` while walking.
    pub(super) alpha: f32,
    /// The page content's base transform (px → points, y-flip): a
    /// shading pattern's /Matrix ignores the CTM at paint time, so
    /// the gradient must bake it plus the node's abs transform.
    pub(super) base: tiny_skia_path::Transform,
}

impl<'db> PdfContext<'db> {
    pub(super) fn new(db: &'db Arc<fontdb::Database>) -> Self {
        Self {
            db,
            next_ref: 1,
            faces: HashMap::new(),
            face_order: Vec::new(),
            widths: HashMap::new(),
            to_unicode: HashMap::new(),
            images: HashMap::new(),
            image_order: Vec::new(),
            shadings: HashMap::new(),
            shading_order: Vec::new(),
            fallbacks: HashMap::new(),
            face_cache: HashMap::new(),
            shaper_fonts: HashMap::new(),
            image_counter: 0,
            shade_counter: 0,
            gstates: HashMap::new(),
            gstate_order: Vec::new(),
            alpha: 1.0,
            base: tiny_skia_path::Transform::identity(),
        }
    }

    pub(super) fn alloc_ref(&mut self) -> Ref {
        let r = Ref::new(self.next_ref);
        self.next_ref += 1;
        r
    }

    /// A content-stream / resources name leaked for 'static.
    pub(super) fn leak_name(prefix: &str, n: u32) -> &'static str {
        Box::leak(format!("{prefix}{n}").into_boxed_str())
    }

    /// The face bytes + index for a FaceKey, read from the db.
    pub(super) fn face_data(&mut self, key: FaceKey) -> FaceBytes {
        if let Some(hit) = self.face_cache.get(&key) {
            return hit.clone();
        }
        let info = self.db.face(key.0)?;
        let data = match &info.source {
            // The trait object has no Clone: copy its bytes once,
            // then the memo serves every later call.
            fontdb::Source::Binary(arc) => {
                let bytes: &[u8] = (**arc).as_ref();
                Arc::new(bytes.to_vec())
            }
            fontdb::Source::File(path) => std::fs::read(path).ok().map(Arc::new)?,
            fontdb::Source::SharedFile(path, _) => std::fs::read(path).ok().map(Arc::new)?,
            #[allow(unreachable_patterns)] // memmap-off builds
            _ => return None,
        };
        let hit: FaceBytes = Some((data, info.index));
        self.face_cache.insert(key, hit.clone());
        hit
    }

    /// The shaper font for a face, built once and cached (see
    /// [`Self::shaper_fonts`]) — shaping a text span re-borrows it
    /// instead of re-copying the face bytes (Greptile, #278).
    pub(super) fn shaper_font(&mut self, key: FaceKey) -> Option<harfrust::Font> {
        if let Some(hit) = self.shaper_fonts.get(&key) {
            return hit.clone();
        }
        let built = self
            .face_data(key)
            .and_then(|(data, index)| harfrust::Font::new(data.to_vec(), index));
        self.shaper_fonts.insert(key, built.clone());
        built
    }

    /// Query the db for the face a chunk's span asks for. The SVG's
    /// font-family chains were already resolved to ONE canonical
    /// installed family by `raster::parse_svg`, so the query is
    /// exact-family; weight/style come from the span.
    pub(super) fn query_face(&self, font: &usvg::Font) -> Option<FaceKey> {
        let families: Vec<fontdb::Family> = font
            .families()
            .iter()
            .map(|f| match f {
                usvg::FontFamily::Named(name) => fontdb::Family::Name(name.as_str()),
                usvg::FontFamily::Serif => fontdb::Family::Serif,
                usvg::FontFamily::SansSerif => fontdb::Family::SansSerif,
                usvg::FontFamily::Monospace => fontdb::Family::Monospace,
                usvg::FontFamily::Cursive => fontdb::Family::Cursive,
                usvg::FontFamily::Fantasy => fontdb::Family::Fantasy,
            })
            .collect();
        let query = fontdb::Query {
            families: &families,
            weight: fontdb::Weight(font.weight()),
            style: fontdb::Style::from(font.style()),
            stretch: fontdb::Stretch::from(font.stretch()),
        };
        self.db.query(&query).map(FaceKey)
    }

    /// A face that can draw `c` when the primary one can't: scans the
    /// database for any face whose cmap covers the char (cached per
    /// char). Vendored + system faces alike.
    pub(super) fn fallback_face(&mut self, c: char) -> Option<FaceKey> {
        if let Some(hit) = self.fallbacks.get(&c) {
            return *hit;
        }
        let mut found: Option<FaceKey> = None;
        for face in self.db.faces() {
            // Coverage scans probe WITHOUT face_data (#218 review):
            // the memo would retain every probed font's bytes for the
            // rest of the export — a system font directory is
            // hundreds of faces for one char lookup. Only faces that
            // actually shape or embed go through face_data.
            if self.face_covers_uncached(face.id, c) {
                found = Some(FaceKey(face.id));
                break;
            }
        }
        self.fallbacks.insert(c, found);
        found
    }

    /// A coverage probe that holds no font bytes past the call:
    /// binary sources borrow the db's own arc, file sources read and
    /// drop. Never touches `face_cache`.
    fn face_covers_uncached(&self, id: fontdb::ID, c: char) -> bool {
        let Some(info) = self.db.face(id) else {
            return false;
        };
        match &info.source {
            fontdb::Source::Binary(arc) => face_covers((**arc).as_ref(), info.index, c),
            fontdb::Source::SharedFile(_, arc) => face_covers((**arc).as_ref(), info.index, c),
            fontdb::Source::File(path) => std::fs::read(path)
                .map(|bytes| face_covers(&bytes, info.index, c))
                .unwrap_or(false),
            #[allow(unreachable_patterns)] // memmap-off builds
            _ => false,
        }
    }

    /// Resolve a face key to refs, registering on first use.
    pub(super) fn face_refs(&mut self, key: FaceKey) -> Option<FaceRefs> {
        if let Some(r) = self.faces.get(&key) {
            return Some(r.clone());
        }
        let postscript = self
            .db
            .face(key.0)
            .map(|i| i.post_script_name.clone())
            .unwrap_or_default();
        let refs = FaceRefs {
            type0: self.alloc_ref(),
            cid: self.alloc_ref(),
            descriptor: self.alloc_ref(),
            file: self.alloc_ref(),
            to_unicode: self.alloc_ref(),
            res_name: Self::leak_name("F", self.face_order.len() as u32),
            postscript,
        };
        self.faces.insert(key, refs.clone());
        self.face_order.push(key);
        Some(refs)
    }
}
