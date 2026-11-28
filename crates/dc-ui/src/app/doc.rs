//! The document lifecycle: the async load pipeline, editor
//! accessors, and the undo/redo and page commands over dc-core.

use super::*;

impl App {
    pub(super) fn load(&mut self, path: PathBuf) {
        // Visio semantics: File→Open of a stencil lands it in the
        // Shapes pane, not the editor — the drawing stays untouched
        // (#83). Stencil loads are small and stay synchronous.
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("vssx"))
        {
            self.open_stencil(path);
            return;
        }
        self.begin_async_load(path);
    }

    /// #224: the async open — the parse runs on a worker thread (the
    /// same pattern as the rfd pick) so a large .vsdx never freezes the
    /// frame loop. The overlay + status line cover the flight; the
    /// result lands through [`App::finish_load`] on a later frame. The
    /// parsed Document is plain data (Send); the editor — whose History
    /// owns `Box<dyn Command>` — is built on the main thread. EVERY
    /// document-open entry rides this (the Recent list and backstage
    /// opens used to stay synchronous — the owner's #242 report: the
    /// parse froze the UI and the loading scrim never showed).
    pub(super) fn begin_async_load(&mut self, path: PathBuf) {
        if self.loading.is_some() {
            return; // one in-flight open at a time (modal for v1)
        }
        let (tx, rx) = mpsc::channel();
        let worker_path = path.clone();
        std::thread::spawn(move || {
            // A panicking parse must surface as a failed open, not a
            // dead channel (#242: the recent-entry fixture managed to
            // kill the worker invisibly).
            let result = std::panic::catch_unwind(|| vsdx::parse::parse(&worker_path))
                .unwrap_or_else(|p| {
                    Err(anyhow::anyhow!(
                        "the parse panicked: {}",
                        p.downcast_ref::<String>()
                            .cloned()
                            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown panic".into())
                    ))
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.status = format!(
            "Opening {}…",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        );
        self.loading = Some(InflightLoad {
            path,
            started: std::time::Instant::now(),
            rx,
            result: None,
        });
    }

    /// The shared open-completion path: the #224 worker's landing and
    /// the synchronous callers (tests, Recent lists) all end here.
    /// Lands one worker result; the return says whether an EDITOR
    /// landed — a failed parse or the #263 no-pages rejection both
    /// refuse (with a status note) and their callers must not enter
    /// the render-completion scrim (PR #269 review: an async no-pages
    /// open kept the loading scrim up forever — nothing renders, so
    /// the drain never closes it).
    pub(super) fn finish_load(&mut self, path: PathBuf, result: Result<vsdx::Document, String>) -> bool {
        match result {
            Ok(ed) => {
                // #263: a package that parses but carries NO pages (the
                // reporter's minimal hand-authored file — one master, an
                // empty Pages index) used to land in the editor and panic
                // the first frame at `pages[0]`; on Windows that was an
                // instant silent exit, no window, no error. The UI refuses
                // to delete the last page, so opens are the only way a
                // pages-empty document can exist — reject it here (every
                // document-open entry lands through this fn) with a status
                // note, and the app stays up.
                if ed.pages.is_empty() {
                    self.status =
                        crate::i18n::strs().msg_open_failed_no_pages(path.display().to_string());
                    return false;
                }
                self.status = crate::i18n::strs().msg_opened_path(path.display().to_string());
                self.recent.record(&path);
                // #192 spec: opening ADDS a document — the active one
                // (if any) becomes an inactive session first. The
                // live text edit commits against the OLD document
                // BEFORE the swap: a surviving session would resolve
                // its page/shape IDs against the NEW document and
                // write the old draft's text and field formulas there
                // (CodeRabbit PR review).
                if self.text_edit.is_some() {
                    self.end_text_edit(true);
                }
                self.stash_active_doc();
                // The Insert Field dialog never carries across the
                // document swap (#267, CodeRabbit round 4): page and
                // shape IDs both collide across documents — and the
                // retained last-edit caret is page/shape-ID-scoped
                // too, so it must not outlive its document
                // (CodeRabbit/Greptile round 5).
                self.fields_open = None;
                self.fields_caret = None;
                self.fields_draft = None;
                self.last_edit_caret = None;
                // The Check Spelling pass is document-scoped the same
                // way (#121): page and shape IDs collide across
                // documents, so the session must not outlive its own.
                self.spell.session = None;
                // The Find session's cached match list is
                // document-scoped the same way (#115): its key cannot
                // see the swap (same query, same undo depth), so the
                // list must not outlive its document. The window
                // stays open — the next Find Next recomputes.
                self.find.session.matches.clear();
                self.find.session.key = None;
                self.find.session.nav_sel = None;
                self.find.session.cursor = 0;
                self.find.session.current = None;
                self.repl.session.matches.clear();
                self.repl.session.key = None;
                self.repl.session.nav_sel = None;
                self.repl.session.cursor = 0;
                self.repl.session.current = None;
                self.find.status = crate::find_dlg::FindStatus::Idle;
                self.ed = Some(Editor::new(ed));
                self.reload_snap_settings();
                self.path = path;
                self.dirty = false;
                self.version += 1;
                self.view = View::default();
                self.page_idx = 0;
                self.set_selection(None);
                self.group_edit.clear();
                self.stencil.armed = None;
                self.tex.clear();
                // Per-shape layers are keyed by shape id, which
                // collides across documents (both number from 1), and
                // set_page() alone won't clear when the page ids match
                // — drop the old document's textures/bounds.
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
                true
            }
            Err(e) => {
                self.status = crate::i18n::strs().msg_open_failed_e(e);
                false
            }
        }
    }

    /// #224: consume a finished file-open worker result. `Some(path)`
    /// means still in flight (the caller draws the overlay + the
    /// status heartbeat); the landing goes through
    /// [`App::finish_load`].
    pub(super) fn poll_loading(&mut self) -> Option<(PathBuf, std::time::Instant)> {
        let mut load = self.loading.take()?;
        // Drain the worker's result into the flight's own slot (#242):
        // a value pulled early stays on the flight until the minimum
        // display elapses, instead of being dropped (a bare try_recv
        // into a local left the channel empty and every later poll
        // reported a dead worker).
        if load.result.is_none() {
            match load.rx.try_recv() {
                Ok(r) => load.result = Some(r),
                Err(mpsc::TryRecvError::Empty) => {}
                // The sender is gone: the value (if any) is already in
                // the slot; an empty slot with a dead sender is a dead
                // worker.
                Err(mpsc::TryRecvError::Disconnected) if load.result.is_none() => {
                    self.status = crate::i18n::strs().msg_open_failed_the_load_worker_died();
                    return None;
                }
                Err(_) => {}
            }
        }
        // Land only when the result is here AND the scrim showed its
        // minimum window (#242).
        if load.result.is_some()
            && load.started.elapsed().as_secs_f32() >= crate::loading::LOADING_MIN_SECS
        {
            let result = load.result.take().expect("result gated the landing");
            let path = load.path.clone();
            let loaded = self.finish_load(path.clone(), result);
            // The first render is the heavy half (every shape rasters +
            // lands): keep the scrim up until the layer store drains
            // (#242 — "run until render is complete"). A FAILED open
            // must not enter it — its drain would overwrite the error
            // status with "opened …" (#218 review).
            if loaded {
                self.rendering = Some((path, load.started));
            }
            return None;
        }
        self.status = format!(
            "Opening {}…",
            load.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| load.path.display().to_string())
        );
        let (p, s) = (load.path.clone(), load.started);
        self.loading = Some(load);
        Some((p, s))
    }
}

impl App {
    pub(super) fn ed(&self) -> Option<&Editor> {
        self.ed.as_ref()
    }

    pub(super) fn ed_mut(&mut self) -> Option<&mut Editor> {
        self.ed.as_mut()
    }

    pub(super) fn page_geom(&self) -> PageGeom {
        match self.ed() {
            Some(ed) => {
                let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
                let n = |name: &str| {
                    page.sheet()
                        .and_then(|s| s.cell(name))
                        .and_then(|c| c.numeric())
                        .unwrap_or(0.0)
                };
                PageGeom {
                    w_in: n("PageWidth").max(0.1),
                    h_in: n("PageHeight").max(0.1),
                }
            }
            None => PageGeom {
                w_in: 8.5,
                h_in: 11.0,
            },
        }
    }

    pub(super) fn page_id(&self) -> Option<i64> {
        self.ed()
            .map(|ed| ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))].id)
    }

    pub(super) fn apply<C: dc_core::Command + 'static>(&mut self, cmd: C) {
        // Field-disjoint borrows (ed/layers vs status): the #219
        // damage routing needs the editor document alongside the
        // layer store, so this site inlines page_id().
        let Some(ed) = self.ed.as_mut() else { return };
        // #219: capture the edited sub-shapes' PRE-edit ink bounds
        // from the pre-apply document — the old half of each damage
        // rect. A failed apply drops them (nothing routed).
        let pre = self
            .layers
            .harvest_pre_damage(&ed.doc, &cmd.touched(&ed.doc));
        match ed.apply(cmd) {
            Ok((label, touched)) => {
                let page = ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))].id;
                self.layers.touch_damage(&ed.doc, &touched, &pre, page);
                self.status = label;
                self.dirty = true;
                self.version += 1; // Info thumbnail (whole page)
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    /// Undo/redo selection restore (#136): instead of dropping the
    /// selection, map it through the command. The pre-command selection
    /// simply survives when its shapes are still on the page — undoing
    /// a move or resize keeps the shape selected (handles stay live),
    /// and the Format pane it drove stays up. When the selection died
    /// with the command — a delete cleared it, or an undone placement
    /// removed its shape — the command's membership touches that
    /// resolve again become the selection: undoing a delete restores
    /// *and selects* the shape (observed Visio 2016 behavior), redoing
    /// a placement lands it selected like a fresh placement. Clearing
    /// is the fallback when neither yields anything (page-structure
    /// edits, commands that only touched other pages).
    pub(super) fn restore_selection_after_history(&mut self, touched: &dc_core::TouchList) {
        // Group-edit mode (#78) selects members of the open group and
        // defers nested handle semantics; a command's touched rollup is
        // top-level, and mapping it here would jump the selection out
        // of the open group. Keep the member selection and only prune
        // entries whose shapes no longer resolve.
        if !self.group_edit.is_empty() {
            let alive: Vec<(i64, i64)> = self
                .ed()
                .and_then(|ed| ed.doc.pages.get(self.page_idx))
                .map(|page| {
                    self.sel_set
                        .iter()
                        .copied()
                        .filter(|(_, s)| vsdx::model::find_shape_by_id(&page.shapes, *s).is_some())
                        .collect()
                })
                .unwrap_or_default();
            self.sel = match self.sel {
                Some(entry) if alive.contains(&entry) => Some(entry),
                _ => alive.last().copied(),
            };
            self.sel_set = alive;
            return;
        }
        let picked: Option<Vec<(i64, i64)>> = self.ed().and_then(|ed| {
            let page = ed
                .doc
                .pages
                .get(self.page_idx.min(ed.doc.pages.len().saturating_sub(1)))?;
            // The pre-command selection, pruned to entries that are
            // still TOP-LEVEL on the active page after the
            // inverse/apply. Top-level only (not find_shape_by_id,
            // which resolves nested members): page-mode selection is
            // top-level by construction, and a re-nesting inverse —
            // undo of an ungroup puts the selected member back inside
            // its group — must drop it, or handles and Delete would
            // act on a hidden nested shape (#136 review).
            let old_alive: Vec<(i64, i64)> = self
                .sel_set
                .iter()
                .copied()
                .filter(|(p, s)| *p == page.id && page.shapes.iter().any(|sh| sh.id == *s))
                .collect();
            if !old_alive.is_empty() {
                return Some(old_alive);
            }
            // Membership touches (shapes that entered or left the page)
            // that exist again, in page z-order — the same order a
            // marquee pick produces.
            let restored: std::collections::HashSet<i64> = touched
                .shapes
                .iter()
                .filter(|t| t.page == page.id && t.touch == dc_core::Touch::Membership)
                .map(|t| t.shape)
                .collect();
            Some(
                page.shapes
                    .iter()
                    .filter(|s| restored.contains(&s.id))
                    .map(|s| (page.id, s.id))
                    .collect(),
            )
        });
        let picked = picked.unwrap_or_default();
        if picked.is_empty() {
            self.set_selection(None);
            return;
        }
        // Primary: the pre-history primary when it survived; otherwise
        // the top-most survivor, matching a fresh marquee pick.
        self.sel = match self.sel {
            Some(entry) if picked.contains(&entry) => Some(entry),
            _ => Some(picked[picked.len() - 1]),
        };
        self.sel_set = picked;
    }

    pub(super) fn do_undo(&mut self) {
        if let Some(ed) = self.ed.as_mut() {
            match ed.undo() {
                Ok(Some((l, touched))) => {
                    // #219: no pre-apply harvest here — the undo pairs
                    // against the per-transition bounds cache, which
                    // holds exactly the state being reverted to.
                    // (Field-disjoint borrows; page_id() is inlined.)
                    let page =
                        ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))].id;
                    self.layers.touch_damage(&ed.doc, &touched, &[], page);
                    self.status = crate::i18n::strs().msg_undo_l(l);
                    self.dirty = true;
                    self.version += 1;
                    self.restore_selection_after_history(&touched);
                }
                Ok(None) => self.status = crate::i18n::strs().msg_nothing_to_undo(),
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }

    /// Switch the active page (#54): selection and view position park
    /// under the outgoing page's ID and restore when it comes back;
    /// chrome (rulers/grid/units) is app-level and follows the live
    /// view, like Visio.
    pub(super) fn switch_page(&mut self, to: usize) {
        let Some((cur_id, new_id)) = self.ed().and_then(|ed| {
            let n = ed.doc.pages.len();
            (to < n && to != self.page_idx)
                .then(|| ed.doc.pages.get(self.page_idx).map(|c| c.id))
                .flatten()
                .zip(ed.doc.pages.get(to).map(|p| p.id))
        }) else {
            return;
        };
        // An in-place text edit ends on the page switch (Visio commits
        // on the focus change; #22 review) — before the view parks, so
        // the parked view is the pre-edit one.
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        // The Insert Field dialog closes too (#267, CodeRabbit round
        // 1): it is bound to a SHAPE ID, and ids collide across pages
        // — an open dialog must never resolve against a different
        // page's same-id shape.
        self.fields_open = None;
        self.fields_caret = None;
        self.fields_draft = None;
        // #121: the spelling pass does not survive a document load
        // (same-ID collision rule).
        self.spell.session = None;
        {
            self.page_states.insert(
                cur_id,
                PageUiState {
                    sel: self.sel,
                    sel_set: self.sel_set.clone(),
                    view: self.view.clone(),
                },
            );
        }
        self.page_idx = to;
        // Group-edit state does not carry across pages: shape ids
        // overlap across pages, and the per-frame coherence check only
        // drops levels that stop resolving to a Group — an id that
        // happens to be a group on the new page would keep the mode
        // alive with the wrong scope (#78).
        self.group_edit.clear();
        self.tab_scroll_to_active = true;
        match self.page_states.remove(&new_id) {
            Some(st) => {
                self.sel = st.sel;
                self.sel_set = st.sel_set;
                let (unit, grid, rulers) =
                    (self.view.unit, self.view.show_grid, self.view.show_rulers);
                self.view = st.view;
                self.view.unit = unit;
                self.view.show_grid = grid;
                self.view.show_rulers = rulers;
            }
            None => {
                self.set_selection(None);
                self.view.fit_pending = true;
            }
        }
    }

    /// Next sequential Visio page name for a new/duplicated tab.
    pub(super) fn next_page_name(&self) -> String {
        let mut n = self.ed().map(|e| e.doc.pages.len()).unwrap_or(0) + 1;
        let taken = |self_: &Self, name: &str| {
            self_
                .ed()
                .map(|e| e.doc.pages.iter().any(|p| p.name == name))
                .unwrap_or(false)
        };
        while taken(self, &format!("Page-{n}")) {
            n += 1;
        }
        format!("Page-{n}")
    }

    /// Drop per-page state and selection that outlived their pages
    /// (delete) or no longer match the active page (#54).
    pub(super) fn reconcile_page_state(&mut self) {
        let live: Option<std::collections::HashSet<i64>> = self
            .ed()
            .map(|e| e.doc.pages.iter().map(|p| p.id).collect());
        if let Some(live) = live {
            self.page_states.retain(|id, _| live.contains(id));
        }
        self.page_idx = self.page_idx.min(
            self.ed()
                .map(|e| e.doc.pages.len())
                .unwrap_or(0)
                .saturating_sub(1),
        );
        if let Some((p, _)) = self.sel {
            if Some(p) != self.page_id() {
                self.set_selection(None);
            }
        }
    }

    /// Tab context-menu commands (#54). Each goes through the command
    /// layer (undoable) and lands the user on the affected page.
    pub(super) fn tab_cmd_insert(&mut self, after: usize) {
        let name = self.next_page_name();
        // #68: Visio's Insert Page copies the current page's setup —
        // seed the new sheet from the active page.
        let add = match self.page_id() {
            Some(id) => AddPage::seeded_from(name, id),
            None => AddPage::new(name),
        };
        self.apply(add);
        if let Some(n) = self.ed().map(|e| e.doc.pages.len()) {
            if n > 1 && after + 1 < n {
                self.apply(MovePage {
                    from: n - 1,
                    to: after + 1,
                });
            }
            self.page_idx = after + 1;
        }
        self.view.fit_pending = true;
        self.tab_scroll_to_active = true;
    }

    pub(super) fn tab_cmd_duplicate(&mut self, idx: usize) {
        let Some(id) = self.ed().and_then(|e| e.doc.pages.get(idx)).map(|p| p.id) else {
            return;
        };
        let name = self.next_page_name();
        self.apply(DuplicatePage::new(id));
        self.apply(RenamePage::new(
            self.ed()
                .and_then(|e| e.doc.pages.get(idx + 1))
                .map(|p| p.id)
                .unwrap_or(id),
            name,
        ));
        self.page_idx = idx + 1;
        self.view.fit_pending = true;
        self.tab_scroll_to_active = true;
    }

    pub(super) fn tab_cmd_delete(&mut self, idx: usize) {
        let Some(ed) = self.ed() else { return };
        if ed.doc.pages.len() < 2 {
            self.status = crate::i18n::strs().msg_the_last_page_cannot_be_deleted();
            return;
        }
        let Some(id) = ed.doc.pages.get(idx).map(|p| p.id) else {
            return;
        };
        self.apply(RemovePage::new(id));
        self.reconcile_page_state();
        self.view.fit_pending = true;
    }

    pub(super) fn tab_cmd_reorder(&mut self, idx: usize, to: usize) {
        let n = self.ed().map(|e| e.doc.pages.len()).unwrap_or(0);
        if idx >= n || to >= n {
            return;
        }
        let active = self.page_id();
        self.apply(MovePage { from: idx, to });
        // The ACTIVE page stays active wherever it lands (Visio's
        // reorder dialog never moves your view).
        if let Some(id) = active {
            if let Some(p) = self
                .ed()
                .and_then(|e| e.doc.pages.iter().position(|p| p.id == id))
            {
                self.page_idx = p;
            }
        }
        self.tab_scroll_to_active = true;
    }

    pub(super) fn do_redo(&mut self) {
        if let Some(ed) = self.ed.as_mut() {
            match ed.redo() {
                Ok(Some((l, touched))) => {
                    // #219: redo pairs against the bounds cache, like
                    // undo (field-disjoint borrows; page_id() inlined).
                    let page =
                        ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))].id;
                    self.layers.touch_damage(&ed.doc, &touched, &[], page);
                    self.status = crate::i18n::strs().msg_redo_l(l);
                    self.dirty = true;
                    self.version += 1;
                    // Same selection mapping as undo (#136) — redo never
                    // touched the selection before, so a redo-of-delete
                    // could leave a selection pointing at a removed shape.
                    self.restore_selection_after_history(&touched);
                }
                Ok(None) => self.status = crate::i18n::strs().msg_nothing_to_redo(),
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }
}
