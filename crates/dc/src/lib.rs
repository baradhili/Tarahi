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

/// The vendored CJK face (#282 zh-Hans wave — Noto Sans SC subset
/// OTFs, OFL; provenance and license beside the files in
/// `assets/fonts/noto-sc/`). Same one-drop contract as Vazirmatn:
/// registered into the measurement and raster databases, and loaded
/// into egui as the CJK fallback family by `theme::install_fonts`.
pub const NOTO_SANS_SC_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Regular.otf");
/// The bold weight of [`NOTO_SANS_SC_REGULAR`].
pub const NOTO_SANS_SC_BOLD: &[u8] =
    include_bytes!("../assets/fonts/noto-sc/NotoSansSC-Bold.otf");

/// Register the non-Latin fallback faces into BOTH font databases —
/// `textmetrics` (wrap measurement) and `raster` (the usvg fontdb) —
/// so RTL and CJK shape text measures and rasterizes with the vendored
/// faces instead of falling off at the Latin chain. Registration ORDER
/// is the raster's per-cluster fallback order (usvg walks the database
/// as loaded): Vazirmatn ahead of Noto Sans SC, matching how the
/// measurement chain resolves runs. Idempotent per process (the
/// pending lists are consumed at each database's one lazy build;
/// repeated constructors must not stack copies). Call before the
/// first measurement or raster: `main` does it for the CLI,
/// `theme::install_fonts` for every dc-ui context.
pub fn register_complex_script_faces() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        for bytes in [
            VAZIRMATN_REGULAR,
            VAZIRMATN_BOLD,
            NOTO_SANS_SC_REGULAR,
            NOTO_SANS_SC_BOLD,
        ] {
            textmetrics::register_face_bytes(bytes.to_vec());
            raster::register_face_bytes(bytes.to_vec());
        }
    });
}
