//! The spellcheck pass over shape text (F7): driving, the window,
//! and outcome application.

use super::*;

impl App {
    /// F7: the dialog-driven pass over the page's shape text. A live
    /// text edit commits first (#22's click-away semantics — the same
    /// journey the Insert Field dialog's keyboard path takes), so the
    /// check sees the committed text.
    pub(super) fn start_spellcheck(&mut self) {
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        let Some(page) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else {
            return;
        };
        let found = self.spell.start(&ed.doc, page);
        if !found {
            self.spell_finish_status();
            return;
        }
        self.select_spell_shape();
    }

    /// The pass-ended status: the completion note on a clean page, the
    /// dictionary-failure note when the checker never came up (a
    /// discovered pair that fails to load is not a clean page —
    /// CodeRabbit round).
    pub(super) fn spell_finish_status(&mut self) {
        let loaded = dc::spellcheck::loaded(&self.spell.lang);
        self.status = if loaded {
            crate::i18n::strs().msg_spell_check_complete()
        } else {
            crate::i18n::strs().msg_spell_dict_failed()
        };
    }

    /// While the dialog is open the misspelled shape is selected (the
    /// Visio model: the flagged word is visible on canvas).
    pub(super) fn select_spell_shape(&mut self) {
        let sel = self
            .spell
            .session
            .as_ref()
            .and_then(|s| s.word.as_ref().map(|w| (s.page, w.shape)));
        if let Some(sel) = sel {
            self.set_selection(Some(sel));
        }
    }

    /// Advance the pass; completion closes the dialog with the
    /// status-bar note.
    pub(super) fn spell_advance(&mut self) {
        let Some(ed) = self.ed.as_ref() else {
            return;
        };
        let found = self.spell.advance(&ed.doc);
        if found {
            self.select_spell_shape();
        } else {
            self.spell.session = None;
            self.spell_finish_status();
        }
    }

    pub(super) fn spell_window(&mut self, ctx: &egui::Context) {
        if self.spell.session.is_none() {
            return;
        }
        // Pin to the page the pass opened on (shape IDs collide across
        // pages and documents — the fields-dialog rule). The document
        // closing under an open pass ends it.
        let pass_page = self.spell.session.as_ref().map(|s| s.page);
        if pass_page != self.page_id() || self.ed.is_none() {
            self.spell.session = None;
            return;
        }
        let mut open = true;
        let outcome = crate::spellcheck::window(ctx, &mut open, &mut self.spell);
        if !open {
            self.spell.session = None;
            return;
        }
        self.handle_spell_outcome(outcome);
    }

