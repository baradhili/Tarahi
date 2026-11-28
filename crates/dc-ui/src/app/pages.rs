//! Pages: orientation and size, background pages, page setup, and
//! the page-tab context menu.

use super::*;

impl App {
    /// #113 (URS-1122): swap the page's width/height — portrait ↔
    /// landscape, one undoable step, then refit the view.
    pub(super) fn toggle_page_orientation(&mut self) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sheet) = page.sheet() else { return };
        let get = |n: &str| sheet.cell(n).and_then(|c| c.numeric());
        let Some((w, h)) = get("PageWidth").zip(get("PageHeight")) else {
            return;
        };
        self.apply_page_size((h, w));
    }
}

impl App {
    pub(super) fn apply_page_size(&mut self, (w, h): (f64, f64)) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let scope = Scope::PageSheet { page: page_id };
        let cmds: Vec<Box<dyn dc_core::Command>> = vec![
            Box::new(SetCell::new(
                scope.clone(),
                "PageWidth",
                vsdx::Cell::value(format!("{w:.6}")),
            )),
            Box::new(SetCell::new(
                scope,
                "PageHeight",
                vsdx::Cell::value(format!("{h:.6}")),
            )),
        ];
        self.apply(Macro::new(crate::i18n::strs().msg_set_page_size(), cmds));
        self.view.fit_pending = true;
        self.layers.set_page(-1); // force re-key on the new geometry
        self.status =
            crate::i18n::strs().msg_page_size_set(self.view.unit.fmt(w), self.view.unit.fmt(h));
    }
}

impl App {
    /// #113 (URS-1158): the page's background assignment — the
    /// `BackPage` pages-index attribute names a page ID; `Background`
    /// marks the page itself as one. Corpus: `BackPage='4'` /
    /// `Background='1'` on the `<Page>` entries.
    pub(super) fn back_page_of(&self, page_id: i64) -> Option<i64> {
        let ed = self.ed.as_ref()?;
        let p = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        p.attrs
            .iter()
            .find(|(k, _)| k == "BackPage")
            .and_then(|(_, v)| v.parse::<i64>().ok())
    }

    pub(super) fn is_background_page(&self, page_id: i64) -> bool {
        let Some(ed) = self.ed.as_ref() else {
            return false;
        };
        ed.doc
            .pages
            .iter()
            .find(|p| p.id == page_id)
            .is_some_and(|p| p.attrs.iter().any(|(k, v)| k == "Background" && v == "1"))
    }

