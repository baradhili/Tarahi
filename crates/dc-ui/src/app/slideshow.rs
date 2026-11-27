//! #130 Slide Show: a read-only, page-at-a-time presentation of the
//! document's pages — full screen, one fitted page at a time,
//! keyboard advance and SubAddress hyperlink jumps (ConceptDraw's
//! presenting workflow, beyond Visio's own tool). Distinct from
//! Presentation Mode (#209), which keeps the live canvas and only
//! maximizes drawing space.

use super::*;

/// The active show (#130): the page the editor was on when it
/// started — Esc restores it.
pub(super) struct SlideShow {
    pub(super) resume: usize,
}

impl App {
    /// Enter the show (#130 entry/exit spec): the chrome closes
    /// (backstage, dialogs, any live drag or in-place edit — the
    /// shell does not paint while the show runs), the window goes
    /// full screen, and `from` picks the first slide. Requires an
    /// open document (the entry points gate on it, the guard keeps
    /// the state machine honest).
    pub(super) fn enter_slideshow(&mut self, from: ShowFrom) {
        if self.ed.is_none() {
            return;
        }
        // Chrome closes with the show — everything a frame could
        // leave open that the windowed shell owns. The dialogs
        // `close_front_dialog` covers run first; the surfaces that
        // manage their own Esc close here.
        self.backstage = false;
        while self.close_front_dialog() {}
        self.hyperlink_open = None;
        self.shape_data_open = None;
        self.comment_target = None;
        self.about_open = false;
        self.winmenu_open = false;
        self.tab_menu = None;
        self.tab_rename = None;
        self.quick_menu = None;
        self.ctx_menu = None;
        self.page_menu = None;
        // A live drag cannot survive into a mode whose canvas is
        // gone — dropping it cancels the drag without a commit.
        self.drag = None;
        self.fields_open = None;
        self.fields_caret = None;
        self.fields_draft = None;
        if self.text_edit.is_some() {
            self.end_text_edit(true);
        }
        self.slideshow = Some(SlideShow {
            resume: self.page_idx,
        });
        if matches!(from, ShowFrom::First) && self.page_idx != 0 {
            self.switch_page(0);
        }
        self.fullscreen_pending = Some(true);
    }

    /// Exit the show (#130): back to the page it was entered from,
    /// the normal windowed shell — the chrome returns and the window
    /// un-fullscreens even when the show was entered from
    /// Presentation Mode.
    pub(super) fn exit_slideshow(&mut self) {
        if let Some(show) = self.slideshow.take() {
            self.switch_page(show.resume);
        }
        self.chrome_hidden = false;
        self.fullscreen_pending = Some(false);
    }

    /// The show's key map (#130 navigation spec) — the modal tier:
    /// while the show is up it owns the keyboard ahead of keytips and
    /// app shortcuts (the Options dialog's precedence). Esc exits;
    /// →/↓/PageDown/Space advance, ←/↑/PageUp/Backspace go back,
    /// Home/End clamp to the first/last slide. No wrap at the ends.
    pub(super) fn slideshow_keys(&mut self, ctx: &Context) {
        let mut exit = false;
        let mut step: Option<isize> = None;
        let mut to: Option<usize> = None;
        ctx.input(|i| {
            // The show takes the keys bare — a modified press (Ctrl+
            // Home and friends) belongs to no slide command and is
            // ignored, not repurposed.
            let plain = !i.modifiers.ctrl
                && !i.modifiers.shift
                && !i.modifiers.alt
                && !i.modifiers.command;
            if i.key_pressed(egui::Key::Escape) {
                exit = true;
            } else if plain
                && (i.key_pressed(egui::Key::ArrowRight)
                    || i.key_pressed(egui::Key::ArrowDown)
                    || i.key_pressed(egui::Key::PageDown)
                    || i.key_pressed(egui::Key::Space))
            {
                step = Some(1);
            } else if plain
                && (i.key_pressed(egui::Key::ArrowLeft)
                    || i.key_pressed(egui::Key::ArrowUp)
                    || i.key_pressed(egui::Key::PageUp)
                    || i.key_pressed(egui::Key::Backspace))
            {
                step = Some(-1);
            } else if plain && i.key_pressed(egui::Key::Home) {
                to = Some(0);
            } else if plain && i.key_pressed(egui::Key::End) {
                to = Some(usize::MAX);
            }
        });
        if exit {
            self.exit_slideshow();
            // Consume the Esc so nothing behind the show reacts to
            // the same press (the dialog-closer pattern in
            // `handle_shortcuts`).
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
            return;
        }
        if let Some(d) = step {
            self.slideshow_step(d);
        }
        if let Some(t) = to {
            self.slideshow_go_to(t);
        }
    }

