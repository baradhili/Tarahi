//! The Find dialog's app side (GitHub #115 phase 2): hosting the
//! modeless window, computing and cycling matches, navigating to the
//! owning shape, and the ribbon's Find dropdown menu.

use super::*;

/// How many queries the history dropdown keeps.
const HISTORY_LIMIT: usize = 10;

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

    fn find_key(&self) -> crate::find_dlg::FindKey {
        // The Selection scope reads the selection the search STARTED
        // from while the live selection is still exactly what the
        // last Find Next selected (navigating must not re-scope the
        // search to the one shape it just found); any other live
        // selection — the user's change — re-scopes.
        let selection = if self.find.scope == crate::find_dlg::FindScopeKind::Selection {
            match (&self.find.key, &self.find.nav_sel) {
                (Some(prev), Some(nav)) if &self.sel_set == nav => prev.selection.clone(),
                _ => self.sel_set.clone(),
            }
        } else {
            Vec::new()
        };
        crate::find_dlg::FindKey {
            query: self.find.query.clone(),
            scope: self.find.scope,
            page: if self.find.scope == crate::find_dlg::FindScopeKind::CurrentPage {
                self.page_id()
            } else {
                None
            },
            selection,
            targets: self.find.targets,
            match_case: self.find.match_case,
            match_width: self.find.match_width,
            whole_words: self.find.whole_words,
            rev: self
                .ed
                .as_ref()
                .map(|e| e.history.undo_labels().len())
                .unwrap_or(0),
        }
    }

    /// Find Next (spec 2's default action): (re)compute the match
    /// list, advance through it — wrapping to the first match after
    /// the last — select the owning shape, bring it into view (#237's
    /// pending zoom), and report in the status area. A live canvas
    /// text edit commits first so the search sees the current text
    /// (the spell dialog's rule).
    pub(super) fn do_find_next(&mut self) {
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        let key = self.find_key();
        if self.find.key.as_ref() != Some(&key) {
            let Some(ed) = self.ed.as_ref() else {
                self.find.status = crate::find_dlg::FindStatus::NotFound;
                return;
            };
            let scope = match self.find.scope {
                crate::find_dlg::FindScopeKind::Selection => {
                    dc_core::FindScope::Selection(&self.sel_set)
                }
                crate::find_dlg::FindScopeKind::CurrentPage => match self.page_id() {
                    Some(p) => dc_core::FindScope::CurrentPage(p),
                    None => {
                        self.find.status = crate::find_dlg::FindStatus::NotFound;
                        return;
                    }
                },
                crate::find_dlg::FindScopeKind::AllPages => dc_core::FindScope::AllPages,
            };
            let opts = dc_core::FindOptions {
                targets: self.find.targets,
                match_case: self.find.match_case,
                match_width: self.find.match_width,
                whole_words: self.find.whole_words,
            };
            self.find.matches = dc_core::find_matches(&ed.doc, &self.find.query, scope, &opts);
            self.find.cursor = 0;
            self.find.key = Some(key);
            // The query joins the history (deduplicated, most recent
            // first, bounded — session lifetime).
            let q = self.find.query.clone();
            if !q.is_empty() {
                self.find.history.retain(|h| *h != q);
                self.find.history.insert(0, q);
                self.find.history.truncate(HISTORY_LIMIT);
            }
        }
        let wrapped = self.find.cursor >= self.find.matches.len();
        let cursor = if wrapped { 0 } else { self.find.cursor };
        let Some(m) = self.find.matches.get(cursor) else {
            self.find.status = crate::find_dlg::FindStatus::NotFound;
            return;
        };
        // Navigate: switch to the match's page, select the owning
        // shape, and zoom it into view.
        let Some(ed) = self.ed.as_ref() else {
            return;
        };
        let page = ed.doc.pages.iter().find(|p| p.id == m.page);
        let page_name = page.map(|p| p.name.clone());
        if let Some(idx) = ed.doc.pages.iter().position(|p| p.id == m.page) {
            self.page_idx = idx;
        }
        let shape_label = page
            .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, m.shape))
            .and_then(|sh| sh.name_u.clone())
            .unwrap_or_else(|| crate::i18n::strs().msg_shape_id_fallback(m.shape));
        self.set_selection(Some((m.page, m.shape)));
        self.find.nav_sel = Some(self.sel_set.clone());
        self.zoom_selection_pending = true;
        self.find.status = crate::find_dlg::FindStatus::Found {
            page: page_name.unwrap_or_default(),
            shape: shape_label,
        };
        self.find.cursor = cursor + 1;
    }

    /// The ribbon Editing group's Find dropdown (Functional
    /// Specification 1 §3.2): Find… opens this dialog; Replace… lands
    /// with phase 3 and stays a disabled entry carrying its
    /// planned-later tip. The anchor is the dispatch frame's pointer
    /// (the Task Panes pattern).
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
            let r = ui.add_enabled(false, egui::Button::new(s.msg_replace_ellipsis()).small());
            r.on_hover_text(s.msg_replace_planned_later_milestone());
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