    /// Assign/clear the current page's background (direct model
    /// write — a page-index flag, like Visio's non-undoable page
    /// setup properties).
    pub(super) fn set_back_page(&mut self, bg: Option<i64>) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(page) = ed.doc.pages.iter_mut().find(|p| p.id == page_id) else {
            return;
        };
        page.attrs.retain(|(k, _)| k != "BackPage");
        if let Some(bg) = bg {
            page.attrs.push(("BackPage".to_string(), bg.to_string()));
        }
        self.dirty = true;
        self.version += 1;
        self.status = match bg {
            Some(id) => {
                let name = ed
                    .doc
                    .pages
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| id.to_string());
                crate::i18n::strs().msg_background_page_name(name)
            }
            None => crate::i18n::strs().msg_background_cleared(),
        };
    }

    /// The Backgrounds dialog (#113, URS-1158): pick which page
    /// renders behind the current one (chain: a background with its
    /// own background stacks too), or none.
    pub(super) fn backgrounds_window(&mut self, ctx: &egui::Context) {
        if !self.backgrounds_open {
            return;
        }
        let mut open = true;
        let mut pick: Option<Option<i64>> = None; // Some(None) = clear
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        // Visio's gallery lists BACKGROUND pages (the #113 mark makes
        // one); a plain page never shows as a background choice.
        let entries: Vec<(i64, String)> = ed
            .doc
            .pages
            .iter()
            .filter(|p| p.id != page_id && self.is_background_page(p.id))
            .map(|p| (p.id, p.name.clone()))
            .collect();
        let current = self.back_page_of(page_id);
        let this_is_bg = self.is_background_page(page_id);
        egui::Window::new(crate::i18n::strs().msg_backgrounds())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors every window
                // (the Language-dialog exemplar — content right-aligns
                // while RTL).
                ui.with_layout(dialog_dir(), |ui| {
                    ui.label(crate::i18n::strs().msg_this_page_renders_behind());
                    if this_is_bg {
                        ui.weak(crate::i18n::strs().msg_this_page_is_itself_a_background());
                    }
                    ui.separator();
                    if ui
                        .radio(current.is_none(), crate::i18n::strs().msg_no_background())
                        .clicked()
                    {
                        pick = Some(None);
                    }
                    if entries.is_empty() {
                        ui.weak(crate::i18n::strs().msg_no_background_pages_right_click());
                    }
                    for (id, name) in &entries {
                        if ui
                            .radio(
                                current == Some(*id),
                                crate::i18n::strs().msg_name_page_id(name, id),
                            )
                            .clicked()
                        {
                            pick = Some(Some(*id));
                        }
                    }
                });
            });
        if let Some(bg) = pick {
            self.set_back_page(bg);
        }
        if !open {
            self.backgrounds_open = false;
        }
    }

    /// #113 (URS-1158): the background chain's underlay quads —
    /// deepest background first, each rasterized on demand at the
    /// current zoom (cache keyed page/version/ppi-bucket). #414: the
    /// raster density folds the device scale factor, and the chain
    /// shares the JOINT texture budget with the foreground layers
    /// (PR #415 review) — degrading against what the store holds and
    /// reserving what it retains in return.
    pub(super) fn build_bg_underlays(
        &mut self,
        ctx: &egui::Context,
    ) -> Vec<crate::layers::ShapeQuad> {
        let Some(page_id) = self.page_id() else {
            return Vec::new();
        };
        // Walk the chain (cycle-guarded, depth cap).
        let mut chain: Vec<i64> = Vec::new();
        let mut cur = self.back_page_of(page_id);
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
        // #414: underlays are canvas layers — the raster density (and
        // its bucket key) folds the device scale factor like the layer
        // store's, so background-page ink stays as sharp as the ink
        // above it on a HiDPI display.
        let mut raster_ppi = self.view.raster_px_per_in(ctx.pixels_per_point());
        // PR #415 review: the chain shares the JOINT texture budget
        // with the foreground layers. The old 2048 texture cap
        // incidentally bounded eight Letter backgrounds at 128 MiB;
        // under the device-derived ceiling, zoom 8 on a 2× display
        // would otherwise retain up to 8 × 8192² RGBA ≈ 2 GiB with no
        // bound at all. The chain's page rects degrade against what is
        // LEFT of the budget after the layer store's live bytes (soft
        // before clipped, never content loss), and the store reserves
        // the chain's live bytes in return, so neither path can spend
        // the budget independently.
        let rects: Vec<(i64, crate::layers::Rect)> = chain
            .iter()
            .filter_map(|id| {
                let bg = self
                    .ed
                    .as_ref()?
                    .doc
                    .pages
                    .iter()
                    .find(|p| p.id == *id)?;
                let w = bg
                    .sheet()
                    .and_then(|s| s.cell("PageWidth"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(8.5);
                let h = bg
                    .sheet()
                    .and_then(|s| s.cell("PageHeight"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(11.0);
                Some((0, (0.0, 0.0, w, h)))
            })
            .collect();
        raster_ppi = underlay_ppi(raster_ppi, &rects, self.layers.texture_bytes());
        let bucket = (raster_ppi * 4.0).round().max(4.0) as u32;
        let version = self.layer_base_version();
        // Drop stale entries — page switch, version bump, zoom bucket
        // (CodeRabbit: keeping version-stale handles leaked a texture
        // per edit while a background was assigned).
        self.bg_underlays
            .retain(|(p, v, b, _)| chain.contains(p) && *b == bucket && *v == version);
        let mut out: Vec<crate::layers::ShapeQuad> = Vec::new();
        for id in chain.iter().rev() {
            let Some(ed) = self.ed.as_ref() else { break };
            let Some(bg) = ed.doc.pages.iter().find(|p| p.id == *id) else {
                continue;
            };
            let version = self.layer_base_version();
            let (w, h) = (
                bg.sheet()
                    .and_then(|s| s.cell("PageWidth"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(8.5),
                bg.sheet()
                    .and_then(|s| s.cell("PageHeight"))
                    .and_then(|c| c.numeric())
                    .unwrap_or(11.0),
            );
            if let Some(hh) = self
                .bg_underlays
                .iter()
                .find(|(p, v, b, _)| *p == *id && *v == version && *b == bucket)
                .map(|(_, _, _, hh)| hh)
            {
                out.push(crate::layers::ShapeQuad {
                    id: -1000 - id,
                    tex: hh.id(),
                    bbox: (0.0, 0.0, w, h),
                });
                continue;
            }
            // Rasterize this background page's own content.
            let Ok(svg) = dc::svg::render_page(&ed.doc, bg) else {
                continue;
            };
            let Ok(tree) = crate::svgimg::parse_svg(&svg, crate::perf::Kind::Thumb) else {
                continue;
            };
            let Ok(img) = crate::svgimg::render_tree(
                &tree,
                w as f32 * raster_ppi,
                crate::perf::Kind::Thumb,
            ) else {
                continue;
            };
            let tex = ctx.load_texture(format!("dc-bg-{id}"), img, egui::TextureOptions::default());
            out.push(crate::layers::ShapeQuad {
                id: -1000 - id,
                tex: tex.id(),
                bbox: (0.0, 0.0, w, h),
            });
            self.bg_underlays.push((*id, version, bucket, tex));
        }
        // Reserve the chain's live bytes (the retained handles' own
        // sizes — the landed truth, degraded or not) against the
        // store's budget for the next frame's composite. Recomputed
        // every build pass, and composite only ever runs in frames
        // whose canvas pass also ran this builder, so the value is
        // always fresh where it is read.
        let held: u64 = self
            .bg_underlays
            .iter()
            .map(|(_, _, _, t)| {
                let [w, h] = t.size();
                (w * h * 4) as u64
            })
            .sum();
        self.layers.reserve_budget(held);
        out
    }

    /// The layer store's base version — the underlay invalidation key.
    pub(super) fn layer_base_version(&self) -> u64 {
        self.version
    }
}

impl App {
    /// The Page Setup dialog (#113, URS-1122): ISO/ANSI presets plus
    /// custom dimensions (display units) and orientation.
    pub(super) fn page_setup_window(&mut self, ctx: &egui::Context) {
        if !self.pagesetup_open {
            return;
        }
        /// (name, portrait w, h) in inches — Visio's Printer paper
        /// preset table, metric and ANSI families.
        const SIZES: &[(&str, f64, f64)] = &[
            ("A5", 5.83, 8.27),
            ("A4", 8.27, 11.69),
            ("A3", 11.69, 16.54),
            ("A2", 16.54, 23.39),
            ("A1", 23.39, 33.11),
            ("A0", 33.11, 46.81),
            ("Letter (ANSI A)", 8.5, 11.0),
            ("Legal", 8.5, 14.0),
            ("Tabloid (ANSI B)", 11.0, 17.0),
            ("ANSI C", 17.0, 22.0),
            ("ANSI D", 22.0, 34.0),
            ("ANSI E", 34.0, 44.0),
        ];
        let unit = self.view.unit;
        let geom = self.page_geom();
        // Current dims in display units.
        let (cw, ch) = (unit.from_in(geom.w_in), unit.from_in(geom.h_in));
        let mut open = true;
        let mut apply: Option<(f64, f64)> = None; // portrait pair, inches
        let mut custom: Option<(f64, f64)> = None;
        egui::Window::new(crate::i18n::strs().msg_page_size())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(300.0)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window.
                let mut __rtl_body = |ui: &mut Ui| {
                    ui.label(crate::i18n::strs().msg_current());
                    ui.label(format!(
                        "{:.1} × {:.1} {}",
                        cw.max(ch),
                        cw.min(ch),
                        unit.suffix()
                    ));
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_preset_sizes());
                    let mut picked: Option<(f64, f64)> = None;
                    egui::Grid::new("dc-sizes")
                        .num_columns(3)
                        .spacing(egui::vec2(8.0, 4.0))
                        .show(ui, |ui| {
                            // #257: an RTL grid fills its rows right-to-left.
                            let rows: Vec<Vec<(&str, f64, f64)>> = if crate::i18n::rtl() {
                                SIZES
                                    .chunks(3)
                                    .map(|c| {
                                        let mut v: Vec<_> = c.to_vec();
                                        v.reverse();
                                        v
                                    })
                                    .collect()
                            } else {
                                SIZES.chunks(3).map(|c| c.to_vec()).collect()
                            };
                            for chunk in rows {
                                for (name, w, h) in chunk {
                                    if ui.button(name).clicked() {
                                        picked = Some((w, h));
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    if let Some(p) = picked {
                        apply = Some(p);
                    }
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_custom());
                    let mut wd = cw;
                    let mut ht = ch;
                    // The range is ~10 mm .. 2 m, expressed in display
                    // units so inches never clamps at 10 in.
                    let (lo, hi) = (unit.from_in(10.0 / 25.4), unit.from_in(2000.0 / 25.4));
                    // #257: mirrored emission under RTL — ui::horizontal
                    // keeps the wrap-aware measure (a with_layout flip here
                    // reserves the parent's full cross space and balloons
                    // the window).
                    let wd_label = crate::i18n::strs().msg_width();
                    let ht_label = crate::i18n::strs().msg_height();
                    let width_first = !crate::i18n::rtl();
                    ui.horizontal(|ui| {
                        let (first_label, second_label) = if width_first {
                            (wd_label.as_str(), ht_label.as_str())
                        } else {
                            (ht_label.as_str(), wd_label.as_str())
                        };
                        let mut first_val = if width_first { wd } else { ht };
                        let mut second_val = if width_first { ht } else { wd };
                        ui.label(first_label);
                        ui.add(
                            egui::DragValue::new(&mut first_val)
                                .range(lo..=hi)
                                .suffix(unit.suffix()),
                        );
                        ui.label(second_label);
                        ui.add(
                            egui::DragValue::new(&mut second_val)
                                .range(lo..=hi)
                                .suffix(unit.suffix()),
                        );
                        // The RTL dialog swaps the FIELDS (height first);
                        // the write-back must swap with them — the fixed
                        // order traded a Persian page's W and H on every
                        // Apply (PR #269 review).
                        wd = if width_first { first_val } else { second_val };
                        ht = if width_first { second_val } else { first_val };
                    });
                    if ui
                        .button(crate::i18n::strs().msg_apply_custom_size())
                        .clicked()
                        && wd > 0.0
                        && ht > 0.0
                    {
                        custom = Some((unit.to_in(wd), unit.to_in(ht)));
                    }
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_orientation());
                    // #257: mirrored emission under RTL (horizontal keeps
                    // the wrap-aware measure).
                    let rtl_ori = crate::i18n::rtl();
                    let (mut p_clicked, mut l_clicked) = (false, false);
                    ui.horizontal(|ui| {
                        if rtl_ori {
                            if ui
                                .selectable_label(
                                    geom.w_in > geom.h_in,
                                    crate::i18n::strs().msg_landscape(),
                                )
                                .clicked()
                                && geom.w_in <= geom.h_in
                            {
                                l_clicked = true;
                            }
                            if ui
                                .selectable_label(
                                    geom.w_in <= geom.h_in,
                                    crate::i18n::strs().msg_portrait(),
                                )
                                .clicked()
                                && geom.w_in > geom.h_in
                            {
                                p_clicked = true;
                            }
                        } else {
                            if ui
                                .selectable_label(
                                    geom.w_in <= geom.h_in,
                                    crate::i18n::strs().msg_portrait(),
                                )
                                .clicked()
                                && geom.w_in > geom.h_in
                            {
                                p_clicked = true;
                            }
                            if ui
                                .selectable_label(
                                    geom.w_in > geom.h_in,
                                    crate::i18n::strs().msg_landscape(),
                                )
                                .clicked()
                                && geom.w_in <= geom.h_in
                            {
                                l_clicked = true;
                            }
                        }
                    });
                    if p_clicked {
                        apply = Some((geom.w_in.min(geom.h_in), geom.w_in.max(geom.h_in)));
                    }
                    if l_clicked {
                        apply = Some((geom.h_in.max(geom.w_in), geom.h_in.min(geom.w_in)));
                    }
                    // Layout and Routing (#251, grown by #324 phase 5
                    // toward Visio's Page Setup ▸ Layout and Routing
                    // tab): the Line jumps group — which lines jump,
                    // the jump style, and the horizontal/vertical size
                    // sliders — all persisted on the PageSheet through
                    // one undo step each, with the stored-hop reconcile
                    // riding the same macro (normalize-on-edit, #301).
                    // The Routing group paints Visio's controls
                    // DISABLED — routing proper is #225's deferred
                    // bulk ("follow direction of paint, but disabled").
                    let (doc_code, doc_style, doc_factor_x, doc_factor_y) = self
                        .ed
                        .as_ref()
                        .map(|ed| {
                            let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len() - 1)];
                            // The RESOLVED cells — pages usually carry
                            // no inline jump cells and ride the style
                            // chain (#294).
                            let code = ed
                                .doc
                                .resolve_page_layout_cell(page, "LineJumpCode")
                                .and_then(|c| c.numeric())
                                .map_or(1, |v| v.round() as i64);
                            let style_code = match crate::connectors::page_jump_style(&ed.doc, page)
                            {
                                crate::connectors::JumpStyle::Arc => 1,
                                crate::connectors::JumpStyle::Gap => 2,
                                crate::connectors::JumpStyle::Square => 3,
                                crate::connectors::JumpStyle::Sides(n) => n as i64 + 2,
                            };
                            (
                                code,
                                style_code,
                                ed.doc.page_line_jump_factor(page),
                                ed.doc.page_line_jump_factor_y(page),
                            )
                        })
                        .unwrap_or((1, 1, 2.0 / 3.0, 2.0 / 3.0));
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_line_jumps());
                    let s = crate::i18n::strs();
                    // "Add line jumps to:" — Visio's five-entry list
                    // (the screenshots), LineJumpCode 0–4. An
                    // unwitnessed other value shows its number.
                    let code_key = doc_code.to_string();
                    let code_choices: [(&'static str, String); 5] = [
                        ("0", s.msg_lj_code_none()),
                        ("1", s.msg_lj_code_horizontal()),
                        ("2", s.msg_lj_code_vertical()),
                        ("3", s.msg_lj_code_last_routed()),
                        ("4", s.msg_lj_code_last_displayed()),
                    ];
                    let code_label = s.msg_add_line_jumps_to();
                    ui.horizontal(|ui| {
                        if !crate::i18n::rtl() {
                            ui.label(&code_label);
                        }
                        if let Some(k) =
                            Self::fmt_dropdown(ui, &code_key, &|k| k.to_string(), &code_choices)
                        {
                            if let Ok(code) = k.parse::<i64>() {
                                self.apply_line_jump_code(code);
                            }
                        }
                        if crate::i18n::rtl() {
                            ui.label(&code_label);
                        }
                    });
                    // "Line jump style:" — Arc/Gap/Square/2–7 Sides,
                    // LineJumpStyle 1–9 (0 and 1 both draw Arc).
                    let style_choices: [(&'static str, String); 9] = [
                        ("1", s.msg_lj_style_arc()),
                        ("2", s.msg_lj_style_gap()),
                        ("3", s.msg_lj_style_square()),
                        ("4", s.msg_lj_style_2_sides()),
                        ("5", s.msg_lj_style_3_sides()),
                        ("6", s.msg_lj_style_4_sides()),
                        ("7", s.msg_lj_style_5_sides()),
                        ("8", s.msg_lj_style_6_sides()),
                        ("9", s.msg_lj_style_7_sides()),
                    ];
                    let style_label = s.msg_line_jump_style();
                    ui.horizontal(|ui| {
                        if !crate::i18n::rtl() {
                            ui.label(&style_label);
                        }
                        if let Some(k) = Self::fmt_dropdown(
                            ui,
                            &doc_style.to_string(),
                            &|k| k.to_string(),
                            &style_choices,
                        ) {
                            if let Ok(code) = k.parse::<i64>() {
                                let name = style_choices
                                    .iter()
                                    .find(|(kk, _)| *kk == k)
                                    .map(|(_, d)| d.clone())
                                    .unwrap_or_default();
                                self.apply_line_jump_style(code, name);
                            }
                        }
                        if crate::i18n::rtl() {
                            ui.label(&style_label);
                        }
                    });
                    // The horizontal/vertical size sliders — the
                    // LineJumpFactorX/Y multipliers (the screenshots'
                    // twin 0.6667 sliders).
                    let mut jump_factor_x = self.line_jump_draft.unwrap_or(doc_factor_x as f32);
                    let mut jump_factor_y = self.line_jump_draft_y.unwrap_or(doc_factor_y as f32);
                    let x_label = s.msg_horizontal_size();
                    let y_label = s.msg_vertical_size();
                    // One row per factor (a local macro keeps the two
                    // identical rows in step without fighting the
                    // double-`self` borrow a shared closure would take).
                    macro_rules! factor_row {
                        ($ui:expr, $label:expr, $value:expr, $doc:expr, $draft:ident, $apply:ident) => {{
                            $ui.label(&$label);
                            let r = $ui.add(
                                egui::DragValue::new(&mut $value)
                                    .range(0.5..=3.0)
                                    .speed(0.05)
                                    .suffix("×"),
                            );
                            // Draft while dragging; commit ONCE at drag
                            // end (or immediately for typed input) — the
                            // value reloads from the document each
                            // frame, so the draft carries the drag
                            // (#218 review).
                            if r.dragged() {
                                self.$draft = Some($value);
                            }
                            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                self.$draft = None;
                                if ($value - $doc as f32).abs() > 1e-6 {
                                    self.$apply($value as f64);
                                }
                            }
                        }};
                    }
                    ui.vertical(|ui| {
                        factor_row!(
                            ui,
                            x_label,
                            jump_factor_x,
                            doc_factor_x,
                            line_jump_draft,
                            apply_line_jump_factor
                        );
                        factor_row!(
                            ui,
                            y_label,
                            jump_factor_y,
                            doc_factor_y,
                            line_jump_draft_y,
                            apply_line_jump_factor_y
                        );
                    });
                    // The Routing group: Visio's five controls, present
                    // but disabled (this issue's body — routing proper
                    // is #225's deferred bulk).
                    ui.separator();
                    ui.strong(crate::i18n::strs().msg_routing_group());
                    let disabled_note = crate::i18n::strs().msg_routing_deferred();
                    for (label, value) in [
                        (
                            crate::i18n::strs().msg_routing_style(),
                            crate::i18n::strs().msg_routing_right_angle(),
                        ),
                        (
                            crate::i18n::strs().msg_routing_direction(),
                            String::new(),
                        ),
                        (
                            crate::i18n::strs().msg_routing_separate(),
                            crate::i18n::strs().msg_routing_unrelated_lines(),
                        ),
                        (
                            crate::i18n::strs().msg_routing_overlap(),
                            crate::i18n::strs().msg_routing_using_style(),
                        ),
                        (
                            crate::i18n::strs().msg_routing_appearance(),
                            crate::i18n::strs().msg_routing_straight(),
                        ),
                    ] {
                        ui.horizontal(|ui| {
                            if !crate::i18n::rtl() {
                                ui.label(&label);
                            }
                            ui.add_enabled(false, egui::Button::new(format!("{value}  ›")))
                                .on_disabled_hover_text(&disabled_note);
                            if crate::i18n::rtl() {
                                ui.label(&label);
                            }
                        });
                    }
                    ui.label(
                        egui::RichText::new(crate::i18n::strs().msg_routing_deferred()).small(),
                    );
                };
                if crate::i18n::rtl() {
                    ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                        __rtl_body(ui)
                    });
                } else {
                    __rtl_body(ui);
                }
            });
        if let Some((w, h)) = apply.or(custom) {
            self.apply_page_size((w, h));
        }
        if !open {
            self.pagesetup_open = false;
        }
    }
}

impl App {
    /// The page-tab context menu (#54): Visio's tab affordances —
    /// insert, duplicate, delete (never the last page), rename, and
    /// The page-tab context menu (#54), copying the shipped Visio
    /// reference (owner screenshots 2026-10-24): Insert, Delete, Rename,
    /// Duplicate, then Page Setup (later milestone) and Reorder Pages….
    /// All live items run through the undoable command layer.
    /// #272: the empty-page context menu. Visio documents the route
    /// to a page's sheet as "make sure nothing is selected, right-click
    /// the page, and click Show ShapeSheet".
    pub(super) fn page_context_menu(&mut self, ctx: &egui::Context) {
        let Some(anchor) = self.page_menu else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.page_menu = None;
            return;
        }
        let mut show = false;
        let menu = egui::Popup::new(
            egui::Id::unique("dc-page-ctx-menu"),
            ctx.clone(),
            anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(232.0);
        let popup = menu.show(|ui| {
            ui.set_min_width(232.0);
            let tokens = theme::menu_tokens(ui.visuals());
            ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = tokens.hover_fill;
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::ShapeSheet),
                &crate::i18n::strs().msg_show_shapesheet(),
                "",
                true,
            ) {
                show = true;
            }
        });
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.page_menu = None;
            }
        }
        if show {
            self.page_menu = None;
            if let Some(page_id) = self.page_id() {
                let target = crate::shapesheet::SheetTarget::Page(page_id);
                if !self.shapesheets.contains(&target) {
                    self.shapesheets.push(target);
                }
            }
        }
    }

    pub(super) fn tab_context_menu(&mut self, ctx: &egui::Context) {
        let Some((idx, anchor)) = self.tab_menu else {
            return;
        };
        let n = self.ed().map(|e| e.doc.pages.len()).unwrap_or(0);
        if idx >= n {
            self.tab_menu = None;
            return;
        }
        #[derive(Clone, Copy, PartialEq)]
        enum Act {
            Insert,
            Delete,
            Rename,
            Duplicate,
            MarkBg,
            Reorder,
        }
        let mut act: Option<Act> = None;
        let menu = egui::Popup::new(
            egui::Id::unique("dc-tab-ctx-menu"),
            ctx.clone(),
            anchor,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(232.0);
        let popup = menu.show(|ui| {
            ui.set_min_width(232.0);
            let tokens = theme::menu_tokens(ui.visuals());
            ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = tokens.hover_fill;
            ui.visuals_mut().widgets.noninteractive.bg_stroke.color = tokens.separator;
            if ctx_item(ui, None, &crate::i18n::strs().msg_insert_page(), "", true) {
                act = Some(Act::Insert);
            }
            if ctx_item(
                ui,
                Some(crate::ribbon::Icon::Delete),
                &crate::i18n::strs().msg_delete_page(),
                "",
                n > 1,
            ) {
                act = Some(Act::Delete);
            }
            if ctx_item(ui, None, &crate::i18n::strs().msg_rename_page(), "", true) {
                act = Some(Act::Rename);
            }
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_duplicate_page(),
                "",
                true,
            ) {
                act = Some(Act::Duplicate);
            }
            // #113 remainder: mark a page AS a background (Visio's Page
            // Setup ▸ Page Properties ▸ Type: Background, surfaced on
            // the tab the way Visio surfaces page commands).
            let marked = self
                .ed()
                .and_then(|e| e.doc.pages.get(idx))
                .is_some_and(|p| self.is_background_page(p.id));
            let bg_label = if marked {
                crate::i18n::strs().msg_remove_background_mark()
            } else {
                crate::i18n::strs().msg_set_as_background()
            };
            if ctx_item(ui, None, &bg_label, "", true) {
                act = Some(Act::MarkBg);
            }
            ui.separator();
            ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_page_setup_menu(),
                "(later milestone)",
                false,
            );
            if ctx_item(
                ui,
                None,
                &crate::i18n::strs().msg_reorder_pages_menu(),
                "",
                true,
            ) {
                act = Some(Act::Reorder);
            }
        });
        // Close on an outside primary click (the shape menu's idiom).
        let outside_primary = ctx.input(|i| i.pointer.primary_clicked());
        if let Some(ir) = &popup {
            if outside_primary
                && !ir
                    .response
                    .rect
                    .contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default())
            {
                self.tab_menu = None;
            }
        }
        let Some(a) = act else { return };
        self.tab_menu = None;
        match a {
            Act::Insert => self.tab_cmd_insert(idx),
            Act::Delete => self.tab_cmd_delete(idx),
            Act::Rename => {
                let id = self.ed().and_then(|e| e.doc.pages.get(idx)).map(|p| p.id);
                let name = self
                    .ed()
                    .and_then(|e| e.doc.pages.get(idx))
                    .map(|p| p.name.clone());
                if let (Some(id), Some(name)) = (id, name) {
                    self.tab_rename = Some((id, name));
                }
            }
            Act::Duplicate => self.tab_cmd_duplicate(idx),
            Act::MarkBg => self.toggle_background_mark(idx),
            Act::Reorder => self.reorder_open = true,
        }
    }

    /// #113 remainder: the page-tab menu's background mark — the
    /// `Background="1"` page attribute (the same attribute the
    /// underlay chain and the Backgrounds gallery read), assigned the
    /// same direct-mutation way `set_back_page` works.
    pub(super) fn toggle_background_mark(&mut self, idx: usize) {
        let Some(page_id) = self.ed().and_then(|e| e.doc.pages.get(idx)).map(|p| p.id) else {
            return;
        };
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(page) = ed.doc.pages.iter_mut().find(|p| p.id == page_id) else {
            return;
        };
        let marked = page
            .attrs
            .iter()
            .any(|(k, v)| k == "Background" && v == "1");
        page.attrs.retain(|(k, _)| k != "Background");
        let name = page.name.clone();
        if !marked {
            page.attrs.push(("Background".to_string(), "1".to_string()));
        }
        self.dirty = true;
        self.version += 1;
        self.status = if marked {
            crate::i18n::strs().msg_page_name_is_a_foreground_page_a(name)
        } else {
            crate::i18n::strs().msg_page_name_marked_as_a_background(name)
        };
    }
}

