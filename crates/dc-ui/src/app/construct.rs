//! `App` construction: the headless and windowed constructors, plus
//! the first test-hook cluster the test suite drives state through.

use super::*;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> App {
        // OS light/dark (GitHub #52): detect the opening mode (gsettings
        // with the XDG portal as fallback; `detect` debug-prints the
        // source), install BOTH style slots and select it — then follow
        // runtime flips via a watcher thread. Linux only: elsewhere
        // `detect` yields System and eframe tracks the OS natively.
        let pref = crate::os_theme::detect();
        crate::theme::install_for(&cc.egui_ctx, pref);
        let mut app = App::with_doc_opt(None);
        // #246 i18n: AFTER the shared ctor (which pins English for the
        // headless path), the persisted language — or the system locale
        // on first run — selects the catalog before the first frame.
        crate::i18n::init_from_disk();
        // #224: the branded boot moment — production only (the headless
        // ctors stay splash-free for kittest determinism).
        app.splash = Some(std::time::Instant::now());
        #[cfg(target_os = "linux")]
        {
            app.os_theme = Some(crate::os_theme::OsThemeWatcher::spawn());
        }
        // Production entry only: adopt the persistent on-disk MRU
        // (tests and kittest constructors stay ephemeral so they never
        // touch the user's real config dir).
        app.recent = RecentFiles::open();
        app.quick_shapes = crate::quickshapes::QuickShapes::open();
        app.relative_paths = crate::hyperlink::RelativePaths::open();
        // #121: the persisted custom dictionary, and local dictionary
        // discovery — the headless ctor pinned it, so production
        // re-enables with the user-config `dictionaries/` drop-point
        // joining the system scan.
        app.spell = crate::spellcheck::SpellState::prod();
        // #268: the persisted options (user name/initials, the Save
        // page's AutoRecover settings) — headless ctors stay ephemeral.
        app.options_store = crate::options::Options::open();
        dc::spellcheck::enable_local_discovery();
        dc::spellcheck::register_local_dirs(
            crate::recent::config_base()
                .map(|b| vec![b.join("diagramcraft").join("dictionaries")])
                .unwrap_or_default(),
        );
        // Production entry only: built-ins open from the extracted
        // My Shapes files (#104) — headless constructors stay on the
        // embedded fallback so they never touch the user's home.
        app.stencil_disk = true;
        // #105: the default stencil set auto-opens with the app
        // (Visio's companion set), and More Shapes shows the
        // configured categories in the configured order.
        app.stencil_prefs = crate::stencil::prefs::StencilPrefs::open();
        app.apply_stencil_preferences();
        app.open_default_stencils();
        // #104: the built-in catalog also exists as real `.vssx`
        // files under My Shapes — extracted on a BACKGROUND thread
        // (missing or size-mismatched files rewritten and
        // digest-verified) so users can open, copy, and inspect them
        // like any stencil without the first run blocking or going
        // silent (owner report 2026-10-30). Best-effort: an
        // unwritable My Shapes degrades to the embedded bytes.
        app.stencil_extract = crate::stencil::disk::extract_catalog_async(Self::my_shapes_dir());
        if let Some(p) = path {
            // #242: the CLI entry's open rides the async flight like
            // every interactive open — the splash covers the boot and
            // the scrim the parse + first render; the frames pump the
            // poll from the first frame on. Routed through load() so
            // a .vssx still lands in the Shapes pane (#83, #218
            // review), not the editor.
            app.load(p);
        }
        app
    }

    /// Headless-friendly constructor (theme applied by the host UI loop).
    pub fn with_doc_opt(path: Option<PathBuf>) -> App {
        // Headless constructors pin English so kittest goldens never
        // move with the user's real language (#246).
        crate::i18n::pin_english();
        // …and pin the embedded dictionaries — local discovery stays
        // off so kittest never reads the user's installed hunspell
        // dictionaries (#121, the stencil built-ins' rule).
        dc::spellcheck::pin_embedded();
        let mut app = App {
            ed: None,
            path: PathBuf::new(),
            dirty: false,
            version: 0,
            view: View::default(),
            page_idx: 0,
            page_states: std::collections::HashMap::new(),
            tab_menu: None,
            tab_rename: None,
            tab_scroll_to_active: false,
            reorder_open: false,
            hyperlink_open: None,
            spell: crate::spellcheck::SpellState::headless(),
            options_open: false,
            lang_write_failed: false,
            settings_write_failed: false,
            options_draft: None,
            options_opened_this_frame: false,
            options_store: crate::options::Options::ephemeral(),
            hyperlink_draft: None,
            fields_open: None,
            fields_caret: None,
            last_edit_caret: None,
            fields_draft: None,
            comment_target: None,
            comment_body: String::new(),
            comment_edit: None,
            show_comments: true,
            shape_data_open: None,
            prop_bufs: std::collections::HashMap::new(),
            prop_add: None,
            format_pane_open: false,
            format_pane_focus: None,
            format_pane_last_focus: None,
            fmt_bufs: std::collections::HashMap::new(),
            tab: Tab::Home,
            zoom_selection_pending: false,
            sel: None,
            sel_set: Vec::new(),
            drag: None,
            group_edit: Vec::new(),
            stencil: Stencil::default(),
            stencils: StencilSet::default(),
            stencil_panel_inner: Rect::NOTHING,
            tex: TextureCache::default(),
            pool: crate::rasterpool::RasterPool::default(),
            layers: crate::layers::ShapeLayers::default(),
            info_render_fail_for_test: false,
            last_drag_ghosts: 0,
            last_drag_ghost_rect: None,
            mouse_page: None,
            status: String::new(),
            shapesheets: Vec::new(),
            docs: Vec::new(),
            winmenu_open: false,
            pending_activate: None,
            snap: crate::snap::SnapSettings::default(),
            snap_dlg_open: false,
            line_jump_draft: None,
            snap_live: Vec::new(),
            conn_fx_last: crate::connectors::ConnPointFx::default(),
            drag_glue: None,
            endpoint_preview: None,
            tool: Tool::Pointer,
            align_to_page: false,
            selby_open: false,
            selby_criterion: 0,
            selby_value: String::new(),
            pagesetup_open: false,
            copypic_open: false,
            copypic_factor: 2,
            copypic_svg: false,
            contgal_open: false,
            mdi_client: egui::Rect::NOTHING,
            mdi_last_count: 0,
            mdi_arranging: false,
            mdi_decorated: false,
            mdi_drawing_tile: egui::Rect::NOTHING,
            os_clipboard_text: None,
            os_image_rx: None,
            backgrounds_open: false,
            bg_underlays: Vec::new(),
            last_picture: None,
            painter_armed: None,
            painter_sticky: false,
            connpt_sel: None,
            jumps_last: 0,
            last_endpoint_ghost: false,
            shot_frames: 0,
            // #224: headless default — no branded boot moment (the
            // same ephemeral rule as the MRU adoption below). The
            // production entry re-arms it in `App::new`.
            splash: None,
            loading: None,
            rendering: None,
            about_open: false,
            test_click: false,
            dialog: None,
            canvas_rect: Rect::NOTHING,
            backstage: false,
            backstage_page: BackPage::Info,
            backstage_search: String::new(),
            export_pane: crate::export::ExportPane::default(),
            confirm_new: false,
            new_after_save: false,
            confirm_close: None,
            close_after_save: None,
            close_req_for_test: false,
            closing: false,
            dialog_preset_for_test: None,
            debug_close_vetoes: 0,
            debug_close_issued: 0,
            ctx_menu: None,
            page_menu: None,
            quick_menu: None,
            window_title: String::new(),
            ctx_menu_rect: None,
            clipboard: None,
            text_edit: None,
            placed_last_click: false,
            color_pick: None,
            recent: RecentFiles::ephemeral(),
            quick_shapes: crate::quickshapes::QuickShapes::ephemeral(),
            relative_paths: crate::hyperlink::RelativePaths::ephemeral(),
            stencil_prefs: crate::stencil::prefs::StencilPrefs::ephemeral(),
            stencil_disk: false,
            stencil_extract: None,
            os_theme: None,
            keytips: crate::keytips::Keytips::new(),
        };
        if let Some(p) = path {
            // Headless: no frame loop to pump the poll — land the
            // parse synchronously (the interactive open paths use the
            // async load(); tests assert right after the ctor).
            let result = vsdx::parse::parse(&p).map_err(|e| format!("{e:#}"));
            let _ = app.finish_load(p, result);
        }
        app
    }

    /// Open a document at `path` (headless tests, `dc-ui <file>`).
    pub fn with_doc(path: &std::path::Path) -> App {
        let mut app = App::with_doc_opt(Some(path.to_path_buf()));
        app.splash = None; // headless: no branded boot moment
        app
    }

    /// A fresh blank drawing with the starter shape masters
    /// (headless tests, File → New).
    pub fn with_blank() -> App {
        let mut app = App::with_doc_opt(None);
        app.splash = None; // headless: no branded boot moment
        app.do_new();
        app
    }

    /// Test hooks for the close-prompt flow (GitHub #65). kittest
    /// cannot synthesize a WM_CLOSE or read viewport output, so the
    /// request is flagged in and the sent commands counted out.
    #[doc(hidden)]
    pub fn request_close_for_test(&mut self) {
        self.close_req_for_test = true;
    }
    #[doc(hidden)]
    pub fn mark_dirty_for_test(&mut self) {
        self.dirty = true;
    }
    /// kittest: the saved state (a Save leaves the document clean).
    #[doc(hidden)]
    pub fn mark_clean_for_test(&mut self) {
        self.dirty = false;
    }
    /// #184 tests: the armed keytip level ("strip"/"controls"); None
    /// when disarmed.
    #[doc(hidden)]
    pub fn debug_keytip_mode(&self) -> Option<&'static str> {
        self.keytips.mode.map(|m| match m {
            crate::keytips::Mode::Strip => "strip",
            crate::keytips::Mode::Controls => "controls",
        })
    }
    /// #184 tests: the assigned keytip letter for a content-row
    /// control, looked up by its label in the current badge set.
    #[doc(hidden)]
    pub fn debug_keytip_letter(&self, label: &str) -> Option<char> {
        self.keytips.badges.iter().find_map(|b| match &b.hit {
            crate::keytips::Hit::Action { label: l, .. } if l == label => b.key.chars().next(),
            _ => None,
        })
    }
    /// #184 tests: the active ribbon tab (a strip keytip's effect).
    #[doc(hidden)]
    pub fn debug_tab(&self) -> Tab {
        self.tab
    }
    /// #184 tests: the View ▸ Rulers toggle (a control keytip's
    /// observable effect).
    #[doc(hidden)]
    pub fn debug_show_rulers(&self) -> bool {
        self.view.show_rulers
    }
    /// #192 4a tests: File > Open ADDS a document — parse the file
    /// and activate it, stashing the current one (the real load()
    /// path minus the file-dialog worker).
    #[doc(hidden)]
    pub fn debug_load_adds_for_test(&mut self, path: &std::path::Path) {
        // Synchronous landing (tests assert right after): the same
        // completion path the worker uses, without the flight.
        self.debug_load_sync_for_test(path);
    }

    /// #242: the synchronous open landing for unit tests — `load()`
    /// itself is the async starter now, and these tests assert
    /// immediately after the call.
    #[doc(hidden)]
    pub fn debug_load_sync_for_test(&mut self, path: &std::path::Path) {
        let result = vsdx::parse::parse(path).map_err(|e| format!("{e:#}"));
        let _ = self.finish_load(path.to_path_buf(), result);
    }

    /// #263 tests: the production CLI dispatch exactly as `App::new`
    /// runs it — a `.vssx` argument lands in the Shapes pane, anything
    /// else arms the async document flight.
    #[doc(hidden)]
    pub fn debug_load_for_test(&mut self, path: std::path::PathBuf) {
        self.load(path);
    }

    /// #192 4a tests: the open documents' file names (active first,
    /// then the inactive sessions).
    #[doc(hidden)]
    pub fn debug_open_doc_names(&self) -> Vec<String> {
        let title = |p: &std::path::Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| crate::i18n::strs().msg_untitled())
        };
        let mut names = vec![title(&self.path)];
        names.extend(self.docs.iter().map(|d| title(&d.path)));
        names
    }

    /// #192 4a tests: how many documents are open.
    #[doc(hidden)]
    pub fn debug_open_doc_count(&self) -> usize {
        self.docs.len() + self.ed.is_some() as usize
    }
    /// kittest (#304): swap the Options store (the language-save
    /// failure tests install a PERSISTING store at a tempdir).
    #[doc(hidden)]
    pub fn set_options_store_for_test(&mut self, o: crate::options::Options) {
        self.options_store = o;
    }
    /// kittest (#304): the avatar-initials fit — (text, font size);
    /// a long configured string must truncate into the 16pt circle.
    #[doc(hidden)]
    pub fn debug_fit_initials_for_test(
        ctx: &egui::Context,
        initials: &str,
    ) -> (String, f32) {
        fit_initials_to_avatar(ctx, initials)
    }
    /// kittest: what every editing command does — dirty + version bump
    /// (the Info thumbnail's cache key, #58).
    #[doc(hidden)]
    pub fn simulate_edit_for_test(&mut self) {
        self.dirty = true;
        self.version += 1;
    }
    /// kittest: force the Info thumbnail render to fail (#58).
    #[doc(hidden)]
    pub fn force_info_render_failure_for_test(&mut self, on: bool) {
        self.info_render_fail_for_test = on;
    }
    /// kittest: the Info thumbnail texture for the active page, if the
    /// Info page has drawn one (#58).
    #[doc(hidden)]
    pub fn info_thumb_tex_id_for_test(&self) -> Option<egui::TextureId> {
        let ed = self.ed.as_ref()?;
        let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
        let w_in = page
            .sheet()
            .and_then(|s| s.cell("PageWidth"))
            .and_then(|c| c.numeric())
            .unwrap_or(8.5)
            .max(0.1);
        let key = format!("info-{}", page.part.replace('/', "_"));
        self.tex.id_for(&key, self.version, (192.0 / w_in) as f32)
    }
    #[doc(hidden)]
    pub fn arm_close_chain_for_test(&mut self) {
        self.close_after_save = Some(CloseTarget::App);
    }
    #[doc(hidden)]
    pub fn preset_save_as_for_test(&mut self, answer: Option<PathBuf>) {
        self.dialog_preset_for_test = Some(answer);
    }
    /// kittest (#71): the next native dialog (any kind — e.g. the
    /// Hyperlinks Address browse) resolves with `answer` instead of
    /// opening rfd.
    #[doc(hidden)]
    pub fn preset_file_dialog_for_test(&mut self, answer: Option<PathBuf>) {
        self.dialog_preset_for_test = Some(answer);
    }
    /// kittest (#71): pretend the document was saved at `path` — the
    /// relative-hyperlink logic keys off the document's directory.
    #[doc(hidden)]
    pub fn debug_set_doc_path_for_test(&mut self, path: PathBuf) {
        self.path = path;
    }
    /// kittest (#35): the Export pane's armed output path.
    #[doc(hidden)]
    pub fn debug_export_pane_path(&self) -> &str {
        self.export_pane.path()
    }
    /// kittest (#35): the pane's last note (status of the worker).
    #[doc(hidden)]
    pub fn debug_export_pane_note(&self) -> Option<&str> {
        self.export_pane.note_for_test()
    }
    #[doc(hidden)]
    pub fn debug_close_prompt(&self) -> bool {
        self.confirm_close.is_some()
    }
    /// kittest (#166): the active ribbon tab — clicking a tab behind
    /// an open MODAL must not change it.
    #[doc(hidden)]
    pub fn debug_active_ribbon_tab(&self) -> String {
        format!("{:?}", self.tab)
    }
    #[doc(hidden)]
    pub fn debug_close_chain_armed(&self) -> bool {
        self.close_after_save.is_some()
    }
    #[doc(hidden)]
    pub fn debug_doc_open(&self) -> bool {
        self.ed.is_some()
    }
    #[doc(hidden)]
    pub fn debug_dirty(&self) -> bool {
        self.dirty
    }
    #[doc(hidden)]
    pub fn debug_close_vetoes(&self) -> u32 {
        self.debug_close_vetoes
    }
    #[doc(hidden)]
    pub fn debug_close_issued(&self) -> u32 {
        self.debug_close_issued
    }

    /// Test hooks: select a shape / switch ribbon tab.
    #[doc(hidden)]
    pub fn select_shape_for_test(&mut self, page: i64, shape: i64) {
        self.set_selection(Some((page, shape)));
    }
    #[doc(hidden)]
    pub fn set_tab_for_test(&mut self, tab: crate::ribbon::Tab) {
        self.tab = tab;
    }
}
