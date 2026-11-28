//! The Find/Replace dialogs' app side (GitHub #115): hosting the two
//! modeless windows, computing and cycling matches, navigating to
//! the owning shape, the replace actions (phase 3 — text splices
//! through `ReplaceTextRange`, cells through `SetRowCell`, one undo
//! per Replace and one per Replace All sweep), and the ribbon's Find
//! dropdown menu.

use super::*;

/// How many queries each history dropdown keeps.
const HISTORY_LIMIT: usize = 10;

/// Where a replaced match's search resumes: the replaced TARGET (its
/// span offsets are per-target — plain text, one value string per
/// row) and the character position just past the replacement
/// (matches inside the inserted text are skipped — replacing `a`
/// with `aa` must not find the new `a`), plus the pre-replace match
/// list for when the shape lost its last match.
struct ResumePoint {
    replaced: dc_core::FindMatch,
    past: usize,
    old_matches: Vec<dc_core::FindMatch>,
}

/// Push into a history list (deduplicated, most recent first,
/// bounded — session lifetime, the #115 decision).
fn push_history(history: &mut Vec<String>, entry: String) {
    if entry.is_empty() {
        return;
    }
    history.retain(|h| *h != entry);
    history.insert(0, entry);
    history.truncate(HISTORY_LIMIT);
}

impl App {
    /// The modeless Find window (spec 2). Usable while the canvas
    /// edits; the document closing under it ends it.
    pub(super) fn find_window(&mut self, ctx: &egui::Context) {
        if !self.find.open {
            return;
        }
        if self.ed.is_none() {
            self.find.open = false;
            return;
        }
        match crate::find_dlg::window(ctx, &mut self.find) {
            crate::find_dlg::FindOutcome::Open => {}
            crate::find_dlg::FindOutcome::FindNext => self.do_find_next(),
            crate::find_dlg::FindOutcome::Help => {
                // The Get-Help stub (#204 step 7), opened with the
                // command's name.
                self.tellme_help = Some(crate::i18n::strs().msg_find());
                self.tellme_help_epoch = self.bump_chrome_epoch();
            }
            crate::find_dlg::FindOutcome::Close => self.find.open = false,
        }
    }

    /// The modeless Replace window (spec 3, phase 3) — the same
    /// hosting rules.
    pub(super) fn replace_window(&mut self, ctx: &egui::Context) {
        if !self.repl.open {
            return;
        }
        if self.ed.is_none() {
            self.repl.open = false;
            return;
        }
        match crate::replace_dlg::window(ctx, &mut self.repl) {
            crate::replace_dlg::ReplaceOutcome::Open => {}
            crate::replace_dlg::ReplaceOutcome::FindNext => self.do_replace_find_next(),
            crate::replace_dlg::ReplaceOutcome::Replace => self.do_replace(),
            crate::replace_dlg::ReplaceOutcome::ReplaceAll => self.do_replace_all(),
            crate::replace_dlg::ReplaceOutcome::Help => {
                self.tellme_help = Some(crate::i18n::strs().msg_replace());
                self.tellme_help_epoch = self.bump_chrome_epoch();
            }
            crate::replace_dlg::ReplaceOutcome::Close => self.repl.open = false,
        }
    }

    fn session_key(&self, sess: &crate::find_dlg::FindSession) -> crate::find_dlg::FindKey {
        // The Selection scope reads the selection the search STARTED
        // from while the live selection is still exactly what the
        // last Find Next selected (navigating must not re-scope the
        // search to the one shape it just found); any other live
        // selection — the user's change — re-scopes.
        let selection = if sess.scope == crate::find_dlg::FindScopeKind::Selection {
            match (&sess.key, &sess.nav_sel) {
                // The hold applies only while the scope STAYS
                // Selection: entering the scope fresh (after a
                // Current/All-pages search left nav_sel equal to the
                // live selection) must read that live selection, not
                // the empty `prev.selection` (CodeRabbit round on
                // the #115 PR).
                (Some(prev), Some(nav)) if prev.scope == sess.scope && &self.sel_set == nav => {
                    prev.selection.clone()
                }
                _ => self.sel_set.clone(),
            }
        } else {
            Vec::new()
        };
        crate::find_dlg::FindKey {
            query: sess.query.clone(),
            scope: sess.scope,
            page: if sess.scope == crate::find_dlg::FindScopeKind::CurrentPage {
                self.page_id()
            } else {
                None
            },
            selection,
            targets: sess.targets,
            match_case: sess.match_case,
            match_width: sess.match_width,
            whole_words: sess.whole_words,
            // The DOCUMENT revision (`App::version`, bumped by every
            // edit, undo, redo and open) — the undo-stack depth would
            // collide between "n edits" and "n-1 edits, one undone,
            // another made" (CodeRabbit round on phase 2).
            rev: self.version,
        }
    }

