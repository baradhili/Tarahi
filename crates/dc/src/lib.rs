//! dc library surface — the SVG renderer and edit CLI logic, shared by
//! the `dc` binary and the `dc-ui` desktop app.

pub mod curve;
pub mod edit;
pub mod export;
pub mod geom;
pub mod pdf;
pub mod raster;
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

/// The vendored CJK face (Noto Sans SC, the Simplified-Chinese subset
/// OTFs, OFL — provenance and license beside the files in
/// `assets/fonts/noto-sc/`), #282. Same one-drop coordination as
/// Vazirmatn: registered into both font databases for the Han run
/// splitting, loaded into egui as the Chinese fallback family.
pub const NOTO_SC_REGULAR: &[u8] = include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Regular.otf");
/// The bold weight of [`NOTO_SC_REGULAR`].
pub const NOTO_SC_BOLD: &[u8] = include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Bold.otf");

/// Register the vendored script faces into BOTH font databases —
/// `textmetrics` (wrap measurement) and `raster` (the usvg fontdb) —
/// so shape text in scripts the Latin chain lacks measures and
/// rasterizes with the vendored faces instead of falling off at the
/// Latin chain: Vazirmatn for the Arabic script (#257), Noto Sans SC
/// for Han (#282 — the run splitting measures Han runs with THIS
/// face's shaped advances, so wrap and paint agree even on hosts
/// carrying system CJK fonts). Idempotent per process (the pending
/// lists are consumed at each database's one lazy build; repeated
/// constructors must not stack copies). Call before the first
/// measurement or raster: `main` does it for the CLI,
/// `theme::install_fonts` for every dc-ui context.
pub fn register_complex_script_faces() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        for bytes in [
            VAZIRMATN_REGULAR,
            VAZIRMATN_BOLD,
            NOTO_SC_REGULAR,
            NOTO_SC_BOLD,
        ] {
            textmetrics::register_face_bytes(bytes.to_vec());
            raster::register_face_bytes(bytes.to_vec());
        }
    });
}
