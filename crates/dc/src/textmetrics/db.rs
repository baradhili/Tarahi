use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Every vendored face (GitHub #145): the Carlito set is style-complete
/// (Regular/Bold/Italic/BoldItalic — italic used to measure as the
/// upright face), and the Liberation metric faces (Sans/Serif/Mono × 4
/// styles) make the Arial/Times New Roman/Courier New substitution
/// chains resolve the face they NAME on every machine, not only hosts
/// with Liberation installed. Licenses live beside the files.
const EMBEDDED_FONTS: [&[u8]; 16] = [
    include_bytes!("../../assets/fonts/Carlito-Regular.ttf"),
    include_bytes!("../../assets/fonts/Carlito-Bold.ttf"),
    include_bytes!("../../assets/fonts/Carlito-Italic.ttf"),
    include_bytes!("../../assets/fonts/Carlito-BoldItalic.ttf"),
    include_bytes!("../../assets/fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../../assets/fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../../assets/fonts/LiberationSans-Italic.ttf"),
    include_bytes!("../../assets/fonts/LiberationSans-BoldItalic.ttf"),
    include_bytes!("../../assets/fonts/LiberationSerif-Regular.ttf"),
    include_bytes!("../../assets/fonts/LiberationSerif-Bold.ttf"),
    include_bytes!("../../assets/fonts/LiberationSerif-Italic.ttf"),
    include_bytes!("../../assets/fonts/LiberationSerif-BoldItalic.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-Bold.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-Italic.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-BoldItalic.ttf"),
];

/// CSS generic keywords the chain builder can emit — they resolve to
/// the embedded default, matching the raster db's generic mappings.
pub(super) fn is_generic(name: &str) -> bool {
    matches!(
        name,
        "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui"
    )
}

/// The font database: the vendored faces plus system fonts, or
/// vendored-only for hermetic measurement (system fonts stay the
/// measurement source wherever installed — the vendored set is the
/// deterministic floor, owner note 2026-10-30).
pub(super) struct MetricsDb {
    pub(super) db: fontdb::Database,
    /// The vendored Carlito faces (per-character coverage + the
    /// chain-end default), one per (weight, style) corner.
    pub(super) carlito: fontdb::ID,
    pub(super) carlito_bold: fontdb::ID,
    pub(super) carlito_italic: fontdb::ID,
    pub(super) carlito_bold_italic: fontdb::ID,
}

/// CSS-style distance from the wanted posture (CodeRabbit, #145):
/// an Oblique face serves an italic request ahead of Normal — the
/// same order resvg's fontdb query uses, so measurement and raster
/// pick the same face when a family lacks a true Italic.
fn style_penalty(have: fontdb::Style, want_italic: bool) -> usize {
    use fontdb::Style::{Italic, Normal, Oblique};
    match (want_italic, have) {
        (true, Italic) | (true, Oblique) => 0,
        (true, Normal) => 1,
        (false, Normal) => 0,
        (false, Italic) | (false, Oblique) => 1,
    }
}

/// Rank one face against the wanted (weight, style): upright faces
/// stay preferred for upright text (v1 measured italic as upright),
/// and a family that lacks the wanted style still resolves its
/// nearest face instead of nothing.
pub(super) fn style_rank(f: &fontdb::FaceInfo, want_style: fontdb::Style, target: i32) -> (usize, i32) {
    (
        style_penalty(f.style, want_style == fontdb::Style::Italic),
        (f.weight.0 as i32 - target).abs(),
    )
}

fn build_db(embedded_only: bool) -> MetricsDb {
    let mut db = fontdb::Database::new();
    for bytes in EMBEDDED_FONTS {
        db.load_font_data(bytes.to_vec());
    }
    // Runtime-registered faces (the dc-ui complex-script drop — see
    // `register_face_bytes`); kept so BOTH databases see them no
    // matter which builds first.
    for bytes in pending_faces().lock().unwrap().iter() {
        db.load_font_data(bytes.clone());
    }
    if !embedded_only {
        db.load_system_fonts();
    }
    let pick = |bold: bool, italic: bool| -> Option<fontdb::ID> {
        let want_style = if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        };
        let target = if bold { 700 } else { 400 };
        db.faces()
            .filter(|f| {
                f.families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("Carlito"))
            })
            .filter(|f| matches!(f.style, fontdb::Style::Normal | fontdb::Style::Italic))
            .min_by_key(|f| style_rank(f, want_style, target))
            .map(|f| f.id)
    };
    MetricsDb {
        carlito: pick(false, false).expect("vendored Carlito Regular is embedded"),
        carlito_bold: pick(true, false).expect("vendored Carlito Bold is embedded"),
        carlito_italic: pick(false, true).expect("vendored Carlito Italic is embedded"),
        carlito_bold_italic: pick(true, true).expect("vendored Carlito BoldItalic is embedded"),
        db,
    }
}

/// Kittest/unit determinism: pin the process to the Carlito-only
/// database (see the module docs). Set once by the test harnesses;
/// there is deliberately no way to unset it.
static PIN_EMBEDDED: AtomicBool = AtomicBool::new(false);

/// Faces registered at runtime after this module's vendored set
/// (#257 text-direction coordination: ONE font drop for the complex
/// script face — dc's vendored Vazirmatn — registered into BOTH the
/// metrics and the raster databases, never copied). The list is
/// consulted when each database builds; callers must register before
/// the first measurement — `dc::register_complex_script_faces` (the
/// dc CLI's main and dc-ui's `theme::install_fonts` both call it).
fn pending_faces() -> &'static std::sync::Mutex<Vec<Vec<u8>>> {
    static PENDING: OnceLock<std::sync::Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    PENDING.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Register a face (its file bytes) into the metrics database. Must
/// precede the first `resolve`/`width_in` call — the databases build
/// once, lazily, and consult the pending list at build time.
#[doc(hidden)]
pub fn register_face_bytes(bytes: Vec<u8>) {
    pending_faces().lock().unwrap().push(bytes);
}

/// Test hook for hermetic measurement (see the module docs).
#[doc(hidden)]
pub fn pin_embedded_for_tests() {
    PIN_EMBEDDED.store(true, Ordering::Relaxed);
}

pub(super) fn metrics() -> &'static MetricsDb {
    static SYSTEM: OnceLock<MetricsDb> = OnceLock::new();
    static EMBEDDED: OnceLock<MetricsDb> = OnceLock::new();
    if PIN_EMBEDDED.load(Ordering::Relaxed) {
        EMBEDDED.get_or_init(|| build_db(true))
    } else {
        SYSTEM.get_or_init(|| build_db(false))
    }
}