    /// Recompute a session's match list when its key went stale.
    fn refresh_session(&self, sess: &mut crate::find_dlg::FindSession) {
        let key = self.session_key(sess);
        if sess.key.as_ref() == Some(&key) {
            return;
        }
        let Some(ed) = self.ed.as_ref() else {
            sess.matches.clear();
            sess.key = None;
            return;
        };
        // The Selection scope searches the selection the KEY carries
        // — with navigation holding, that is the selection the search
        // started from, not the live one Find Next itself just
        // narrowed (CodeRabbit round on phase 2); any other live
        // selection re-keyed already.
        let scope = match sess.scope {
            crate::find_dlg::FindScopeKind::Selection => {
                dc_core::FindScope::Selection(&key.selection)
            }
            crate::find_dlg::FindScopeKind::CurrentPage => match self.page_id() {
                Some(p) => dc_core::FindScope::CurrentPage(p),
                None => {
                    sess.matches.clear();
                    return;
                }
            },
            crate::find_dlg::FindScopeKind::AllPages => dc_core::FindScope::AllPages,
        };
        let opts = dc_core::FindOptions {
            targets: sess.targets,
            match_case: sess.match_case,
            match_width: sess.match_width,
            whole_words: sess.whole_words,
        };
        sess.matches = dc_core::find_matches(&ed.doc, &sess.query, scope, &opts);
        sess.key = Some(key);
        sess.current = None; // stale coordinates until the next Find Next
    }

    /// One Find-Next step over a session: refresh when the key
    /// changed (optionally resuming just past a replacement instead
    /// of from the first match), advance with wrap, and navigate to
    /// the owning shape — which becomes the session's CURRENT match
    /// (Replace's gate). A live canvas text edit commits first so
    /// the search sees the current text (the spell dialog's rule).
    /// Returns the found location's display names, or `None` when
    /// the search ran dry.
    fn step_session(
        &mut self,
        sess: &mut crate::find_dlg::FindSession,
        resume: Option<ResumePoint>,
    ) -> Option<(String, String)> {
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        let stale = match &sess.key {
            Some(have) => self.session_key(sess) != have.clone(),
            None => true,
        };
        if stale {
            self.refresh_session(sess);
            sess.cursor = match resume {
                // Resume just past the replacement — in the replaced
                // TARGET's own offsets — else at the next match after
                // it in walk order; wrapping to the first match when
                // nothing follows.
                Some(rp) => resume_index(&sess.matches, &rp),
                None => 0,
            };
        }
        let cursor = if sess.cursor >= sess.matches.len() {
            0
        } else {
            sess.cursor
        };
        let m = sess.matches.get(cursor).cloned()?;
        let names = self.navigate_match(&m);
        sess.current = Some(m);
        sess.nav_sel = Some(self.sel_set.clone());
        sess.cursor = cursor + 1;
        Some(names)
    }