    /// One slide step, clamped at the ends (#130 — no wrap).
    fn slideshow_step(&mut self, delta: isize) {
        let n = self.ed().map(|ed| ed.doc.pages.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        // switch_page no-ops at the current index, so a step past the
        // last (or first) slide simply stays.
        let next = (self.page_idx as isize + delta).clamp(0, n as isize - 1) as usize;
        self.switch_page(next);
    }

    /// Jump to a slide (Home/End clamp to the deck's ends).
    fn slideshow_go_to(&mut self, to: usize) {
        let n = self.ed().map(|ed| ed.doc.pages.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        self.switch_page(to.min(n - 1));
    }

    /// Paint the show over the whole screen (#130 display spec):
    /// black letterbox, the page's background chain (deepest first,
    /// the same underlay order the canvas composes) and the page
    /// itself, each through the TextureCache page path at the show's
    /// fit ppi. A raster still in flight paints the white paper
    /// placeholder (deterministic under the sync-raster test seam).
    /// A click jumps the hit shape's first SubAddress hyperlink
    /// (GOTOPAGE); any other click advances.
    pub(super) fn slideshow_view(&mut self, ui: &mut Ui) {
        // The show cannot outlive its document (a close approved
        // behind a confirm mid-show) — drop it without the PAGE
        // restore (the document is gone) but with the windowed shell
        // back: the exit rule is unconditional, and nothing else
        // would send the un-fullscreen (Esc no longer runs the
        // show's exit path).
        if self.ed.is_none() {
            self.slideshow = None;
            self.chrome_hidden = false;
            self.fullscreen_pending = Some(false);
            return;
        }
        let screen = ui.max_rect();
        ui.painter().rect_filled(screen, 0.0, Color32::BLACK);
        let geom = self.page_geom();
        // Aspect-fit: the largest centered rect that fits the screen,
        // aspect preserved — the ppi of the fitted page drives the
        // raster width (the cache's bucket quantizes it).
        let scale =
            (screen.width() as f64 / geom.w_in).min(screen.height() as f64 / geom.h_in);
        let size = egui::vec2((geom.w_in * scale) as f32, (geom.h_in * scale) as f32);
        let rect = Rect::from_center_size(screen.center(), size);
        let ppi = (rect.width() / geom.w_in as f32).max(1.0);
        // The sheet's white paper — the permanent backdrop the page
        // layers compose over (they render INK ONLY, transparent, so
        // background ink shows beneath the foreground page; review
        // round 1) and the placeholder while a raster is in flight
        // (the canvas `show` None path's rule).
        ui.painter().rect_filled(rect, 0.0, Color32::WHITE);

        // The background chain, deepest first — the #113 walk the
        // canvas underlays use (cycle-guarded, depth-capped).
        let mut chain: Vec<i64> = Vec::new();
        let mut cur = self.page_id().and_then(|id| self.back_page_of(id));
        while let Some(id) = cur {
            if chain.contains(&id) || chain.len() >= 8 {
                break;
            }
            chain.push(id);
            cur = self.ed.as_ref().and_then(|ed| {
                let p = ed.doc.pages.iter().find(|p| p.id == id)?;
                p.attrs
                    .iter()
                    .find(|(k, _)| k == "BackPage")
                    .and_then(|(_, v)| v.parse::<i64>().ok())
            });
        }
        for id in chain.iter().rev() {
            let Some(bg) = self.ed.as_ref().and_then(|ed| {
                ed.doc
                    .pages
                    .iter()
                    .find(|p| p.id == *id)
            }) else {
                continue;
            };
            let (bw, bh) = (
                bg.sheet()
                    .and_then(|s| s.cell("PageWidth"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(8.5),
                bg.sheet()
                    .and_then(|s| s.cell("PageHeight"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(11.0),
            );
            // Natural size, origin at the sheet's BOTTOM-LEFT — the
            // canvas underlay mapping (identical pixels when the
            // pages share size, the norm; an oversized background
            // extends up and right past the sheet, clipped to it).
            let brect = Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - bh as f32 * ppi),
                egui::vec2(bw as f32 * ppi, bh as f32 * ppi),
            );
            let (part, pid) = (bg.part.clone(), bg.id);
            self.slideshow_page_texture(ui, &part, Some(pid), bw, ppi, brect, rect);
        }
        let part = self.ed.as_ref().map(|ed| {
            ed.doc.pages[self
                .page_idx
                .min(ed.doc.pages.len().saturating_sub(1))]
            .part
            .clone()
        });
        if let Some(part) = part {
            self.slideshow_page_texture(ui, &part, None, geom.w_in, ppi, rect, rect);
        }

        // Input: one click surface over the whole screen. A hit on a
        // shape whose first hyperlink row carries a SubAddress jumps
        // to that page (GOTOPAGE — a non-empty SubAddress never
        // spawns the OS); an address-only first row is inert (no
        // navigation, no advance); every other click — no shape, or a
        // shape with no links — advances.
        let resp = ui.allocate_rect(screen, Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                crate::i18n::strs().msg_slide_show(),
            )
        });
        if resp.clicked() {
            // Screen → page inches, y-up — the show's own transform
            // (no rulers, no content-origin offset).
            let hit = resp.interact_pointer_pos().and_then(|p| {
                let x_in = (p.x - rect.left()) as f64 / ppi as f64;
                let y_in = (rect.bottom() - p.y) as f64 / ppi as f64;
                let sid = self.shape_hit((x_in, y_in))?;
                let (addr, sub, _) = self.shape_hyperlinks(sid).into_iter().next()?;
                Some((addr, sub))
            });
            match hit {
                // A first row that carries a SubAddress: the GOTOPAGE
                // jump. An address-only first row: inert — nothing
                // happens (no OS spawn mid-presentation, no advance).
                Some((addr, sub)) if !sub.trim().is_empty() => {
                    self.follow_hyperlink(&addr, &sub);
                }
                Some(_) => {}
                None => self.slideshow_step(1),
            }
        }
    }

    /// One page raster through the TextureCache page path (#130):
    /// the export render in its TRANSPARENT flavor (chrome-free,
    /// ink-only — the sheet's white paper is painted once beneath
    /// the layers so background ink composes under the foreground
    /// page; review round 1) keyed `show-<part>` at the show's ppi,
    /// painted into `r` clipped to the fitted page `clip` (an
    /// oversized background page must not ink the letterbox).
    /// `page_id` names the page to render (None = the current one —
    /// the background underlays name their own).
    /// (8 placement args, like `TextureCache::get`'s raster contract.)
    #[allow(clippy::too_many_arguments)]
    fn slideshow_page_texture(
        &mut self,
        ui: &mut Ui,
        part: &str,
        page_id: Option<i64>,
        w_in: f64,
        ppi: f32,
        r: Rect,
        clip: Rect,
    ) {
        let key = format!("show-{}", part.replace('/', "_"));
        let version = self.version;
        let page_idx = self.page_idx;
        let got = self.tex.get(
            ui.ctx(),
            &mut self.pool,
            w_in,
            &key,
            version,
            ppi,
            || {
                let ed = self.ed.as_ref().unwrap();
                let page = match page_id {
                    Some(id) => ed
                        .doc
                        .pages
                        .iter()
                        .find(|p| p.id == id)
                        .unwrap_or(&ed.doc.pages
                            [page_idx.min(ed.doc.pages.len().saturating_sub(1))]),
                    None => &ed.doc.pages
                        [page_idx.min(ed.doc.pages.len().saturating_sub(1))],
                };
                dc::svg::render_page_export(&ed.doc, page, true)
            },
        );
        if let Ok(Some(tex)) = got {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter()
                .with_clip_rect(clip)
                .image(tex.id(), r, uv, Color32::WHITE);
        }
    }
}