    pub(super) fn handle_spell_outcome(&mut self, outcome: crate::spellcheck::SpellOutcome) {
        use crate::spellcheck::SpellOutcome as O;
        match outcome {
            O::Open => {}
            O::Close => self.spell.session = None,
            O::Restart => {
                // The language dropdown changed — restart the pass on
                // the same page with the new dictionary.
                let page = self
                    .spell
                    .session
                    .as_ref()
                    .map(|s| s.page)
                    .or(self.page_id());
                if let Some(page) = page {
                    let Some(ed) = self.ed.as_ref() else {
                        return;
                    };
                    let found = self.spell.start(&ed.doc, page);
                    if found {
                        self.select_spell_shape();
                    } else {
                        self.spell.session = None;
                        self.spell_finish_status();
                    }
                }
            }
            O::Ignore => {
                // Skip this occurrence: the cursor moves past it.
                let end = self
                    .spell
                    .session
                    .as_ref()
                    .and_then(|s| s.word.as_ref())
                    .map(|w| w.token.end);
                if let Some(end) = end {
                    if let Some(session) = self.spell.session.as_mut() {
                        session.cursor = end;
                    }
                }
                self.spell_advance();
            }
            O::IgnoreAll => {
                let word = self
                    .spell
                    .session
                    .as_ref()
                    .and_then(|s| s.word.as_ref())
                    .map(|w| w.token.word.clone());
                if let Some(word) = word {
                    if !self.spell.ignore_all.contains(&word) {
                        self.spell.ignore_all.push(word);
                    }
                }
                self.handle_spell_outcome(O::Ignore);
            }
            O::Change { to } => {
                let edit = self
                    .spell
                    .session
                    .as_ref()
                    .and_then(|s| s.word.as_ref().map(|w| (s.page, w.shape, w.token.clone())));
                let Some((page, shape, token)) = edit else {
                    return;
                };
                if to == token.word {
                    self.handle_spell_outcome(O::Ignore);
                    return;
                }
                let Some(ed) = self.ed.as_ref() else {
                    return;
                };
                // Stale-snapshot guard (CodeRabbit round): an undo, redo,
                // or any edit while the dialog is open can invalidate the
                // captured span — the replacement only fires when the
                // SAME word still occupies the SAME plain span. A stale
                // token restarts the shape's scan instead of splicing.
                let fresh = ed
                    .doc
                    .pages
                    .iter()
                    .any(|p| p.id == page)
                    && dc::spellcheck::effective_raw(&ed.doc, page, shape).is_some_and(
                        |raw| {
                            dc::spellcheck::words(&raw).iter().any(|t| {
                                t.start == token.start
                                    && t.end == token.end
                                    && t.word == token.word
                            })
                        },
                    );
                if !fresh {
                    if let Some(session) = self.spell.session.as_mut() {
                        session.cursor = 0;
                    }
                    self.spell_advance();
                    return;
                }
                let built =
                    crate::spellcheck::replace_macro(&ed.doc, page, shape, &token, &to);
                match built {
                    Some((mac, cursor)) => {
                        self.apply(mac);
                        if let Some(session) = self.spell.session.as_mut() {
                            session.cursor = cursor;
                        }
                        // The replacement re-checks from its own start: a fix
                        // that introduces a new typo flags immediately.
                        self.spell_advance();
                    }
                    // An unsplicable span (a CDATA boundary crossing) also
                    // restarts the scan rather than splicing blind.
                    None => self.handle_spell_outcome(O::Ignore),
                }
            }
            O::ChangeAll { to } => {
                let run = self.spell.session.as_ref().and_then(|s| {
                    s.word
                        .as_ref()
                        .map(|w| (s.page, w.token.word.clone(), s.shapes.clone()))
                });
                let Some((page, word, shapes)) = run else {
                    return;
                };
                if word.eq_ignore_ascii_case(&to) {
                    self.handle_spell_outcome(O::Ignore);
                    return;
                }
                let Some(ed) = self.ed.as_ref() else {
                    return;
                };
                let lang = self.spell.lang.clone();
                let built =
                    crate::spellcheck::change_all_macro(&ed.doc, page, &shapes, &lang, &word, &to);
                match built {
                    Some(mac) => {
                        self.apply(mac);
                        // Whatever Change All fixed, the current shape re-scans
                        // from its start — the Ignore-All list holds, so only
                        // genuinely other typos re-flag.
                        if let Some(session) = self.spell.session.as_mut() {
                            session.cursor = 0;
                        }
                        self.spell_advance();
                    }
                    // An unsplicable span (a CDATA boundary crossing)
                    // advances past the current word like the single-Change
                    // path does — a global cursor reset would re-flag the
                    // same word forever with no progress (PR #299 review).
                    None => self.handle_spell_outcome(O::Ignore),
                }
            }
            O::Add => {
                let word = self
                    .spell
                    .session
                    .as_ref()
                    .and_then(|s| s.word.as_ref())
                    .map(|w| w.token.word.clone());
                if let Some(word) = word {
                    if !self.spell.custom.contains(&word) {
                        self.spell.custom.push(word);
                        // Headless instances never persist (the stencil
                        // pinning rule — a kittest Add must not rewrite
                        // the user's real dictionary).
                        if self.spell.persist {
                            crate::spellcheck::save_custom(&self.spell.custom);
                        }
                    }
                }
                self.handle_spell_outcome(O::Ignore);
            }
        }
    }
}