    /// Navigate to a match: switch to its page through the canonical
    /// page switch (the outgoing page's view and selection park under
    /// its id; group-edit resets — a bare `page_idx` write lost both,
    /// CodeRabbit round on phase 2), select the owning shape, and
    /// zoom it into view (#237). Returns the location's display
    /// names.
    fn navigate_match(&mut self, m: &dc_core::FindMatch) -> (String, String) {
        let Some(ed) = self.ed.as_ref() else {
            return (String::new(), String::new());
        };
        let page = ed.doc.pages.iter().find(|p| p.id == m.page);
        let page_name = page.map(|p| p.name.clone());
        let idx = ed.doc.pages.iter().position(|p| p.id == m.page);
        let shape_label = page
            .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, m.shape))
            .and_then(|sh| sh.name_u.clone())
            .unwrap_or_else(|| crate::i18n::strs().msg_shape_id_fallback(m.shape));
        if let Some(idx) = idx {
            self.switch_page(idx);
        }
        self.set_selection(Some((m.page, m.shape)));
        self.zoom_selection_pending = true;
        (page_name.unwrap_or_default(), shape_label)
    }

    /// Find Next in the Find window (spec 2's default action): step
    /// the session and report in the dialog's status area.
    pub(super) fn do_find_next(&mut self) {
        // The session parks out of `self` for the step (the step
        // mutates the app around it); its history rides along.
        let mut sess = std::mem::take(&mut self.find.session);
        let found = self.step_session(&mut sess, None);
        push_history(&mut sess.history, sess.query.clone());
        self.find.session = sess;
        self.find.status = match found {
            Some((page, shape)) => crate::find_dlg::FindStatus::Found { page, shape },
            None => crate::find_dlg::FindStatus::NotFound,
        };
    }

    /// Find Next in the Replace window (spec 3): the same step; the
    /// found location reports on the status bar (spec 3's dialog has
    /// no status area).
    fn do_replace_find_next(&mut self) {
        let mut sess = std::mem::take(&mut self.repl.session);
        let found = self.step_session(&mut sess, None);
        push_history(&mut sess.history, sess.query.clone());
        self.repl.session = sess;
        self.status = match found {
            Some((page, shape)) => crate::i18n::strs().msg_found_in(page, shape),
            None => crate::i18n::strs().msg_find_not_found(),
        };
    }

    /// Replace (spec 3): replace the CURRENT match — one undo step —
    /// then automatically find the next, resuming just past the
    /// replacement. A name match replaces nothing (NameU is
    /// authored, not replaceable text): the step still advances.
    fn do_replace(&mut self) {
        // A live canvas text edit commits first (the spell dialog's
        // rule): the replacement writes committed text, and the draft
        // cannot later overwrite it (CodeRabbit round on the #115
        // PR). The commit bumps the revision, so the stale rule
        // below then governs the (now recomputed) match.
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        // A stale CURRENT match must never splice: the modeless
        // window sits over an editable canvas, so the document (or
        // the query/options) may have moved since Find Next
        // highlighted it — the stored span could overwrite the wrong
        // characters (CodeRabbit round on phase 3). Stale → this
        // click behaves as Find Next: recompute, highlight the fresh
        // match, and let the next click replace it.
        let stale = match &self.repl.session.key {
            Some(have) => self.session_key(&self.repl.session) != have.clone(),
            None => true,
        };
        if stale {
            self.do_replace_find_next();
            return;
        }
        let Some(m) = self.repl.session.current.clone() else {
            return;
        };
        let replacement = self.repl.replacement.clone();
        let cmd = self
            .ed
            .as_ref()
            .and_then(|ed| dc_core::find::replace_command(&ed.doc, &m, &replacement));
        if let Some(cmd) = cmd {
            self.apply(cmd);
        }
        push_history(&mut self.repl.repl_history, replacement.clone());
        // The resume point: just past the replacement, in the match's
        // own coordinates (plain chars for text, value chars for
        // cells).
        let decoded = dc_core::find::decode_replacement(&replacement)
            .chars()
            .count();
        let past = match &m.target {
            dc_core::FindTarget::Text { start, .. }
            | dc_core::FindTarget::ShapeData { start, .. }
            | dc_core::FindTarget::UserCell { start, .. }
            | dc_core::FindTarget::ShapeName { start, .. } => start + decoded,
        };
        let mut sess = std::mem::take(&mut self.repl.session);
        let old_matches = sess.matches.clone();
        let found = self.step_session(
            &mut sess,
            Some(ResumePoint {
                replaced: m.clone(),
                past,
                old_matches,
            }),
        );
        self.repl.session = sess;
        self.status = match found {
            Some((page, shape)) => crate::i18n::strs().msg_found_in(page, shape),
            None => crate::i18n::strs().msg_find_not_found(),
        };
    }

    /// Replace All (spec 3): every scoped match in ONE undo step (a
    /// `Macro`), reported on the status bar. Name matches are not
    /// replaceable and are skipped.
    fn do_replace_all(&mut self) {
        // A live canvas text edit commits first — same rule as
        // Replace (CodeRabbit round on the #115 PR).
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        let mut sess = std::mem::take(&mut self.repl.session);
        self.refresh_session(&mut sess);
        let matches = sess.matches.clone();
        sess.cursor = 0;
        sess.current = None;
        // nav_sel STAYS: with a Selection scope it is what keeps the
        // search's starting selection ([A, B]) authoritative over the
        // single shape the last navigation selected — clearing it
        // would re-scope the next search to that one shape
        // (CodeRabbit round on phase 3).
        self.repl.session = sess;
        let replacement = self.repl.replacement.clone();
        let built = self
            .ed
            .as_ref()
            .map(|ed| dc_core::find::replace_all_command(&ed.doc, &matches, &replacement));
        match built {
            Some((Some(mac), count)) => {
                self.apply(mac);
                push_history(&mut self.repl.repl_history, replacement.clone());
                self.status = crate::i18n::strs().msg_replacements_made(count as i64);
            }
            _ => {
                self.status = crate::i18n::strs().msg_find_not_found();
            }
        }
    }

    /// The ribbon Editing group's Find dropdown (Functional
    /// Specification 1 §3.2): Find… opens the Find dialog, Replace…
    /// the Replace dialog (phase 3). The anchor is the dispatch
    /// frame's pointer (the Task Panes pattern).
    pub(super) fn find_menu(&mut self, ctx: &egui::Context) {
        let Some(anchor) = self.find_menu else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.find_menu = None;
            self.find_menu_fresh = false;
            return;
        }
        let s = crate::i18n::strs();
        let has_doc = self.ed.is_some();
        let mut pick: Option<Action> = None;
        let menu = egui::Popup::new(
            egui::Id::unique("dc-find-menu"),
            ctx.clone(),
            anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let popup = menu.show(|ui| {
            ui.set_min_width(150.0);
            let f = ui.add_enabled(has_doc, egui::Button::new(s.msg_find_ellipsis()).small());
            if f.clicked() {
                pick = Some(Action::FindDialog);
            }
            let r = ui.add_enabled(has_doc, egui::Button::new(s.msg_replace_ellipsis()).small());
            if r.clicked() {
                pick = Some(Action::ReplaceDialog);
            }
        });
        if let Some(a) = pick {
            self.find_menu = None;
            self.find_menu_fresh = false;
            self.dispatch(a);
            return;
        }
        // Outside-primary close (the Task Panes idiom), gated one
        // frame so the opening click's release cannot shut the menu
        // it made.
        if self.find_menu_fresh {
            self.find_menu_fresh = false;
            return;
        }
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.find_menu = None;
            }
        }
    }
}

