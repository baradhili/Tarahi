//! File operations: open/save/new, the native file dialogs, and
//! stencil set management (open, toggle, defaults, extraction).

use super::*;

impl App {
    pub(super) fn do_save(&mut self, save_as: bool) {
        if save_as || self.path.as_os_str().is_empty() {
            self.begin_dialog(PendingKind::SaveAs);
        } else {
            let path = self.path.clone();
            self.save_to(&path);
        }
    }

    pub(super) fn do_open(&mut self) {
        self.begin_dialog(PendingKind::Open);
    }

    /// Open a `.vssx` into the Shapes pane (#83): dedupes by source
    /// path (re-opening expands and focuses the existing entry),
    /// reveals the entry, and notes the status. Parse failures surface
    /// as a status note, like other load failures.
    /// The user's My Shapes folder (Visio convention, owner direction
    /// 2026-10-27): `<home>/Documents/My Shapes`. PURE — no directory
    /// is created here (kittest reaches this through the built-in
    /// menu, #104); the extraction pass and the Open Existing Stencil
    /// dialog create it on demand. None when no home directory is
    /// known (neither HOME nor USERPROFILE).
    pub(super) fn my_shapes_dir() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)?;
        Some(my_shapes_path(Some(home)))
    }

    pub(super) fn open_stencil(&mut self, path: PathBuf) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::strs().msg_stencil_fallback());
        match self.stencils.open_file(&path) {
            Ok((ix, existing)) => {
                self.stencil.reveal = Some(StencilId::Open(ix));
                self.status = if existing {
                    crate::i18n::strs().msg_stencil_stem_is_already_open_exp(stem)
                } else {
                    crate::i18n::strs().msg_opened_stencil_stem(stem)
                };
            }
            Err(e) => self.status = crate::i18n::strs().msg_open_stencil_failed_e(format!("{e:#}")),
        }
    }

    /// More Shapes ▸ category ▸ stencil: open a built-in catalog
    /// stencil, or close its entry when it is already open (the menu
    /// checkbox state, #87). Built-ins land in the pane exactly like
    /// file stencils (#83) — placement, thumbnails, search, and close
    /// all key off the open-stencil set.
    pub(super) fn toggle_builtin(&mut self, slug: &'static str) {
        let Some(b) = crate::stencil::builtin::by_slug(slug) else {
            return;
        };
        // The dedupe key matches however the entry was opened: the
        // extracted file (#104) or the `builtin/<slug>` fallback.
        let my_shapes = self.stencil_disk.then(Self::my_shapes_dir).flatten();
        let source = crate::stencil::disk::extracted_path(
            my_shapes
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new("")),
            b.slug,
        );
        let fallback = crate::stencil::builtin::source_path(b.slug);
        if let Some(ix) = self
            .stencils
            .position_for(&source)
            .or_else(|| self.stencils.position_for(&fallback))
        {
            let title = self
                .stencils
                .get(ix)
                .map(|st| st.title.clone())
                .unwrap_or_else(|| b.title.to_string());
            self.stencil.close_entry(&mut self.stencils, ix);
            self.status = crate::i18n::strs().msg_closed_title(title);
            return;
        }
        self.open_builtin_stencil(b);
    }

    /// "More Shapes ▸ My Shapes ▸ <stencil>" (#266): open (or focus)
    /// the file through the user-stencil path — exactly a picker
    /// result: title = file stem, pane identity = the file path — or
    /// close its entry when it is already open (the menu checkbox
    /// state, the built-in categories' semantics). An unreadable
    /// stencil surfaces as a status note, like other load failures.
    pub(super) fn toggle_my_shapes_stencil(&mut self, path: PathBuf) {
        if let Some(ix) = self.stencils.position_for(&path) {
            let title = self
                .stencils
                .get(ix)
                .map(|st| st.title.clone())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            self.stencil.close_entry(&mut self.stencils, ix);
            self.status = crate::i18n::strs().msg_closed_title(title);
            return;
        }
        self.open_stencil(path);
    }

    /// Open one built-in catalog stencil (#87), DISK-FIRST (#104):
    /// the extracted `.vssx` under My Shapes when it is there (the
    /// file identity — the same entry the user gets opening that
    /// file), else the embedded bytes with the `builtin/<slug>`
    /// pseudo-path. Title and origin stay the catalog's, so menu
    /// provenance is kept either way.
    pub(super) fn open_builtin_stencil(&mut self, b: &'static crate::stencil::builtin::BuiltinStencil) {
        // Which variant opens is the pane's session preference (#85
        // follow-up): US by default, the metric twin with "Metric
        // Stencils" ticked.
        let metric = self.stencil.metric_units;
        let my_shapes = self.stencil_disk.then(Self::my_shapes_dir).flatten();
        let opened = crate::stencil::disk::open_from_disk(my_shapes.as_deref(), b, metric)
            .map(|(doc, source)| (doc, source, true))
            .or_else(|| {
                vsdx::parse::parse_bytes(b.bytes_for(metric))
                    .ok()
                    .map(|doc| (doc, crate::stencil::builtin::source_path(b.slug), false))
            });
        match opened {
            Some((doc, source, from_disk)) => {
                let (ix, _) = self.stencils.open_parsed(
                    doc,
                    source,
                    b.title.to_string(),
                    crate::stencil::Origin::Builtin,
                );
                self.stencil.reveal = Some(StencilId::Open(ix));
                self.status = if from_disk {
                    crate::i18n::strs().msg_opened_builtin_myshapes(b.title)
                } else if metric {
                    crate::i18n::strs().msg_opened_builtin_metric(b.title)
                } else {
                    format!("opened {title}", title = b.title)
                };
            }
            None => {
                self.status = crate::i18n::strs().msg_builtin_stencil_failed(b.title);
            }
        }
    }

    /// Apply the configured category list/order to the pane (#105):
    /// the configured subset, or the catalog's manifest order when
    /// unset.
    pub(super) fn apply_stencil_preferences(&mut self) {
        self.stencil.builtin_categories = match self.stencil_prefs.categories.clone() {
            Some(cats) => cats,
            None => crate::stencil::builtin::categories()
                .into_iter()
                .map(String::from)
                .collect(),
        };
    }

    /// Open the default stencil set (#105): built-in slugs through
    /// the disk-first path (#104), plus files named under My Shapes
    /// opened like user stencils. Dedupe keeps a re-run cheap (and
    /// the startup path never toggles anything closed).
    pub(super) fn open_default_stencils(&mut self) {
        let my_shapes = self.stencil_disk.then(Self::my_shapes_dir).flatten();
        for entry in self.stencil_prefs.default_open.clone() {
            if let Some(b) = crate::stencil::builtin::by_slug(&entry) {
                self.open_builtin_stencil(b);
                continue;
            }
            let Some(dir) = &my_shapes else {
                continue;
            };
            // A My Shapes file name (config entries are trusted —
            // the user's own config file).
            let p = dir.join(entry.trim_start_matches(['/', '\\']));
            if p.is_file() {
                if let Err(e) = self.stencils.open_file(&p) {
                    self.status = crate::i18n::strs()
                        .msg_default_stencil_failed(format!("{entry:?}"), format!("{e:#}"));
                }
            }
        }
        // Startup opens land at the pane top; no reveal-scroll.
        self.stencil.reveal = None;
    }

    /// kittest (#105): run the startup preference application against
    /// the current (ephemeral-by-default) prefs.
    #[doc(hidden)]
    pub fn apply_stencil_defaults_for_test(&mut self) {
        self.apply_stencil_preferences();
        self.open_default_stencils();
    }

    /// kittest (#105): swap the prefs (e.g. a custom default set /
    /// category order) before applying.
    #[doc(hidden)]
    pub fn set_stencil_prefs_for_test(&mut self, prefs: crate::stencil::prefs::StencilPrefs) {
        self.stencil_prefs = prefs;
    }

    /// kittest (#105/#104): the open stencils' titles, in pane order.
    #[doc(hidden)]
    pub fn debug_open_stencil_titles(&self) -> Vec<String> {
        self.stencils
            .iter()
            .map(|(_, st)| st.title.clone())
            .collect()
    }

    /// kittest (#105): the category list More Shapes shows, in order.
    #[doc(hidden)]
    pub fn debug_builtin_categories(&self) -> Vec<String> {
        self.stencil.builtin_categories.clone()
    }

    /// File → New: replace the current document with a blank drawing,
    /// prompting first if it has unsaved changes.
    pub(super) fn do_new(&mut self) {
        if self.dirty && self.ed.is_some() {
            self.confirm_new = true;
        } else {
            self.start_new();
        }
    }

    pub(super) fn start_new(&mut self) {
        self.confirm_new = false;
        self.new_after_save = false;
        self.close_after_save = None;
        self.ed = Some(Editor::new_blank());
        self.reload_snap_settings();
        self.path = PathBuf::new();
        self.dirty = false;
        self.version += 1;
        self.view = View::default();
        self.page_idx = 0;
        self.set_selection(None);
        self.group_edit.clear();
        self.stencil = Stencil::default();
        // The reset pane drops the configured category order (#105,
        // CodeRabbit): re-apply the stencil preferences right after.
        self.apply_stencil_preferences();
        self.tex.clear();
        self.layers.clear();
        self.page_states.clear();
        self.tab_menu = None;
        self.tab_rename = None;
        self.reorder_open = false;
        self.comment_target = None;
        self.comment_edit = None;
        self.status = crate::i18n::strs().msg_new_blank_drawing_unsaved();
    }

    pub(super) fn begin_dialog(&mut self, kind: PendingKind) {
        if self.dialog.is_some() {
            return; // one native dialog at a time
        }
        let default_name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "drawing.vsdx".into());
        // The Export pane's suggested file (drawing.svg / .png / .pdf,
        // #35) — the save dialog opens on it.
        // Path::file_name handles both separators (#218 review) — a
        // backslash path would survive rsplit('/') whole.
        let default_export_name = std::path::Path::new(self.export_pane.path())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "drawing.svg".to_string());
        let (tx, rx) = mpsc::channel();
        // The native dialog is untestable headless (rfd opens GTK);
        // an armed preset answers in its place (tests only).
        let preset = self.dialog_preset_for_test.take();
        std::thread::spawn(move || {
            let picked = match preset {
                Some(p) => p,
                None => match kind {
                    PendingKind::Open => rfd::FileDialog::new()
                        .add_filter(
                            crate::i18n::strs().msg_visio_vsdx_family(),
                            &["vsdx", "vstx", "vssx"],
                        )
                        .pick_file(),
                    PendingKind::OpenStencil => {
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter(crate::i18n::strs().msg_visio_stencil(), &["vssx"]);
                        if let Some(dir) = Self::my_shapes_dir() {
                            let _ = std::fs::create_dir_all(&dir);
                            dlg = dlg.set_directory(dir);
                        }
                        dlg.pick_file()
                    }
                    PendingKind::SaveAs => rfd::FileDialog::new()
                        .add_filter(
                            crate::i18n::strs().msg_visio_drawing(),
                            &["vsdx", "vstx", "vssx"],
                        )
                        .set_file_name(&default_name)
                        .save_file(),
                    PendingKind::OpenPicture => rfd::FileDialog::new()
                        .add_filter(crate::i18n::strs().msg_pictures(), &["png", "jpg", "jpeg"])
                        .pick_file(),
                    // Any file can be a hyperlink target (#71).
                    PendingKind::HyperlinkAddress => rfd::FileDialog::new().pick_file(),
                    PendingKind::ExportSave(format) => {
                        // bound so the catalog Strings outlive the call
                        let label = match format {
                            dc::export::Format::Svg => crate::i18n::strs().msg_filter_svg(),
                            dc::export::Format::Png => crate::i18n::strs().msg_filter_png(),
                            dc::export::Format::Pdf => crate::i18n::strs().msg_filter_pdf(),
                        };
                        rfd::FileDialog::new().add_filter(label.as_str(), format.extensions())
                    }
                    .set_file_name(&default_export_name)
                    .save_file(),
                },
            };
            let _ = tx.send(picked);
        });
        self.dialog = Some(match kind {
            PendingKind::Open => PendingDialog::Open(rx),
            PendingKind::OpenStencil => PendingDialog::OpenStencil(rx),
            PendingKind::SaveAs => PendingDialog::SaveAs(rx),
            PendingKind::OpenPicture => PendingDialog::OpenPicture(rx),
            PendingKind::HyperlinkAddress => PendingDialog::HyperlinkAddress(rx),
            PendingKind::ExportSave(_) => PendingDialog::ExportSave(rx),
        });
    }

    /// Poll the background stencil extraction (#104): progress lands
    /// in the status bar — the one piece of UI every shell state
    /// shows — with a repaint heartbeat so it advances without user
    /// input, and a completion note when files were actually written.
    pub(super) fn poll_stencil_extraction(&mut self, ctx: &Context) {
        let Some(h) = self.stencil_extract.take() else {
            return;
        };
        // Drain everything ready this frame (the writer is fast).
        match h.poll() {
            Some(crate::stencil::disk::ExtractionMsg::Progress { done, total }) => {
                self.status = crate::i18n::strs().msg_extracting_built_in_stencils_to(done, total);
                self.stencil_extract = Some(h);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            Some(crate::stencil::disk::ExtractionMsg::Done(e)) => {
                if e.failed > 0 {
                    self.status =
                        crate::i18n::strs().msg_builtin_extracted_summary(e.written, e.failed);
                } else if e.written > 0 {
                    self.status = crate::i18n::strs().msg_builtin_extracted_to(e.written);
                }
                // Nothing written (files already current): no noise.
            }
            None => {
                // Nothing ready yet — keep waiting with a heartbeat.
                self.stencil_extract = Some(h);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    /// Consume a finished dialog result; keep a slow repaint heartbeat
    /// while one is still open so the UI loop (and the window's event
    /// processing) never stalls behind it.
    pub(super) fn poll_dialog(&mut self, ctx: &Context) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let (path, again) = match &dialog {
            PendingDialog::Open(rx)
            | PendingDialog::SaveAs(rx)
            | PendingDialog::OpenStencil(rx)
            | PendingDialog::OpenPicture(rx)
            | PendingDialog::ExportSave(rx)
            | PendingDialog::HyperlinkAddress(rx) => match rx.try_recv() {
                Ok(picked) => (picked, false),
                Err(mpsc::TryRecvError::Empty) => (None, true),
                Err(mpsc::TryRecvError::Disconnected) => (None, false),
            },
        };
        if again {
            self.dialog = Some(dialog);
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        let Some(path) = path else {
            // cancelled: drop any chained follow-up
            self.new_after_save = false;
            self.close_after_save = None;
            return;
        };
        match dialog {
            // #224/#242: the picked-file parse runs on the worker via
            // load() — the scrim's minimum display keeps it visible;
            // a stencil still lands in the Shapes pane synchronously
            // (#83, the routing inside load()).
            PendingDialog::Open(_) => self.load(path),
            PendingDialog::OpenStencil(_) => self.open_stencil(path),
            PendingDialog::OpenPicture(_) => self.insert_picture_file(&path),
            // The picked export target lands in the pane's path field
            // (#35); a typed/picked extension re-targets the format.
            PendingDialog::ExportSave(_) => self.export_pane.set_path(path.clone()),
            // The picked target lands in the open draft's Address
            // field (#71), stored the way the document's toggle wants
            // it (a path inside the document's directory shows as its
            // relative form when the toggle is on — what 2016 shows).
            PendingDialog::HyperlinkAddress(_) => {
                let relative = self.relative_paths.get(&self.path);
                let stored = crate::hyperlink::store_address(
                    &path.to_string_lossy(),
                    crate::hyperlink::doc_dir(&self.path),
                    relative,
                );
                if let Some(d) = self.hyperlink_draft.as_mut() {
                    d.address = stored;
                }
            }
            PendingDialog::SaveAs(_) => {
                self.save_to(&path);
                if self.new_after_save {
                    if self.dirty {
                        // save failed — keep the document open, drop the chain
                        self.new_after_save = false;
                    } else {
                        self.start_new();
                    }
                } else if let Some(target) = self.close_after_save.take() {
                    if !self.dirty {
                        // save succeeded — the close continues (GitHub #65)
                        self.finish_close(ctx, target);
                    }
                    // save failed — keep everything open
                }
            }
        }
    }

    pub(super) fn save_to(&mut self, path: &Path) {
        let Some(ed) = self.ed() else { return };
        match ed.save_as(path) {
            Ok(()) => {
                self.status = crate::i18n::strs().msg_saved_path(path.display().to_string());
                self.dirty = false;
                self.path = path.to_path_buf();
                self.recent.record(path);
            }
            Err(e) => self.status = crate::i18n::strs().msg_save_failed_e(format!("{e:#}")),
        }
    }

    // -- hit testing ------------------------------------------------------
}
