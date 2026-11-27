//! The backstage view (File) with its recent list, the options
//! dialog, and the close/new-window flows.

use super::*;

impl App {
    /// The Recent list shared by the start screen and the backstage
    /// Open page: one row per MRU entry, click to reopen. Files that no
    /// longer exist stay listed but ghosted (Visio-style) rather than
    /// being silently dropped. Returns the clicked path, if any.
    pub(super) fn recent_list_ui(&self, ui: &mut Ui) -> Option<PathBuf> {
        if self.recent.entries().is_empty() {
            ui.weak(crate::i18n::strs().msg_files_you_open_will_appear_here());
            return None;
        }
        let mut clicked = None;
        for p in self.recent.entries() {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            let missing = !p.exists();
            // Theme-aware: the start screen's central area follows the
            // egui visuals (dark by default), so live entries take the
            // normal text color and ghosts the weak one — the same
            // pairing the empty-list placeholder uses.
            let txt = if missing {
                egui::RichText::new(name).color(ui.visuals().weak_text_color())
            } else {
                egui::RichText::new(name)
            };
            if ui
                .button(txt)
                .on_hover_text(p.display().to_string())
                .clicked()
            {
                clicked = Some(p.clone());
            }
        }
        clicked
    }
}

impl App {
    /// #268: open the Options dialog from the backstage rail — the
    /// draft starts at the live store values plus the current language
    /// choice (match-system while no explicit language is persisted).
    pub(super) fn open_options_dialog(&mut self) {
        let lang = if crate::i18n::is_system_match() {
            crate::options::LangChoice::System
        } else {
            crate::options::LangChoice::Concrete(crate::i18n::current())
        };
        self.options_open = true;
        self.options_opened_this_frame = true;
        self.options_draft = Some(crate::options::Draft::capture(&self.options_store, lang));
    }

    /// #268: the Options dialog over the backstage. Rendered inside
    /// the backstage branch (the backstage early-returns); the modal
    /// manages its own Esc, so nothing here touches the key handling.
    pub(super) fn options_window(&mut self, ctx: &egui::Context) {
        if !self.options_open {
            return;
        }
        let freshly_opened = std::mem::take(&mut self.options_opened_this_frame);
        // #304: a confirm modal ABOVE the dialog (the close/new
        // confirmation) owns Escape while it is up.
        let esc_blocked = self.confirm_close.is_some() || self.confirm_new;
        let outcome = match self.options_draft.as_mut() {
            Some(draft) => {
                draft.lang_persist_failed = self.lang_write_failed;
                draft.settings_persist_failed = self.settings_write_failed;
                crate::options::window(ctx, draft, freshly_opened, esc_blocked)
            }
            None => crate::options::Outcome::Cancel,
        };
        match outcome {
            crate::options::Outcome::None => {}
            crate::options::Outcome::Ok => {
                self.commit_options(ctx);
                // PR #305 review: a FAILED save (language OR
                // options.json) keeps the dialog open — closing here
                // would hide the in-dialog warning (the File screen
                // behind it paints no status bar) and the user would
                // believe the choice saved until a restart restored
                // the old settings.
                if !self.lang_write_failed && !self.settings_write_failed {
                    self.options_open = false;
                    self.options_draft = None;
                }
            }
            crate::options::Outcome::Apply => self.commit_options(ctx),
            crate::options::Outcome::Cancel => {
                self.options_open = false;
                self.options_draft = None;
            }
        }
    }