/// PR #415 review: the underlay chain's effective raster density —
/// the device-folded density degraded against the texture budget
/// MINUS the foreground layers' live bytes. The chain's page rects
/// enter as the "visible set" of `layers::degrade_to_budget` (the
/// store's own rule: soft before clipped, never content loss), so
/// the two canvas texture paths share ONE budget and neither can
/// spend it independently.
fn underlay_ppi(raster_ppi: f32, rects: &[(i64, crate::layers::Rect)], fg_bytes: u64) -> f32 {
    let avail = crate::layers::TEXTURE_BUDGET_BYTES.saturating_sub(fg_bytes);
    crate::layers::degrade_to_budget(raster_ppi, rects, avail)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PR #415 review (the CodeRabbit scenario): a healthy budget
    /// keeps the chain's density BIT-IDENTICAL, and a starved one
    /// degrades it instead of allocating past the budget — the old
    /// 2048 texture cap incidentally bounded eight Letter backgrounds
    /// at 128 MiB; the device-derived ceiling alone would let zoom 8
    /// × 2× retain 8 × 8192² RGBA ≈ 2 GiB.
    #[test]
    fn underlay_chain_ppi_shares_the_joint_budget() {
        let letter = (0i64, (0.0, 0.0, 8.5, 11.0));
        // Healthy (nothing held by the foreground): full density,
        // bit-identical — the early return never touches the ppi.
        assert_eq!(underlay_ppi(96.0, &[letter], 0), 96.0);
        // The chain scenario under the HEADLESS 2048 ceiling: eight
        // Letter pages at zoom 8 clamp to 2048² each (16 MiB, 134 MiB
        // total) — still fits, still full density.
        let eight: Vec<(i64, crate::layers::Rect)> = (0..8).map(|_| letter).collect();
        assert_eq!(underlay_ppi(768.0, &eight, 0), 768.0);
        // Starved — the foreground holds all but 8 KiB: the chain
        // degrades to a couple of ppi (soft, content preserved).
        let starved = underlay_ppi(
            768.0,
            &eight,
            crate::layers::TEXTURE_BUDGET_BYTES - 8 * 1024,
        );
        assert!(starved < 20.0, "the starved chain degrades: {starved}");
        assert!(starved >= 1.0, "content is never lost, however soft");
        // And a single page starved the same way also stays alive.
        let one = underlay_ppi(96.0, &[letter], crate::layers::TEXTURE_BUDGET_BYTES - 8 * 1024);
        assert!(one < 20.0, "one starved page degrades: {one}");
        assert!(one >= 1.0, "one starved page keeps its content");
    }
}
