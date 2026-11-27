//! Keyboard input: shortcut handling and the Alt keytip mode
//! (badges collected in `crate::keytips`).

use super::*;

impl App {
    pub(super) fn handle_shortcuts(&mut self, ctx: &Context) {
        // #130: the Slide Show owns the keyboard at the modal tier —
        // the same precedence the Options dialog has (#304): while
        // the show runs, keytips and the application shortcuts stand
        // down and the show's own map decides everything.
        if self.slideshow.is_some() {
            self.keytips.disarm();
            self.slideshow_keys(ctx);
            return;
        }
        // #204 step 3: while the Tell Me panel is open it owns the
        // keyboard — arrows walk the results (wrapping), Enter runs
        // the selected command (a faded row does nothing, per the
        // acceptance), Esc closes the panel and returns the keys to
        // the canvas. It stands BEFORE the keytip mode and the
        // dialog/tool Esc paths so one press cannot do two things.
        // (Keeping the focus in the query field while the arrows
        // drive the panel is step 4's wiring, when the field lands.)
        if self.tellme_open {
            let results = self.tellme_results();
            let n = results.len();
            let mut run: Option<Action> = None;
            let mut close = false;
            ctx.input(|i| {
                if n > 0 {
                    if i.key_pressed(egui::Key::ArrowDown) {
                        self.tellme_sel = (self.tellme_sel + 1) % n;
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        self.tellme_sel = (self.tellme_sel + n - 1) % n;
                    }
                    if i.key_pressed(egui::Key::Enter) && results[self.tellme_sel].enabled {
                        run = Some(results[self.tellme_sel].action);
                        close = true;
                    }
                }
                if i.key_pressed(egui::Key::Escape) {
                    close = true;
                }
            });
            if close {
                self.tellme_open = false;
                self.tellme_query.clear();
                self.tellme_sel = 0;
            }
            if let Some(a) = run {
                self.dispatch(a);
            }
            return;
        }
        // #304: the Options dialog is MODAL — while it is up the
        // application shortcuts (Ctrl+Z/N/S…) never fire and keytips
        // stand down; the dialog's own fields own the keyboard (a
        // Ctrl+Z in the User name field must undo TEXT, not the
        // drawing).
        if self.options_open {
            self.keytips.disarm();
            return;
        }
        // #184 keytips own the keyboard FIRST: while Alt-armed (or on
        // an Alt+letter combo) the letter never reaches a widget.
        self.keytip_input(ctx);
        let mut open = false;
        let mut save = false;
        let mut save_as = false;
        let mut new_doc = false;
        let kb_owned = ctx.egui_wants_keyboard_input();
        // An Esc that CLOSES something (dialog / presentation chrome)
        // is consumed after this closure (see `esc_dialog`) so the one
        // keypress cannot also reach the shape-text editor, whose own
        // Esc abandons the edit.
        let mut esc_dialog = false;
        ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            if ctrl && i.key_pressed(egui::Key::N) {
                new_doc = true;
            }
            if ctrl && i.key_pressed(egui::Key::O) {
                open = true;
            }
            if ctrl && i.key_pressed(egui::Key::S) {
                if i.modifiers.shift {
                    save_as = true;
                } else {
                    save = true;
                }
            }
            if ctrl && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    self.do_redo();
                } else {
                    self.do_undo();
                }
            }
            if ctrl && i.key_pressed(egui::Key::Y) {
                self.do_redo();
            }
            // #237: zoom to selection (Visio's Zoom ▸ Selected
            // affordance). Plain Shift+Z — Ctrl+Shift+Z is redo.
            if !kb_owned && i.modifiers.shift && !ctrl && i.key_pressed(egui::Key::Z) {
                self.zoom_selection_pending = true;
            }
            if i.key_pressed(egui::Key::Delete) && !kb_owned {
                // #108: the connection-point selection only owns
                // Delete while its tool is active — a stale selection
                // must never shadow shape deletion (CodeRabbit).
                if self.tool == Tool::ConnectionPoint && self.connpt_sel.is_some() {
                    self.delete_selected_conn_point();
                } else if self.sel.is_some() {
                    self.delete_selection();
                }
            }
            // #209 step 7: Presentation Mode owns Esc outright — the
            // chrome is hidden, so restoring it is the only Esc action
            // (nothing else is reachable anyway). #130: restoring the
            // chrome restores the windowed shell — the fullscreen
            // command leaves with it.
            if self.chrome_hidden && i.key_pressed(egui::Key::Escape) {
                self.chrome_hidden = false;
                self.fullscreen_pending = Some(false);
                esc_dialog = true;
            } else
            // Esc closes the frontmost open dialog first (#110
            // tab-order audit: Visio dismisses a dialog on Esc);
            // only with none open do the tool/painter shortcuts run.
            // An open empty-page menu owns Esc (#272): it closes
            // itself in `page_context_menu`, and nothing behind it.
            if i.key_pressed(egui::Key::Escape) && !kb_owned && self.page_menu.is_none() {
                let consumed = self.close_front_dialog();
                if consumed {
                    esc_dialog = true;
                }
                if !consumed {
                    if matches!(
                        self.tool,
                        Tool::Connector
                            | Tool::Lasso
                            | Tool::ConnectionPoint
                            | Tool::Freeform
                            | Tool::Pan
                    ) {
                        self.tool = Tool::Pointer;
                    }
                    self.connpt_sel = None;
                    if self.painter_armed.is_some() {
                        self.painter_armed = None;
                        self.painter_sticky = false;
                        self.status = crate::i18n::strs().msg_format_painter_cancelled();
                    }
                }
            }
            // #283 keyboard view navigation (Visio's Ctrl+Home/End →
            // the page's top / bottom edge). Deliberately tier-2 small:
            // PageUp/PageDown stay page switches, untouched.
            if ctrl && !kb_owned && i.key_pressed(egui::Key::Home) {
                self.jump_view_vertical(true);
            }
            if ctrl && !kb_owned && i.key_pressed(egui::Key::End) {
                self.jump_view_vertical(false);
            }
            // Ctrl+A selects the page (#110, URS-1160).
            if ctrl && i.key_pressed(egui::Key::A) && !kb_owned {
                self.select_all();
            }
            // Arrow-key nudging (#110, URS-1217 keyboard slice):
            // arrows move the selection, Shift widens the step.
            // #180: with glued connectors selected, the documented
            // Visio operation takes the press instead — any arrow key
            // unglues all ends of every selected connector in one
            // undo step (MS Learn, Glue or unglue connectors; its
            // Ctrl is multi-SELECT, not a modifier on the key).
            if !kb_owned && self.sel.is_some() {
                let arrow = [
                    egui::Key::ArrowLeft,
                    egui::Key::ArrowRight,
                    egui::Key::ArrowUp,
                    egui::Key::ArrowDown,
                ]
                .into_iter()
                .any(|k| i.key_pressed(k));
                if !(arrow && self.unglue_selection_connectors()) {
                    let step = if i.modifiers.shift { 1.0 } else { 0.25 };
                    let nudge = [
                        (egui::Key::ArrowLeft, (-step, 0.0)),
                        (egui::Key::ArrowRight, (step, 0.0)),
                        (egui::Key::ArrowUp, (0.0, step)),
                        (egui::Key::ArrowDown, (0.0, -step)),
                    ];
                    for (key, (dx, dy)) in nudge {
                        if i.key_pressed(key) {
                            self.nudge_selection(dx, dy);
                        }
                    }
                }
            }
            // F7: Check Spelling (Visio's key) — a page-wide pass,
            // selection optional. A live text edit commits first
            // (start_spellcheck's #22 journey): the CANVAS editor
            // owning the keyboard must still let F7 through — other
            // owners (dialog text fields) keep it consumed
            // (PR #299 review: egui_wants_keyboard_input is true for
            // the editor's own TextEdit, so the bare !kb_owned guard
            // dead-ended the advertised flow).
            if i.key_pressed(egui::Key::F7) && (!kb_owned || self.text_edit.is_some()) {
                self.dispatch(Action::SpellCheck);
            }
            // #130: F5 starts the Slide Show from the first page,
            // Shift+F5 from the current one (PowerPoint muscle
            // memory) — only with a document open. The in-canvas text
            // editor still lets it through (the F7 rule); dialog
            // fields keep their focus.
            if i.key_pressed(egui::Key::F5)
                && self.ed.is_some()
                && (!kb_owned || self.text_edit.is_some())
            {
                let from = if i.modifiers.shift {
                    ShowFrom::Current
                } else {
                    ShowFrom::First
                };
                self.dispatch(Action::Slideshow(from));
            }
            // Shape clipboard / duplicate / text edit (Visio keys). Not
            // while a text field owns the keyboard.
            if !kb_owned && self.sel.is_some() {
                if ctrl && i.key_pressed(egui::Key::X) {
                    self.copy_selection();
                    let (p, s) = self.sel.unwrap();
                    self.delete_one(p, s);
                    self.set_selection(None);
                }
                if ctrl && i.key_pressed(egui::Key::C) {
                    self.copy_selection();
                }
                if ctrl && i.key_pressed(egui::Key::V) {
                    if let Some((p, _)) = self.sel {
                        self.paste_clipboard(p);
                    }
                }
                if ctrl && i.key_pressed(egui::Key::D) {
                    if let Some((p, s)) = self.sel {
                        self.apply(DuplicateShape::new(p, s, (0.25, -0.25)));
                    }
                }
                if i.key_pressed(egui::Key::F2) {
                    if let Some((_, s)) = self.sel {
                        let draft = self.selected_text(s).unwrap_or_default();
                        self.begin_text_edit(s, draft);
                        self.status = format!("editing text of shape {s}");
                    }
                }
            }
        });
        // The closing Esc consumes the event (egui's InputState::consume_key
        // removes it from this frame's events) so the in-place shape-text
        // editor — which reads Esc later in the frame and abandons the
        // edit — never sees the same press. The no-dialog path stays
        // unconsumed: Esc with nothing to close still reaches the editor
        // and abandons the edit as before.
        if esc_dialog {
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
        if new_doc {
            self.do_new();
        }
        if open {
            self.do_open();
        }
        if save {
            self.do_save(false);
        }
        if save_as {
            self.do_save(true);
        }
    }

    /// #184: the keytip input step — one pass over the raw events
    /// before any widget sees them. A bare Alt arms the strip level
    /// (Office gates this to no dialog being open); while armed — or
    /// on a direct Alt+letter combo — the letter resolves against the
    /// last-painted badge set, Esc backs out one level, and a pointer
    /// click disarms. Consumed key events are removed from the input
    /// so no text field ever types them.
    pub(super) fn keytip_input(&mut self, ctx: &Context) {
        // Office shows no ribbon keytips on the File screen.
        if self.backstage {
            self.keytips.disarm();
            return;
        }
        let armed = self.keytips.armed();
        // Scan (read-only) first; act after the borrow ends. The tap
        // simulation walks the events in order: a clean Alt press
        // opens the window, any other press closes it, and the
        // release only arms when it stayed open (Office's Alt TAP —
        // Alt+Tab and other Alt combos never arm, #218 review round).
        let mut alt_tap = false;
        let mut back = false;
        let mut letter: Option<char> = None;
        let mut click = false;
        let mut clean = self.keytips.alt_down_clean;
        // The combo path (Alt held) must not steal from a text field:
        // on macOS egui reports Option as Alt, and Option+letter is
        // how accents/layout characters are typed (#218 review round).
        let kb_owned = ctx.egui_wants_keyboard_input();
        ctx.input(|i| {
            click = i.pointer.any_click();
            for e in &i.events {
                match e {
                    egui::Event::Key {
                        key: egui::Key::AltLeft | egui::Key::AltRight,
                        pressed: true,
                        modifiers: m,
                        ..
                    } if !m.ctrl && !m.shift && !m.command => {
                        clean = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => {
                        clean = false;
                        back = true;
                    }
                    egui::Event::Key {
                        key: k,
                        pressed: true,
                        modifiers: m,
                        ..
                    } => {
                        clean = false;
                        // One ASCII alphanumeric at a time — keytips
                        // are single characters by construction
                        // (`Key::name` is "A".."Z" / "0".."9"). First
                        // letter wins; the scan keeps walking so the
                        // tap state stays truthful.
                        if !m.ctrl
                            && !m.command
                            && (armed || (m.alt && !kb_owned))
                            && is_keytip_key(*k)
                            && letter.is_none()
                        {
                            letter = k.name().chars().next();
                        }
                    }
                    egui::Event::Key {
                        key: egui::Key::AltLeft | egui::Key::AltRight,
                        pressed: false,
                        ..
                    } => {
                        if clean {
                            alt_tap = true;
                        }
                        clean = false;
                    }
                    _ => {}
                }
            }
        });
        self.keytips.alt_down_clean = clean;
        if armed && click {
            self.keytips.disarm();
        }
        // The tap gate also refuses to arm while a text field owns
        // the keyboard (consistent with the app's other shortcuts).
        if alt_tap && !self.shell_surface_open() && !ctx.egui_wants_keyboard_input() {
            // A bare Alt (re)starts at the strip level, Office-style.
            self.keytips.mode = Some(crate::keytips::Mode::Strip);
        } else if back && armed {
            // Esc backs out one level; the key never reaches the
            // dialog closer below while keytips own it.
            self.keytips.mode = match self.keytips.mode {
                Some(crate::keytips::Mode::Controls) => Some(crate::keytips::Mode::Strip),
                _ => None,
            };
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                });
            });
        } else if let Some(c) = letter {
            // An Alt+letter combo arms at the strip level on the fly
            // (Office's accelerator form); armed mode resolves at the
            // current level. Resolution itself waits for paint time,
            // when this frame's badges are collected and assigned.
            if !armed {
                self.keytips.mode = Some(crate::keytips::Mode::Strip);
            }
            let c = c.to_ascii_uppercase();
            self.keytips.pending = Some(c);
            // Swallow the key AND its text twin so nothing types it
            // (only letter/digit keys match — never Escape & co.,
            // whose names also start with a letter).
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    let eats = match e {
                        egui::Event::Key {
                            key: k,
                            pressed: true,
                            ..
                        } if is_keytip_key(*k) => k.name().starts_with(c),
                        egui::Event::Text(t) => t
                            .chars()
                            .next()
                            .is_some_and(|t| t.to_ascii_uppercase() == c),
                        _ => false,
                    };
                    !eats
                });
            });
        }
    }

    /// Resolve one armed letter against the current badge set (the
    /// last-painted frame's assignments — the shell is layout-stable,
    /// so the rects and letters are fresh).
    pub(super) fn keytip_resolve(&mut self, c: char) {
        let key = c.to_string();
        // Letters are LEVEL-SCOPED (Office: the tab letters vanish at
        // control level): controls match first there, with the QAT
        // digits kept live at every level; the strip level matches
        // only its own badges. (Control letters may repeat a strip
        // letter — "Rulers" R vs the Review tab R — resolved by
        // scope, never by vec order.)
        let is_control = |b: &crate::keytips::Badge| matches!(&b.hit, crate::keytips::Hit::Action { label, .. } if !label.is_empty());
        let is_qat = |b: &crate::keytips::Badge| matches!(&b.hit, crate::keytips::Hit::Action { label, .. } if label.is_empty());
        let find = |pred: &dyn Fn(&crate::keytips::Badge) -> bool| {
            self.keytips
                .badges
                .iter()
                .find(|b| b.key == key && pred(b))
                .map(|b| b.hit.clone())
        };
        let hit = match self.keytips.mode {
            Some(crate::keytips::Mode::Controls) => find(&is_control).or_else(|| find(&is_qat)),
            _ => find(&|b| !is_control(b)),
        };
        match hit {
            Some(crate::keytips::Hit::Tab(t)) => {
                self.tab = t;
                self.keytips.mode = Some(crate::keytips::Mode::Controls);
            }
            Some(crate::keytips::Hit::File) => {
                self.open_file_backstage();
                self.keytips.disarm();
            }
            Some(crate::keytips::Hit::TellMe) => {
                self.status = crate::i18n::strs().msg_tell_me_search_arrives_with_a_la();
                self.keytips.disarm();
            }
            Some(crate::keytips::Hit::Action {
                action,
                enabled,
                label: _,
            }) => {
                if enabled {
                    self.dispatch(action);
                    self.keytips.disarm();
                } else {
                    self.status = crate::i18n::strs().msg_keytip_unavailable(key.clone());
                }
            }
            None => {
                // Office keeps the mode on an unknown letter (it
                // beeps); our equivalent is a status note.
                self.status = crate::i18n::strs().msg_no_keytip_key(key);
            }
        }
    }

    /// Whether any shell dialog/window surface is open — keytips do
    /// not arm over a dialog (Office's modal behavior). The read-only
    /// mirror of `close_front_dialog`'s list plus the surfaces that
    /// manage their own Esc (Hyperlinks, Shape Data, Comments, About).
    pub(super) fn shell_surface_open(&self) -> bool {
        self.selby_open
            || self.snap_dlg_open
            || self.zoom_dlg_open
            || self.pagesetup_open
            || self.options_open
            || self.copypic_open
            || self.contgal_open
            || self.backgrounds_open
            || self.reorder_open
            || self.color_pick.is_some()
            || !self.shapesheets.is_empty()
            || self.hyperlink_open.is_some()
            || self.shape_data_open.is_some()
            || self.comment_target.is_some()
            || self.about_open
            || self.winmenu_open
            || self.tab_menu.is_some()
            || self.tab_rename.is_some()
            || self.confirm_new
            || self.confirm_close.is_some()
            || self.text_edit.is_some()
    }

    /// The File tab's open action (#184: factored from the strip's
    /// File click so the keytip path shares it exactly).
    pub(super) fn open_file_backstage(&mut self) {
        self.backstage = true;
        // #55 landing rule (2010): Info with a document open, Recent
        // (welcome state) without one.
        self.backstage_page = if self.ed.is_some() {
            BackPage::Info
        } else {
            BackPage::Recent
        };
        // The Export pane's suggested path follows the document (#35).
        self.export_pane.reset_for(&self.path);
    }

    /// Paint the armed veil + badges after the ribbon panels close
    /// (#184). Letters are (re)assigned every frame so an Alt+letter
    /// combo always resolves against fresh assignments; the veil and
    /// chips draw only while armed.
    pub(super) fn paint_keytips(&mut self, ctx: &Context) {
        crate::keytips::assign_badge_letters(&mut self.keytips.badges);
        if let Some(c) = self.keytips.pending.take() {
            self.keytip_resolve(c);
        }
        crate::keytips::paint(ctx, &self.keytips);
    }
}