    /// #268: commit the draft — the store fields (persisted when the
    /// store is not ephemeral) and the Language page's choice (the
    /// Options dialog is the ONLY language surface since #304 retired
    /// the View ▸ Window dialog; the headless pinning rule picks the
    /// ephemeral twins when the store is in-memory). The #300 theme
    /// choice applies to the live context in the same commit.
    pub(super) fn commit_options(&mut self, ctx: &egui::Context) {
        let Some(d) = self.options_draft.as_ref() else {
            return;
        };
        let lang = d.lang;
        let theme_change = d.theme != self.options_store.theme;
        let lang_change = match lang {
            crate::options::LangChoice::System => !crate::i18n::is_system_match(),
            crate::options::LangChoice::Concrete(l) => {
                crate::i18n::current() != l || crate::i18n::is_system_match()
            }
        };
        // #305 review: the SETTINGS write result rides up too — a
        // failed options.json save warns in the dialog and retries on
        // the next Apply/OK (the in-memory apply always happened).
        match self.options_store.apply(d) {
            Ok(()) => self.settings_write_failed = false,
            Err(e) => {
                self.settings_write_failed = true;
                self.status = format!(
                    "{} ({e})",
                    crate::i18n::strs().msg_options_settings_save_failed()
                );
            }
        }
        // Sync the committed values back into the draft (a clamped
        // interval or trimmed name corrects visibly on the next paint).
        if let Some(d) = self.options_draft.as_mut() {
            d.sync_committed(&self.options_store);
        }
        // #300: a changed theme choice flips the shell on the very
        // next frame — the same single `set_theme` the OS watcher
        // makes, and Follow-OS re-arms by re-detecting NOW (the
        // watcher only reports changes, so an OS flip that happened
        // while a manual choice was active must not wait for the next
        // one). An unchanged choice applies nothing, so the Follow-OS
        // default never probes on a commit.
        if theme_change {
            self.apply_theme_choice(ctx);
        }
        // #304: retry a FAILED persistence even when the in-memory
        // language already matches the draft — swallowing the write
        // error stranded the session/file choice with no path back.
        if lang_change || self.lang_write_failed {
            let persisted = match lang {
                crate::options::LangChoice::System => {
                    if self.options_store.persists() {
                        crate::i18n::set_system_default()
                    } else {
                        crate::i18n::set_system_default_ephemeral();
                        Ok(())
                    }
                }
                crate::options::LangChoice::Concrete(l) => {
                    if self.options_store.persists() {
                        crate::i18n::set_language(l)
                    } else {
                        crate::i18n::set_language_ephemeral(l);
                        Ok(())
                    }
                }
            };
            match persisted {
                Ok(()) => {
                    self.lang_write_failed = false;
                    self.status = crate::i18n::strs().msg_applies_to_the_ribbon_menus_and();
                }
                Err(e) => {
                    self.lang_write_failed = true;
                    self.status = format!(
                        "{} ({e})",
                        crate::i18n::strs().msg_options_language_save_failed()
                    );
                }
            }
        }
    }

    /// #300: apply the committed theme choice to the live context.
    /// Forced Light/Dark is the same single `set_theme` the OS watcher
    /// makes (both style slots are installed, so selecting restyles
    /// every shell surface next frame); Follow-OS re-detects — on
    /// Linux that is the concrete gsettings/portal probe, elsewhere
    /// `System` hands tracking back to eframe, both exactly the #52
    /// startup behavior.
    pub(super) fn apply_theme_choice(&self, ctx: &egui::Context) {
        match self.options_store.theme {
            crate::options::ThemeChoice::FollowOs => {
                ctx.set_theme(crate::os_theme::detect())
            }
            crate::options::ThemeChoice::Light => {
                ctx.set_theme(egui::ThemePreference::Light)
            }
            crate::options::ThemeChoice::Dark => {
                ctx.set_theme(egui::ThemePreference::Dark)
            }
        }
    }

    /// A close request arrived — the window's ✕ / Alt-F4 (or the
    /// kittest hook): veto it and ask the user first when there is
    /// anything to lose; a clean document closes straight away
    /// (GitHub #65). egui 0.37 semantics: the app exits after this
    /// frame unless `ViewportCommand::CancelClose` is sent.
    pub(super) fn handle_close_request(&mut self, ctx: &Context) {
        let requested = self.close_req_for_test || ctx.input(|i| i.viewport().close_requested());
        self.close_req_for_test = false;
        if !requested {
            return;
        }
        if self.closing {
            // Our own approved close echoing back (egui-winit feeds
            // ViewportCommand::Close into the next frame's input):
            // send nothing — the un-vetoed request is what exits the
            // app — and never resurrect the prompt.
            return;
        }
        // While the prompt is already up — or its Save has the native
        // save-as dialog open (`close_after_save` armed) — absorb
        // further requests: a second prompt's Cancel would contradict
        // the pending chain, and its Don't Save would race the
        // dialog's save (one prompt; the open dialog still decides).
        if self.confirm_close.is_some() || self.close_after_save.is_some() {
            self.veto_close(ctx);
            return;
        }
        if self.dirty && self.ed.is_some() {
            // Leaving supersedes the New prompt if it was up.
            self.confirm_new = false;
            self.veto_close(ctx);
            self.confirm_close = Some(CloseTarget::App);
        } else {
            // Clean: let the close proceed. Re-issuing Close also
            // covers the test hook, where no OS close stands behind
            // the flag (in production it is an idempotent no-op).
            self.issue_close(ctx);
        }
    }

