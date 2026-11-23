//! dc library surface — the SVG renderer and edit CLI logic, shared by
//! the `dc` binary and the `dc-ui` desktop app.

pub mod curve;
pub mod edit;
pub mod export;
pub mod fields;
pub mod geom;
pub mod jumps;
pub mod pdf;
pub mod raster;
pub mod spellcheck;
pub mod svg;
pub mod textfunc;
pub mod textmetrics;

/// The vendored complex-script face (Vazirmatn v33.003, OFL —
/// provenance and license beside the files in
/// `assets/fonts/vazirmatn/`). ONE on-disk drop serves every consumer
/// (PR #269 review): the dc CLI registers it before any export, and
/// dc-ui's `theme::install_fonts` registers the same bytes while also
/// loading them into egui as the Arabic-script fallback family.
pub const VAZIRMATN_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/vazirmatn/Vazirmatn-Regular.ttf");
/// The bold weight of [`VAZIRMATN_REGULAR`].
pub const VAZIRMATN_BOLD: &[u8] = include_bytes!("../assets/fonts/vazirmatn/Vazirmatn-Bold.ttf");

/// The vendored CJK face (#282 zh-Hans wave — Noto Sans SC subset
/// OTFs, OFL; provenance and license beside the files in
/// `assets/fonts/noto-sc/`). Same one-drop contract as Vazirmatn:
/// registered into the measurement and raster databases, and loaded
/// into egui as the CJK fallback family by `theme::install_fonts`.
pub const NOTO_SANS_SC_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Regular.otf");
/// The bold weight of [`NOTO_SANS_SC_REGULAR`].
pub const NOTO_SANS_SC_BOLD: &[u8] = include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Bold.otf");

/// The vendored Japanese face (#282 ja/ko wave — Noto Sans JP subset
/// OTFs, OFL; provenance and license beside the files in
/// `assets/fonts/noto-jp/`). Kana + JIS kanji in the JP regional
/// forms; no Hangul. Same one-drop contract as the SC drop.
pub const NOTO_SANS_JP_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/noto-jp/NotoSansJP-Regular.otf");
/// The bold weight of [`NOTO_SANS_JP_REGULAR`].
pub const NOTO_SANS_JP_BOLD: &[u8] = include_bytes!("../assets/fonts/noto-jp/NotoSansJP-Bold.otf");

/// The vendored Korean face (#282 ja/ko wave — Noto Sans KR subset
/// OTFs, OFL; provenance and license beside the files in
/// `assets/fonts/noto-kr/`). Hangul syllables + KS X 1001 Hanja in
/// the KR regional forms. Same one-drop contract as the SC drop.
pub const NOTO_SANS_KR_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/noto-kr/NotoSansKR-Regular.otf");
/// The bold weight of [`NOTO_SANS_KR_REGULAR`].
pub const NOTO_SANS_KR_BOLD: &[u8] = include_bytes!("../assets/fonts/noto-kr/NotoSansKR-Bold.otf");

/// The CJK face order BOTH the registration below and the measurement
/// chain (`svg.rs`'s fallback chain, which must copy the raster db's
/// per-cluster order) resolve runs through: the family each face
/// answers to in `textmetrics::resolve`. SC leads — the zh-Hans wave's
/// drop — so shared Han codepoints resolve to Simplified forms
/// everywhere and the shipped zh rendering never moves; JP follows for
/// Japanese documents' kana/Kanji the SC subset lacks, KR last for
/// Hangul (only the KR face covers it). The egui shell is the one
/// place the order varies — per-context, it follows the UI language's
/// regional face (`theme::install_fonts`), with no wrap/raster
/// agreement to preserve.
pub const CJK_FACE_ORDER: &[&str] = &["Noto Sans SC", "Noto Sans JP", "Noto Sans KR"];

/// Register the non-Latin fallback faces into BOTH font databases —
/// `textmetrics` (wrap measurement) and `raster` (the usvg fontdb) —
/// so RTL and CJK shape text measures and rasterizes with the vendored
/// faces instead of falling off at the Latin chain. Registration ORDER
/// is the raster's per-cluster fallback order (usvg walks the database
/// as loaded): Vazirmatn ahead of the CJK faces in [`CJK_FACE_ORDER`],
/// matching how the measurement chain resolves runs. Idempotent per
/// process (the pending lists are consumed at each database's one lazy
/// build; repeated constructors must not stack copies). Call before
/// the first measurement or raster: `main` does it for the CLI,
/// `theme::install_fonts` for every dc-ui context.
pub fn register_complex_script_faces() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        for bytes in [
            VAZIRMATN_REGULAR,
            VAZIRMATN_BOLD,
            NOTO_SANS_SC_REGULAR,
            NOTO_SANS_SC_BOLD,
            NOTO_SANS_JP_REGULAR,
            NOTO_SANS_JP_BOLD,
            NOTO_SANS_KR_REGULAR,
            NOTO_SANS_KR_BOLD,
        ] {
            textmetrics::register_face_bytes(bytes.to_vec());
            raster::register_face_bytes(bytes.to_vec());
        }
    });
}