/// The index in a FRESH match list to resume from after a
/// replacement. Within the replaced shape, only the replaced TARGET's
/// own offsets matter — text, each cell row's value, and the name are
/// separate character spaces, and the block's matches are ordered by
/// class then span — so `past` filters only same-target matches
/// (CodeRabbit round on phase 3). When the shape lost its last match,
/// the search continues at the first match of the NEXT distinct shape
/// after it in the old walk order (wrapping to the first match when
/// nothing follows) — not back at the list's start.
fn resume_index(matches: &[dc_core::FindMatch], rp: &ResumePoint) -> usize {
    let m = &rp.replaced;
    let span_start = |x: &dc_core::FindMatch| match &x.target {
        dc_core::FindTarget::Text { start, .. }
        | dc_core::FindTarget::ShapeData { start, .. }
        | dc_core::FindTarget::UserCell { start, .. }
        | dc_core::FindTarget::ShapeName { start, .. } => *start,
    };
    // The emission order inside one shape's block: class first, then
    // the cell row (text/name targets carry no row).
    fn rank(x: &dc_core::FindMatch) -> (u8, Option<&str>) {
        match &x.target {
            dc_core::FindTarget::Text { .. } => (0, None),
            dc_core::FindTarget::ShapeData { row, .. } => (1, Some(row.as_str())),
            dc_core::FindTarget::ShapeName { .. } => (2, None),
            dc_core::FindTarget::UserCell { row, .. } => (3, Some(row.as_str())),
        }
    }
    if let Some(block) = matches
        .iter()
        .position(|x| x.page == m.page && x.shape == m.shape)
    {
        let end = matches
            .iter()
            .rposition(|x| x.page == m.page && x.shape == m.shape)
            .map(|e| e + 1)
            .unwrap_or(matches.len());
        // EMISSION order, not name order: a shape's matches are
        // class-grouped, and each row's spans are contiguous in the
        // section's stored row order (`find_matches` emits rows as
        // the sheet stores them — M, A, Z stays M, A, Z). Resume at
        // the first match after the replaced TARGET's own group:
        // same group needs `past`, any later group (or later row in
        // stored order) resumes as-is (CodeRabbit round on the #115
        // PR).
        let replaced_rank = rank(m);
        let group_start = matches[block..end]
            .iter()
            .position(|x| rank(x) == replaced_rank)
            .map(|i| block + i)
            .unwrap_or(block);
        if let Some(hit) = matches[group_start..end]
            .iter()
            .enumerate()
            .find(|(_, x)| {
                let r = rank(x);
                r != replaced_rank || span_start(x) >= rp.past
            })
            .map(|(i, _)| group_start + i)
        {
            return hit;
        }
        if end < matches.len() {
            return end;
        }
        return 0; // the shape's block was the tail — wrap
    }
    // The shape left the list: the first distinct shape after the
    // replaced match in the OLD walk order, looked up in the fresh
    // one (its matches were not edited by this single Replace).
    if let Some(from) = rp
        .old_matches
        .iter()
        .position(|x| x.page == m.page && x.shape == m.shape)
    {
        for x in &rp.old_matches[from..] {
            if x.page != m.page || x.shape != m.shape {
                if let Some(i) = matches
                    .iter()
                    .position(|y| y.page == x.page && y.shape == x.shape)
                {
                    return i;
                }
            }
        }
    }
    0
}