    /// The "Save changes?" modal before a window close or File → Close
    /// discards a dirty document (GitHub #65). Visio 2016 shape:
    /// Save (save-as first when untitled, then close), Don't Save,
    /// Cancel.
    pub(super) fn confirm_close_window(&mut self, ctx: &Context) {
        let Some(target) = self.confirm_close else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_close = None;
            return;
        }
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::strs().msg_the_drawing());
        // MODAL (#166): the backdrop blocks input to everything
        // behind the prompt — the document cannot change underneath
        // the Save / Don't-Save decision. The title rides inside the
        // dialog frame (a modal has no title bar).
        egui::Modal::new(egui::Id::unique("dc-confirm-close")).show(ctx, |ui| {
            // #257 item 3: the modal mirrors with the chrome.
            ui.with_layout(dialog_dir(), |ui| {
                ui.strong(crate::i18n::strs().msg_save_changes());
                ui.add_space(4.0);
                ui.label(crate::i18n::strs().msg_save_changes_to_name(name));
                row(ui, |ui| {
                    if ui.button(crate::i18n::strs().msg_save()).clicked() {
                        self.confirm_close = None;
                        // New and close chains are mutually exclusive.
                        self.new_after_save = false;
                        if self.path.as_os_str().is_empty() {
                            // pick a file first; the close continues
                            // after the dialog completes
                            self.close_after_save = Some(target);
                            self.begin_dialog(PendingKind::SaveAs);
                        } else {
                            let p = self.path.clone();
                            self.save_to(&p);
                            if !self.dirty {
                                self.finish_close(ctx, target);
                            }
                            // save failed — stay open; the status bar
                            // carries the error
                        }
                    }
                    if ui.button(crate::i18n::strs().msg_don_t_save()).clicked() {
                        self.confirm_close = None;
                        self.finish_close(ctx, target);
                    }
                    if ui.button(crate::i18n::strs().msg_cancel()).clicked() {
                        self.confirm_close = None;
                    }
                });
            });
        });
    }

    /// Execute a confirmed close: the app window (re-issue the close
    /// the veto cancelled) or just the document (back to the start
    /// screen — the app stays open, Visio's File → Close).
    pub(super) fn finish_close(&mut self, ctx: &Context, target: CloseTarget) {
        self.close_after_save = None;
        match target {
            CloseTarget::App => self.issue_close(ctx),
            CloseTarget::Doc => self.close_doc(),
        }
    }

    /// Veto an in-flight OS close so the prompt can run.
    pub(super) fn veto_close(&mut self, ctx: &Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.debug_close_vetoes += 1;
    }

    /// Ask the viewport to close (eframe exits the app for the root).
    /// Marked so the close's own echo — egui-winit delivers
    /// `ViewportCommand::Close` as next frame's close-requested
    /// input — passes through instead of being re-prompted (#65).
    pub(super) fn issue_close(&mut self, ctx: &Context) {
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        self.debug_close_issued += 1;
    }

    /// File → Close teardown: back to the start screen, nothing dirty.
    /// #192 4a: move the active document into the inactive set
    /// (capture its session fields; the per-doc UI state resets on
    /// the way back — view/selection state is slice 4b).
    pub(super) fn stash_active_doc(&mut self) {
        if self.ed.is_none() {
            return;
        }
        let ed = self.ed.take().unwrap();
        self.docs.push(DocSession {
            ed,
            path: std::mem::take(&mut self.path),
            dirty: std::mem::replace(&mut self.dirty, false),
            shapesheets: std::mem::take(&mut self.shapesheets),
            view: std::mem::take(&mut self.view),
            page_idx: self.page_idx,
            sel: self.sel,
            sel_set: std::mem::take(&mut self.sel_set),
            group_edit: std::mem::take(&mut self.group_edit),
        });
    }

    /// #192 4a: activate an inactive document — the active one swaps
    /// into its slot (nothing is lost).
    pub(super) fn activate_doc(&mut self, ix: usize) {
        if ix >= self.docs.len() {
            return;
        }
        let mut incoming = self.docs.swap_remove(ix);
        // Commit the live text edit against the OUTGOING document —
        // a surviving session would write its draft into the incoming
        // one (CodeRabbit PR review).
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        self.stash_active_doc();
        self.ed = Some(incoming.ed);
        self.path = incoming.path;
        self.dirty = incoming.dirty;
        self.shapesheets = std::mem::take(&mut incoming.shapesheets);
        self.view = incoming.view;
        // A stored fit-MODE view re-fits to the (possibly changed)
        // client; a manual zoom/pan view restores exactly.
        if self.view.fit_mode {
            self.view.fit_pending = true;
        }
        self.page_idx = incoming.page_idx;
        self.sel = incoming.sel;
        self.sel_set = incoming.sel_set;
        self.group_edit = incoming.group_edit;
        // The Insert Field dialog does not carry across documents
        // (#267, CodeRabbit round 2): it is bound to a shape ID, and
        // IDs collide across documents as well as pages. The
        // retained last-edit caret is ID-scoped the same way
        // (CodeRabbit/Greptile round 5).
        self.fields_open = None;
        self.fields_caret = None;
        self.fields_draft = None;
        self.last_edit_caret = None;
        // #121: the spelling pass does not carry across documents
        // (same-ID collision rule).
        self.spell.session = None;
        self.tex.clear();
        self.layers.clear();
        self.page_states.clear();
        // The jump overlay belongs to the outgoing document: a switch
        // keeps `version`, so the key alone would serve its hops (#389).
        self.jump_fx_cache = None;
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
        self.status = crate::i18n::strs().msg_switched_to_name(name);
    }

    pub(super) fn close_doc(&mut self) {
        // Commit the live text edit against the document being closed
        // (or the one promoting to active below) — same ownership
        // rule as activate_doc (CodeRabbit PR review).
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        self.ed = None;
        self.path = PathBuf::new();
        self.dirty = false;
        // #192 spec: closing the document closes its ShapeSheet(s).
        self.shapesheets.clear();
        self.set_selection(None);
        self.stencil = Stencil::default();
        // The reset pane drops the configured category order (#105,
        // CodeRabbit): re-apply the stencil preferences right after.
        self.apply_stencil_preferences();
        self.tex.clear();
        self.layers.clear();
        self.page_states.clear();
        // The jump overlay belongs to the outgoing document: a switch
        // keeps `version`, so the key alone would serve its hops (#389).
        self.jump_fx_cache = None;
        self.tab_menu = None;
        self.tab_rename = None;
        self.reorder_open = false;
        self.comment_target = None;
        self.comment_edit = None;
        // The Insert Field dialog does not survive a close/promote
        // (#267, CodeRabbit round 4) — and neither does the retained
        // last-edit caret: the promoting document's same-ID shape
        // must not inherit it (CodeRabbit/Greptile round 5).
        self.fields_open = None;
        self.fields_caret = None;
        self.fields_draft = None;
        self.last_edit_caret = None;
        // #121: the spelling pass does not survive a close/promote
        // (same-ID collision rule).
        self.spell.session = None;
        // #192 4a: another open document promotes to active.
        if !self.docs.is_empty() {
            let next = self.docs.remove(0);
            self.ed = Some(next.ed);
            self.path = next.path;
            self.dirty = next.dirty;
            self.shapesheets = next.shapesheets;
            let name = self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
            self.status = crate::i18n::strs().msg_closed_switched_to_name(name);
        } else {
            self.status = crate::i18n::strs().msg_document_closed();
        }
    }

    /// Unsaved-changes prompt before File → New discards work.
    pub(super) fn confirm_new_window(&mut self, ctx: &Context) {
        if !self.confirm_new {
            return;
        }
        // MODAL (#166), same as the close prompt: nothing behind the
        // dialog is interactive while the decision is pending.
        egui::Modal::new(egui::Id::unique("dc-confirm-new")).show(ctx, |ui| {
            // #257 item 3: the modal mirrors with the chrome.
            ui.with_layout(dialog_dir(), |ui| {
                ui.strong(crate::i18n::strs().msg_save_changes());
                ui.add_space(4.0);
                ui.label(crate::i18n::strs().msg_the_current_drawing_has_unsaved());
                row(ui, |ui| {
                    if ui.button(crate::i18n::strs().msg_save_new()).clicked() {
                        self.confirm_new = false;
                        if self.path.as_os_str().is_empty() {
                            // pick a file first; the new document
                            // starts after the dialog completes
                            self.new_after_save = true;
                            self.begin_dialog(PendingKind::SaveAs);
                        } else {
                            let p = self.path.clone();
                            self.save_to(&p);
                            self.start_new();
                        }
                    }
                    if ui.button(crate::i18n::strs().msg_discard_new()).clicked() {
                        self.start_new();
                    }
                    if ui.button(crate::i18n::strs().msg_cancel()).clicked() {
                        self.confirm_new = false;
                    }
                });
            });
        });
    }

    /// The File backstage (Visio 2016): green command rail on the left,
    /// page content on the right. Replaces the whole shell while open.
    /// The File backstage (GitHub #55): the 2010 three-band rail —
    /// commands (Save/Save As/Close) above the destinations (Info,
    /// Recent, New, Print, Share, Export), Options pinned below — with
    /// 2016's functions folded in (Share/Export split, Close-document,
    /// search-first New). Every color rides theme tokens; Esc and the
    /// back control return to the document.
    pub(super) fn backstage_view(&mut self, ui: &mut Ui) {
        let mut close_backstage = false;
        let mut action: Option<BackAction> = None;
        // Esc always returns to the document — unless a prompt owns
        // the screen, or the Options dialog (#268: it manages its own
        // Esc and must not close the backstage underneath it).
        if !self.confirm_new
            && self.confirm_close.is_none()
            && !self.options_open
            && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
        {
            close_backstage = true;
        }
        let has_doc = self.ed.is_some();
        let t = theme::shell_tokens(ui.visuals());
        if crate::i18n::rtl() {
            egui::Panel::right("backstage_rail")
        } else {
            egui::Panel::left("backstage_rail")
        }
        .frame(
            egui::Frame::new()
                .fill(t.accent_band)
                .inner_margin(egui::Margin::same(10)),
        )
        .default_size(190.0)
        .resizable(false)
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            ui.vertical(|ui| {
                let back = egui::Button::new(
                    egui::RichText::new("<  Back")
                        .color(Color32::WHITE)
                        .strong(),
                )
                .frame(false);
                if ui.add(back).clicked() {
                    close_backstage = true;
                }
                ui.add_space(10.0);
                // One row builder for both bands: a command runs an
                // action immediately; a destination switches pages.
                let mut rail = |ui: &mut Ui,
                                label: &str,
                                enabled: bool,
                                page: Option<BackPage>,
                                act: Option<BackAction>| {
                    let selected = page.is_some() && page == Some(self.backstage_page);
                    let txt = egui::RichText::new(label).size(14.0).color(if enabled {
                        Color32::WHITE
                    } else {
                        t.rail_disabled
                    });
                    let mut b = egui::Button::new(txt).frame(false);
                    if selected {
                        b = b.fill(t.rail_selected);
                    }
                    let resp = ui.add_enabled(enabled, b);
                    if resp.clicked() {
                        if let Some(p) = page {
                            self.backstage_page = p;
                        }
                        action = act;
                    }
                    resp
                };
                // Band 1 — the command cluster above the destinations
                // (2010's separation of actions from navigation, the
                // structure Apple's and Material's rail guidance ask
                // for).
                rail(
                    ui,
                    &crate::i18n::strs().msg_save(),
                    has_doc,
                    None,
                    Some(BackAction::Save),
                );
                rail(
                    ui,
                    &crate::i18n::strs().msg_save_as(),
                    has_doc,
                    None,
                    Some(BackAction::SaveAs),
                );
                rail(
                    ui,
                    &crate::i18n::strs().msg_close(),
                    has_doc,
                    None,
                    Some(BackAction::CloseDoc),
                );
                // Band divider.
                let (sep, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().hline(
                    sep.left()..=sep.right(),
                    sep.center().y,
                    Stroke::new(1.0, Color32::from_white_alpha(60)),
                );
                ui.add_space(4.0);
                // Band 2 — destinations (six, inside M3's 3–7 window;
                // 2016's Share/Export split replaces 2010's
                // "Save & Send").
                rail(
                    ui,
                    &crate::i18n::strs().msg_info(),
                    true,
                    Some(BackPage::Info),
                    None,
                );
                rail(
                    ui,
                    &crate::i18n::strs().msg_recent(),
                    true,
                    Some(BackPage::Recent),
                    None,
                );
                rail(
                    ui,
                    &crate::i18n::strs().msg_rail_new(),
                    true,
                    Some(BackPage::New),
                    None,
                );
                rail(
                    ui,
                    &crate::i18n::strs().msg_rail_print(),
                    true,
                    Some(BackPage::Print),
                    None,
                )
                .on_hover_text(crate::i18n::strs().msg_print_the_current_drawing());
                rail(
                    ui,
                    &crate::i18n::strs().msg_share(),
                    true,
                    Some(BackPage::Share),
                    None,
                )
                .on_hover_text(crate::i18n::strs().msg_share_a_copy_of_the_drawing());
                rail(
                    ui,
                    &crate::i18n::strs().msg_rail_export(),
                    true,
                    Some(BackPage::Export),
                    None,
                )
                .on_hover_text(crate::i18n::strs().msg_export_entry_desc());
            });
            // Band 3 — pinned footer, non-critical items only (Apple:
            // "avoid putting critical information or actions at the
            // bottom of a sidebar").
            let rail_footer_dir = if crate::i18n::rtl() {
                egui::Layout::bottom_up(egui::Align::RIGHT)
            } else {
                egui::Layout::bottom_up(egui::Align::LEFT)
            };
            ui.with_layout(rail_footer_dir, |ui| {
                // #268: the rail's Options entry is live — the click
                // opens the Options dialog over the File screen
                // (Visio's pinned footer spot).
                let resp = ui.add(
                    egui::Button::new(
                        egui::RichText::new(crate::i18n::strs().msg_options())
                            .size(14.0)
                            .color(Color32::WHITE),
                    )
                    .frame(false),
                );
                if resp.clicked() && !self.options_open {
                    self.open_options_dialog();
                }
                resp.on_hover_text(crate::i18n::strs().msg_options_entry_desc());
            });
        });
        // Page content (the 2010 white backstage surface).
        let mut open_browse = false;
        let mut open_recent: Option<PathBuf> = None;
        let mut new_blank = false;
        let page = self.backstage_page;
        let mut search_text = self.backstage_search.clone();
        egui::Frame::new()
            .fill(t.ribbon_content)
            .inner_margin(egui::Margin::same(28))
            .show(ui, |ui| {
                ui.add_space(12.0);
                match page {
                    BackPage::Info => {
                        if has_doc {
                            let name = self
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| crate::i18n::strs().msg_untitled());
                            let dir = self
                                .path
                                .parent()
                                .map(|p| p.display().to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| crate::i18n::strs().msg_not_saved_yet());
                            let status = if self.dirty {
                                crate::i18n::strs().msg_unsaved_changes()
                            } else {
                                crate::i18n::strs().msg_all_changes_saved()
                            };
                            // Live thumbnail of the active page (#58):
                            // the same render → rasterize → texture pipeline
                            // the stencil pane uses, cached per (page,
                            // version) so any edit refreshes it. The SVG
                            // itself is built lazily by the cache's miss
                            // closure (#90) — an open Info tab no longer
                            // re-serializes the page every frame.
                            let (thumb_w_in, thumb_h_in, thumb_part) = {
                                let ed = self.ed.as_ref().unwrap();
                                let page = &ed.doc.pages
                                    [self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
                                let n = |name: &str| {
                                    page.sheet()
                                        .and_then(|s| s.cell(name))
                                        .and_then(|c| c.numeric())
                                        .unwrap_or(0.0)
                                };
                                (
                                    n("PageWidth").max(0.1),
                                    n("PageHeight").max(0.1),
                                    page.part.clone(),
                                )
                            };
                            ui.heading(&name);
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                egui::Grid::new("backstage_info")
                                    .num_columns(2)
                                    .spacing(Vec2::new(18.0, 6.0))
                                    .show(ui, |ui| {
                                        ui.weak(crate::i18n::strs().msg_name());
                                        ui.label(&name);
                                        ui.end_row();
                                        ui.weak(crate::i18n::strs().msg_location());
                                        ui.label(&dir);
                                        ui.end_row();
                                        ui.weak(crate::i18n::strs().msg_status());
                                        ui.label(status);
                                        ui.end_row();
                                    });
                                ui.add_space(24.0);
                                // Bordered card on the card token; neutral
                                // (no error text) when the render fails.
                                let img_w = 192.0_f64;
                                let aspect = (thumb_h_in / thumb_w_in).clamp(0.2, 3.0);
                                let (r, _) = ui.allocate_exact_size(
                                    Vec2::new(
                                        (img_w + 16.0) as f32,
                                        (img_w * aspect + 16.0) as f32,
                                    ),
                                    Sense::hover(),
                                );
                                ui.painter().rect_filled(r, 3.0, t.card_bg);
                                ui.painter().rect_stroke(
                                    r,
                                    3.0,
                                    Stroke::new(1.0, t.ribbon_group),
                                    egui::StrokeKind::Middle,
                                );
                                if !self.info_render_fail_for_test {
                                    // ~192 px wide, aspect-true: the raster
                                    // derives its height from the page.
                                    let ppi = (img_w / thumb_w_in) as f32;
                                    let key = format!("info-{}", thumb_part.replace('/', "_"));
                                    if let Ok(Some(tex)) = self.tex.get(
                                        ui.ctx(),
                                        &mut self.pool,
                                        thumb_w_in,
                                        &key,
                                        self.version,
                                        ppi,
                                        || {
                                            let ed = self.ed.as_ref().unwrap();
                                            let page = &ed.doc.pages[self
                                                .page_idx
                                                .min(ed.doc.pages.len().saturating_sub(1))];
                                            dc::svg::render_page(&ed.doc, page)
                                        },
                                    ) {
                                        let ir = Rect::from_min_size(
                                            r.min + Vec2::new(8.0, 8.0),
                                            Vec2::new(img_w as f32, (img_w * aspect) as f32),
                                        );
                                        let uv =
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                                        ui.painter().image(tex.id(), ir, uv, Color32::WHITE);
                                    }
                                }
                            });
                            ui.add_space(14.0);
                            ui.horizontal(|ui| {
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(crate::i18n::strs().msg_protect_drawing()),
                                )
                                .on_disabled_hover_text(
                                    crate::i18n::strs().msg_document_protection_arrives_with(),
                                );
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(crate::i18n::strs().msg_inspect_drawing()),
                                )
                                .on_disabled_hover_text(
                                    crate::i18n::strs().msg_document_inspection_arrives_with(),
                                );
                            });
                        } else {
                            ui.heading(crate::i18n::strs().msg_no_document_open());
                            ui.add_space(8.0);
                            if ui.button(crate::i18n::strs().msg_open_file_cta()).clicked() {
                                open_browse = true;
                            }
                        }
                    }
                    BackPage::Recent => {
                        ui.horizontal(|ui| {
                            ui.heading(crate::i18n::strs().msg_recent());
                            // #257: the Browse pin mirrors with the chrome.
                            let recent_dir = if crate::i18n::rtl() {
                                egui::Layout::left_to_right(egui::Align::TOP)
                            } else {
                                egui::Layout::right_to_left(egui::Align::TOP)
                            };
                            ui.with_layout(recent_dir, |ui| {
                                if ui.button(crate::i18n::strs().msg_browse()).clicked() {
                                    open_browse = true;
                                }
                            });
                        });
                        ui.add_space(8.0);
                        if self.recent.entries().is_empty() {
                            // elementary Welcome Screen: context plus
                            // named actions instead of an empty list.
                            ui.weak(crate::i18n::strs().msg_drawings_you_open_will_appear_he());
                            ui.add_space(12.0);
                            if welcome_action(
                                ui,
                                Icon::New,
                                &crate::i18n::strs().msg_new_blank_drawing(),
                                &crate::i18n::strs().msg_start_an_empty_drawing_on_a_lett(),
                            ) {
                                new_blank = true;
                            }
                            if welcome_action(
                                ui,
                                Icon::Open,
                                &crate::i18n::strs().msg_open_file_title(),
                                &crate::i18n::strs().msg_browse_for_an_existing_visio_pac(),
                            ) {
                                open_browse = true;
                            }
                        } else if let Some(p) = self.recent_list_ui(ui) {
                            open_recent = Some(p);
                        }
                    }
                    BackPage::New => {
                        ui.horizontal(|ui| {
                            ui.heading(crate::i18n::strs().msg_rail_new());
                            // 2016's search-first New page; elementary/
                            // Apple: trailing toolbar position, instant
                            // type-to-filter.
                            // #257: the search pin mirrors with the chrome.
                            let new_search_dir = if crate::i18n::rtl() {
                                egui::Layout::left_to_right(egui::Align::TOP)
                            } else {
                                egui::Layout::right_to_left(egui::Align::TOP)
                            };
                            ui.with_layout(new_search_dir, |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut search_text)
                                        .hint_text(crate::i18n::strs().msg_search_templates())
                                        .desired_width(200.0),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        ui.weak(crate::i18n::strs().msg_suggested_flowchart_network_floo());
                        ui.add_space(12.0);
                        let q = search_text.trim().to_lowercase();
                        let mut card_click = false;
                        ui.horizontal_wrapped(|ui| {
                            if backstage_card(
                                ui,
                                Icon::New,
                                &crate::i18n::strs().msg_blank_drawing(),
                                q.is_empty(),
                            ) {
                                card_click = true;
                            }
                            for (name, icon) in new_template_entries() {
                                if !q.is_empty() && !name.to_lowercase().contains(&q) {
                                    continue;
                                }
                                backstage_card(ui, icon, &name, false);
                            }
                        });
                        if card_click {
                            new_blank = true;
                        }
                        if !q.is_empty()
                            && !"blank drawing".contains(&q)
                            && new_template_entries()
                                .iter()
                                .all(|(n, _)| !n.to_lowercase().contains(&q))
                        {
                            ui.add_space(8.0);
                            ui.weak(crate::i18n::strs().msg_no_templates_match(search_text.trim()));
                        }
                    }
                    BackPage::Print => backstage_stub(
                        ui,
                        &crate::i18n::strs().msg_rail_print(),
                        &crate::i18n::strs().msg_send_the_drawing_to_a_printer_or(),
                        &crate::i18n::strs().msg_print_stub(),
                        &crate::i18n::strs().msg_printing_arrives_with_m6(),
                    ),
                    BackPage::Share => backstage_stub(
                        ui,
                        &crate::i18n::strs().msg_share(),
                        &crate::i18n::strs().msg_share_a_copy_of_the_drawing_with(),
                        &crate::i18n::strs().msg_share_a_copy(),
                        &crate::i18n::strs().msg_sharing_arrives_with_m6(),
                    ),
                    BackPage::Export => {
                        ui.heading(crate::i18n::strs().msg_rail_export());
                        ui.add_space(6.0);
                        ui.weak(crate::i18n::strs().msg_export_page_sub());
                        ui.add_space(6.0);
                        let page_count = self.ed().map_or(0, |ed| ed.doc.pages.len());
                        match self.export_pane.show(ui, page_count) {
                            crate::export::PaneAction::None => {}
                            crate::export::PaneAction::Browse(format) => {
                                self.begin_dialog(PendingKind::ExportSave(format));
                            }
                            crate::export::PaneAction::Export => {
                                let dir = self.path.parent().map(PathBuf::from);
                                if let Some(ed) = self.ed.as_ref() {
                                    self.export_pane
                                        .launch(ui.ctx(), &ed.doc, self.page_idx, dir);
                                }
                            }
                        }
                    }
                }
            });
        self.backstage_search = search_text;
        if let Some(a) = action {
            match a {
                BackAction::Save => {
                    self.do_save(false);
                    close_backstage = true;
                }
                BackAction::SaveAs => {
                    self.do_save(true);
                    close_backstage = true;
                }
                BackAction::CloseDoc => {
                    // Ask before a dirty document is discarded (#65);
                    // Cancel returns to the document, so the backstage
                    // closes either way, Visio-style.
                    if self.dirty && self.ed.is_some() {
                        self.confirm_close = Some(CloseTarget::Doc);
                    } else {
                        self.close_doc();
                    }
                    close_backstage = true;
                }
            }
        }
        if open_browse {
            close_backstage = true;
            self.do_open();
        }
        if let Some(p) = open_recent {
            close_backstage = true;
            self.load(p);
        }
        if new_blank {
            self.do_new();
            if !self.confirm_new {
                close_backstage = true;
            }
        }
        if close_backstage {
            self.backstage = false;
        }
    }
}
