//! Test hooks: the debug_* accessors and *_for_test mutators the
//! headless test suite drives state through.

use super::*;

impl App {
    /// Test/introspection hooks for the kittest interaction tests.
    #[doc(hidden)]
    /// #121: seed a shape's text for the spell-check tests — the text
    /// editor's own commit path (one undo step, cache refreshes), so
    /// the dialog walk sees exactly what a user edit produces.
    #[doc(hidden)]
    pub fn debug_set_shape_text_for_test(&mut self, shape: i64, text: &str) {
        let Some(page) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else {
            return;
        };
        let scope = dc_core::Scope::Shape { page, shape };
        let mac = dc::textfunc::text_edit_macro(&ed.doc, &scope, text);
        self.apply(mac);
    }

    /// #121: the word currently flagged by the open spelling pass.
    #[doc(hidden)]
    pub fn debug_spell_word(&self) -> Option<String> {
        self.spell
            .session
            .as_ref()
            .and_then(|s| s.word.as_ref())
            .map(|w| w.token.word.clone())
    }

    /// #121: the ranked suggestions for the flagged word.
    #[doc(hidden)]
    pub fn debug_spell_suggestions(&self) -> Vec<String> {
        self.spell
            .session
            .as_ref()
            .map(|s| s.suggestions.clone())
            .unwrap_or_default()
    }

    /// #121: the session's walk position (page, shape index, total).
    #[doc(hidden)]
    pub fn debug_spell_position(&self) -> Option<(i64, usize, usize)> {
        self.spell
            .session
            .as_ref()
            .map(|s| (s.page, s.idx, s.total))
    }

    /// #121: the app-level custom dictionary's words (Add assertions).
    #[doc(hidden)]
    pub fn debug_spell_custom(&self) -> &[String] {
        &self.spell.custom
    }

    /// #121: a shape's current plain text (the Change assertions).
    #[doc(hidden)]
    pub fn debug_shape_plain_text(&self, shape: i64) -> Option<String> {
        let page = self.page_id()?;
        let ed = self.ed.as_ref()?;
        let p = ed.doc.pages.iter().find(|p| p.id == page)?;
        vsdx::model::find_shape_by_id(&p.shapes, shape).and_then(|s| s.sheet.text.clone())
    }

    pub fn debug_shape_count(&self) -> usize {
        self.ed
            .as_ref()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .map(|p| p.shapes.len())
            .unwrap_or(0)
    }
    #[doc(hidden)]
    pub fn debug_master_cells(&self) -> Vec<(crate::stencil::MasterKey, Rect)> {
        self.stencil.cell_rects.clone()
    }
    /// Column count of the last stencil layout, re-derived from the
    /// actual cell rects (distinct column x positions) so tests check
    /// the rendered layout, not a stored number (GitHub #53).
    #[doc(hidden)]
    pub fn debug_stencil_cols(&self) -> usize {
        let mut xs: Vec<f32> = self
            .stencil
            .cell_rects
            .iter()
            .map(|(_, r)| r.left())
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        xs.len()
    }
    /// Width the last stencil layout wrapped into (pane content width
    /// minus the scrollbar allowance) — the input tests use to
    /// re-derive the expected column count (GitHub #53).
    #[doc(hidden)]
    pub fn debug_stencil_wrap_width(&self) -> f32 {
        self.stencil.last_wrap_width
    }
    /// Stencil panel content rect from the last frame; its right edge
    /// plus the 6px frame margin is the panel's resize handle (the
    /// kittest: multi-selection size (#26).
    #[doc(hidden)]
    /// kittest re-wrap tests drag it, GitHub #53).
    #[doc(hidden)]
    pub fn debug_stencil_panel_inner_rect(&self) -> Rect {
        self.stencil_panel_inner
    }
    #[doc(hidden)]
    pub fn debug_canvas_rect(&self) -> Rect {
        self.canvas_rect
    }
    /// kittest: the active view zoom (#22 asserts the entry zoom and
    /// the restore).
    #[doc(hidden)]
    pub fn debug_view_zoom(&self) -> f32 {
        self.view.zoom
    }
    /// kittest: the open in-place editor session as (page, shape)
    /// (#22).
    #[doc(hidden)]
    /// #267: the open editor's caret (kittest debugging).
    #[doc(hidden)]
    pub fn debug_text_edit_caret(&self) -> Option<usize> {
        self.text_edit.as_ref().map(|s| s.caret)
    }

    pub fn debug_text_edit(&self) -> Option<(i64, i64)> {
        self.text_edit.as_ref().map(|s| (s.page, s.shape))
    }
    /// kittest: the screen rect the editor overlay covers — the
    /// shape's text block mapped through the live view (#22).
    #[doc(hidden)]
    pub fn debug_text_edit_rect(&self) -> Option<Rect> {
        let sess = self.text_edit.as_ref()?;
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == sess.page)?;
        let (x, y, w, h) = dc::svg::text_block_page_rect(&ed.doc, page, sess.shape)?;
        let geom = self.page_geom();
        let tl = canvas::page_to_screen(x, y + h, &self.view, &geom);
        let br = canvas::page_to_screen(x + w, y, &self.view, &geom);
        Some(Rect::from_two_pos(tl, br))
    }
    #[doc(hidden)]
    /// kittest: multi-selection size (#26).
    #[doc(hidden)]
    pub fn debug_sel_set_len(&self) -> usize {
        self.sel_set.len()
    }

    /// kittest (#177): the last painted connection-point overlay —
    /// reveal points, glue-target highlight, and the selected
    /// connector's glued sites, all page inches y-up.
    #[doc(hidden)]
    pub fn debug_conn_fx(&self) -> crate::connectors::ConnPointFx {
        self.conn_fx_last.clone()
    }

    /// #109 tests: the live snap & glue toggles.
    pub fn debug_snap_settings(&self) -> crate::snap::SnapSettings {
        self.snap
    }

    /// #109 tests: the document's persisted DocumentSettings values.
    pub fn debug_doc_snap_glue(&self) -> Option<vsdx::model::DocSnapGlue> {
        self.ed.as_ref().and_then(|e| e.doc.doc_snap_glue())
    }

    /// #109 tests: a shape's page rect (inches, y-up, bottom-left
    /// origin) — the snap engine's own frame.
    pub fn debug_shape_page_rect(&self, shape_id: i64) -> Option<(f64, f64, f64, f64)> {
        let ed = self.ed.as_ref()?;
        let page_id = self.page_id()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        self.shape_rect(&ed.doc, page_id, sh)
    }

    /// #109 tests: how many dynamic-grid hints the current frame
    /// holds (nonzero only mid-drag).
    pub fn debug_snap_hint_count(&self) -> usize {
        self.snap_live.len()
    }
    /// kittest (#168 diagnosis): the live dynamic-grid hints.
    #[doc(hidden)]
    pub fn debug_snap_hints_for_test(&self) -> Vec<crate::snap::Hint> {
        self.snap_live.clone()
    }

    /// #109 tests: replace the live snap settings in memory (drag
    /// tests that verify pure translation semantics turn snapping
    /// off; the document copy is untouched).
    pub fn debug_set_snap_for_test(&mut self, s: crate::snap::SnapSettings) {
        self.snap = s;
    }

    /// #113 tests: the page's guides — (id, kind name, x, y), page
    /// inches y-up (x = the line for vertical, y for horizontal,
    /// both for points).
    pub fn debug_guides(&self) -> Vec<(i64, &'static str, f64, f64)> {
        self.page_guides()
            .iter()
            .map(|g| {
                let kind = match g.kind {
                    crate::guides::GuideKind::Vertical => "vertical",
                    crate::guides::GuideKind::Horizontal => "horizontal",
                    crate::guides::GuideKind::Point => "point",
                };
                (g.id, kind, g.x, g.y)
            })
            .collect()
    }

    /// #108 tests: the page's Connect records as (from, from_cell,
    /// from_part, to, to_cell, to_part).
    pub fn debug_connects(&self) -> Vec<(i64, String, i64, i64, String, i64)> {
        let Some(page_id) = self.page_id() else {
            return Vec::new();
        };
        let Some(ed) = self.ed.as_ref() else {
            return Vec::new();
        };
        ed.doc
            .pages
            .iter()
            .find(|p| p.id == page_id)
            .map(|p| {
                p.connects()
                    .list
                    .iter()
                    .map(|c| {
                        (
                            c.from_sheet,
                            c.from_cell.clone(),
                            c.from_part,
                            c.to_sheet,
                            c.to_cell.clone(),
                            c.to_part,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// #108 tests: one geometry row cell of a shape.
    pub fn debug_row_cell(
        &self,
        page: i64,
        shape: i64,
        sec_ix: i64,
        row_ix: i64,
        cell: &str,
    ) -> Option<f64> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        sh.sheet
            .sections_named("Geometry")
            .into_iter()
            .find(|s| s.ix == sec_ix)?
            .rows
            .iter()
            .find(|r| r.ix == row_ix)?
            .cell(cell)
            .and_then(|c| c.numeric())
    }

    /// #202 tests: one geometry-row cell's FORMULA (the control-drag
    /// customizes it away).
    #[doc(hidden)]
    pub fn debug_row_cell_formula(
        &self,
        page: i64,
        shape: i64,
        sec_ix: i64,
        row_ix: i64,
        cell: &str,
    ) -> Option<String> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        sh.sheet
            .sections_named("Geometry")
            .into_iter()
            .find(|s| s.ix == sec_ix)?
            .rows
            .iter()
            .find(|r| r.ix == row_ix)?
            .cell(cell)
            .and_then(|c| c.f.clone())
    }

    /// #108 tests: the active mouse tool.
    pub fn debug_tool(&self) -> Tool {
        self.tool
    }

    /// #108 tests: the shape's LOCAL Connection-section rows as
    /// (row ix, X, Y) — empty until a point is materialized onto the
    /// instance.
    pub fn debug_conn_rows(&self, page: i64, shape: i64) -> Vec<(i64, f64, f64)> {
        let Some(ed) = self.ed.as_ref() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape) else {
            return Vec::new();
        };
        sh.sheet
            .sections_named("Connection")
            .into_iter()
            .flat_map(|sec| {
                sec.rows.iter().map(|r| {
                    (
                        r.ix,
                        r.cell("X").and_then(|c| c.numeric()).unwrap_or(0.0),
                        r.cell("Y").and_then(|c| c.numeric()).unwrap_or(0.0),
                    )
                })
            })
            .collect()
    }

    /// #108 tests: the Connection Point tool's selection.
    pub fn debug_connpt_sel(&self) -> Option<(i64, i64)> {
        self.connpt_sel
    }

    /// #108 tests: the line-jump hops painted on the last frame.
    pub fn debug_jump_count(&self) -> usize {
        self.jumps_last
    }

    /// #113 tests: the last copy-as-picture payload as text (the SVG
    /// vector flavor); None for the PNG flavor.
    pub fn debug_last_picture_text(&self) -> Option<String> {
        self.last_picture
            .as_ref()
            .and_then(|b| String::from_utf8(b.clone()).ok())
            .filter(|t| t.starts_with("<svg"))
    }

    /// #113 tests: whether the page carries the background mark.
    pub fn debug_page_is_bg(&self, page: i64) -> bool {
        self.is_background_page(page)
    }

    /// #156 tests: a shape's effective User-row value by name.
    pub fn debug_user_row(&self, page: i64, shape: i64, row: &str) -> Option<String> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        let chain = shape_chain(&ed.doc, page.id, shape).ok()?;
        // #191: chain-aware — master-root rows resolve too.
        ed.doc
            .effective_user_row(sh, &chain, row)?
            .cell("Value")
            .and_then(|c| c.v.clone())
    }

    /// #156 tests: whether the Container Tools contextual tab is up.
    pub fn debug_tab_is_container_tools(&self) -> bool {
        self.tab == Tab::ContainerTools
    }

    /// #190/#192 tests: whether any ShapeSheet window is open.
    pub fn debug_shapesheet_open(&self) -> bool {
        !self.shapesheets.is_empty()
    }

    /// #192 tests: the open sheets' shape ids, in open order.
    pub fn debug_shapesheets(&self) -> Vec<i64> {
        self.shapesheets.clone()
    }

    /// #157 tests: a geometry row's type.
    pub fn debug_geo_row_type_for_test(&self, shape: i64, row_ix: i64) -> Option<String> {
        let ed = self.ed()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        sh.sheet
            .sections_named("Geometry")
            .first()?
            .rows
            .iter()
            .find(|r| r.ix == row_ix)?
            .t
            .clone()
    }

    /// #157 tests: replace one of the shape's OWN geometry rows (the
    /// section must exist) — sets the row type and cell values.
    pub fn debug_set_geo_row_for_test(
        &mut self,
        page: i64,
        shape: i64,
        sec_ix: i64,
        row_ix: i64,
        t: &str,
        cells: &[(&str, f64)],
    ) {
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(page) = ed.doc.pages.iter_mut().find(|p| p.id == page) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id_mut(&mut page.shapes, shape) else {
            return;
        };
        let mut row_cells: Vec<(String, vsdx::Cell)> = vec![
            ("X".into(), vsdx::Cell::value("0.5")),
            ("Y".into(), vsdx::Cell::value("0.5")),
        ];
        for (n, v) in cells {
            row_cells.retain(|(k, _)| k != n);
            row_cells.push((n.to_string(), vsdx::Cell::value(format!("{v:.6}"))));
        }
        let mut hit = false;
        for sec in sh.sheet.children.iter_mut() {
            if let vsdx::model::SheetChild::Section(sec) = sec {
                if sec.name == "Geometry" && sec.ix == sec_ix {
                    match sec.rows.iter_mut().find(|r| r.ix == row_ix) {
                        Some(row) => {
                            row.t = Some(t.to_string());
                            row.cells
                                .retain(|(n, _)| !cells.iter().any(|(c, _)| c == n));
                            for (n, v) in cells {
                                row.cells
                                    .push((n.to_string(), vsdx::Cell::value(format!("{v:.6}"))));
                            }
                        }
                        None => {
                            // The section exists, the row does not —
                            // append it.
                            sec.rows.push(vsdx::model::Row {
                                ix: row_ix,
                                t: Some(t.to_string()),
                                cells: row_cells,
                                ..vsdx::model::Row::default()
                            });
                        }
                    }
                    hit = true;
                    break;
                }
            }
        }
        if !hit {
            // Materialize the section (a master-based shape has none
            // locally): a MoveTo at the origin + the requested row.
            let mut row = vsdx::model::Row {
                ix: row_ix,
                t: Some(t.to_string()),
                ..vsdx::model::Row::default()
            };
            let mut all: Vec<(String, vsdx::Cell)> = vec![
                ("X".into(), vsdx::Cell::value("0.5")),
                ("Y".into(), vsdx::Cell::value("0.5")),
            ];
            for (n, v) in cells {
                all.retain(|(k, _)| k != n);
                all.push((n.to_string(), vsdx::Cell::value(format!("{v:.6}"))));
            }
            row.cells = all;
            sh.sheet
                .children
                .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                    name: "Geometry".into(),
                    ix: sec_ix,
                    rows: vec![
                        vsdx::model::Row {
                            ix: 1,
                            t: Some("MoveTo".into()),
                            cells: vec![
                                ("X".into(), vsdx::Cell::value("0")),
                                ("Y".into(), vsdx::Cell::value("0")),
                            ],
                            ..vsdx::model::Row::default()
                        },
                        row,
                    ],
                    ..vsdx::model::Section::default()
                }));
        }
    }

    /// #113 tests: the page ids in order.
    pub fn debug_page_ids(&self) -> Vec<i64> {
        self.ed()
            .map(|e| e.doc.pages.iter().map(|p| p.id).collect())
            .unwrap_or_default()
    }

    /// #113 tests: the last copy-as-picture PNG's byte length.
    pub fn debug_last_picture_len(&self) -> Option<usize> {
        self.last_picture.as_ref().map(|v| v.len())
    }

    /// #113 tests: insert a picture from bytes (the kittest path —
    /// no native dialog).
    pub fn debug_insert_picture_bytes(&mut self, bytes: &[u8]) {
        self.insert_picture_bytes(bytes);
    }

    /// #113 tests: the current page's background assignment.
    pub fn debug_back_page(&self) -> Option<i64> {
        self.page_id().and_then(|p| self.back_page_of(p))
    }

    /// #113 tests: assign the current page's background (kittest
    /// path — no dialog).
    pub fn debug_set_back_page(&mut self, bg: Option<i64>) {
        self.set_back_page(bg);
    }

    /// #113 tests: add a fresh page and return its id.
    pub fn debug_add_page(&mut self) -> i64 {
        let page_id = self.page_id().unwrap_or(0);
        let _ = page_id;
        // The AddPage command through the normal path.
        let ed = self.ed.as_ref().expect("doc");
        let id = ed.doc.pages.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        self.apply(dc_core::commands::AddPage::new(format!("Page-{}", id)));
        id
    }

    /// #113 tests: the document's runtime media parts.
    pub fn debug_media_parts(&self) -> Vec<String> {
        self.ed
            .as_ref()
            .map(|e| e.doc.extra_media.iter().map(|m| m.part.clone()).collect())
            .unwrap_or_default()
    }

    /// #113 tests: the page's current size in inches.
    pub fn debug_page_size(&self) -> (f64, f64) {
        let g = self.page_geom();
        (g.w_in, g.h_in)
    }

    /// #113 tests: a cell's formula (the `F` attribute) on a shape.
    pub fn debug_cell_formula(&self, page: i64, shape: i64, cell: &str) -> Option<String> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        sh.sheet.cell(cell).and_then(|c| c.f.clone())
    }
    pub fn debug_sel(&self) -> Option<(i64, i64)> {
        self.sel
    }
    /// The open group-edit stack, outermost first (#78).
    #[doc(hidden)]
    pub fn debug_group_edit(&self) -> Vec<i64> {
        self.group_edit.clone()
    }
    #[doc(hidden)]
    pub fn debug_sel_set(&self) -> Vec<(i64, i64)> {
        self.sel_set.clone()
    }
    /// Selection rect in SCREEN coords (for corner-handle tests).
    #[doc(hidden)]
    pub fn debug_sel_br_corner(&self) -> Option<egui::Pos2> {
        let r = self.sel_page_rect()?;
        let ppi = self.view.px_per_in();
        Some(egui::pos2(
            self.view.pan.x + r.right() * ppi,
            self.view.pan.y + r.bottom() * ppi,
        ))
    }
    #[doc(hidden)]
    pub fn debug_sel_width(&self) -> f64 {
        self.sel_page_rect()
            .map(|r| r.width() as f64)
            .unwrap_or(0.0)
    }
    #[doc(hidden)]
    pub fn debug_sel_height(&self) -> f64 {
        self.sel_page_rect()
            .map(|r| r.height() as f64)
            .unwrap_or(0.0)
    }
    #[doc(hidden)]
    pub fn debug_sel_tl_corner(&self) -> Option<egui::Pos2> {
        let r = self.sel_page_rect()?;
        let ppi = self.view.px_per_in();
        Some(egui::pos2(
            self.view.pan.x + r.left() * ppi,
            self.view.pan.y + r.top() * ppi,
        ))
    }
    /// One shape's bbox in SCREEN coords (context-menu placement tests).
    #[doc(hidden)]
    pub fn debug_shape_screen_rect(&self, shape_id: i64) -> Option<egui::Rect> {
        self.shape_screen_rect(shape_id)
    }
    /// Context-menu rect from the last frame; None while closed
    /// (placement tests, issue #49).
    #[doc(hidden)]
    pub fn debug_ctx_menu_rect(&self) -> Option<egui::Rect> {
        self.ctx_menu_rect
    }
    #[doc(hidden)]
    pub fn debug_armed(&self) -> Option<MasterKey> {
        self.stencil.armed
    }
    /// kittest (#63): open a stencil file into the pane exactly like
    /// the "Open Existing Stencil…" flow (minus the native dialog).
    #[doc(hidden)]
    pub fn open_stencil_for_test(&mut self, path: &std::path::Path) {
        self.open_stencil(path.to_path_buf());
    }
    /// kittest (#83): File→Open through the real dialog-worker seam —
    /// the next dialog resolves with `path` instead of opening rfd.
    #[doc(hidden)]
    pub fn open_file_for_test(&mut self, path: std::path::PathBuf) {
        self.dialog_preset_for_test = Some(Some(path));
        self.do_open();
    }
    /// kittest (#63): open-stencil entry titles + expansion, pane order.
    #[doc(hidden)]
    pub fn debug_stencil_entries(&self) -> Vec<(String, bool)> {
        self.stencils
            .iter()
            .map(|(_, st)| (st.title.clone(), st.expanded))
            .collect()
    }
    /// kittest (#86): set the pane's search box (the results-view
    /// filter) without driving the text edit widget.
    #[doc(hidden)]
    pub fn set_search_for_test(&mut self, q: &str) {
        self.stencil.search = q.to_string();
    }
    /// kittest (#88): scroll the stencil pane back to its top next
    /// frame (a just-opened stencil's reveal may have scrolled the
    /// Quick Shapes cells out of the viewport).
    #[doc(hidden)]
    pub fn scroll_pane_to_top_for_test(&mut self) {
        self.stencil.scroll_fix = Some(0.0);
    }
    /// kittest (#88): the Quick Shapes header rect from the last
    /// layout.
    #[doc(hidden)]
    pub fn debug_quick_rect_for_test(&self) -> Option<egui::Rect> {
        self.stencil.quick_rect
    }
    /// kittest (#88): the search box rect from the last layout.
    #[doc(hidden)]
    pub fn debug_search_rect_for_test(&self) -> Option<egui::Rect> {
        self.stencil.search_rect
    }
    /// kittest (#102): reveal the document-stencil entry for this
    /// session (masters outside Quick Shapes' head set — e.g. the 1D
    /// Line — are only listed there).
    #[doc(hidden)]
    pub fn show_doc_stencil_for_test(&mut self) {
        self.stencil.doc_stencil_visible = true;
    }
    /// kittest (#86): the pane entries currently VISIBLE (headers
    /// rendered this frame) — a search hides zero-match entries.
    #[doc(hidden)]
    pub fn debug_visible_entry_ids(&self) -> Vec<crate::stencil::StencilId> {
        self.stencil.entry_rects.iter().map(|(id, _)| *id).collect()
    }
    /// kittest (#84): the edited document's masters (id, NameU).
    #[doc(hidden)]
    pub fn debug_doc_masters(&self) -> Vec<(i64, String)> {
        self.ed
            .as_ref()
            .map(|ed| {
                ed.doc
                    .masters
                    .iter()
                    .map(|m| (m.id, m.name_u.clone().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// kittest (#85): the landed thumbnail texture for one pane cell.
    #[doc(hidden)]
    pub fn debug_thumb_id_for_test(
        &self,
        stencil: StencilId,
        master: i64,
    ) -> Option<egui::TextureId> {
        let serial = match stencil {
            StencilId::Document => crate::stencil::DOC_SERIAL,
            StencilId::Open(ix) => self.stencils.get(ix).map(|s| s.serial())?,
        };
        self.stencil.thumb_id(serial, master)
    }
    /// kittest (#63): stencil list entry header rects, document entry
    /// first.
    #[doc(hidden)]
    pub fn debug_stencil_entry_rects(&self) -> Vec<(StencilId, egui::Rect)> {
        self.stencil.entry_rects.clone()
    }
    #[doc(hidden)]
    pub fn debug_carry(&self) -> Option<MasterKey> {
        self.stencil.carry
    }
    /// MRU entries, most recent first (recent-files tests).
    #[doc(hidden)]
    pub fn debug_recent(&self) -> &[PathBuf] {
        self.recent.entries()
    }
    /// Adopt an MRU verbatim, never persisted (kittest snapshot of the
    /// populated Recent panes).
    #[doc(hidden)]
    pub fn set_recent_for_test(&mut self, files: Vec<PathBuf>) {
        self.recent.set_entries_for_test(files);
    }
    /// kittest: pin the displayed document path so the backstage Info
    /// page renders a fixed Location instead of the test's random
    /// tempdir (goldens must be identical on every OS/runner).
    #[doc(hidden)]
    pub fn set_path_for_test(&mut self, path: PathBuf) {
        self.path = path;
    }
    /// kittest: enter the backstage exactly like the File tab does —
    /// the #55 landing rule included (Info with a document, Recent
    /// without one).
    #[doc(hidden)]
    pub fn open_backstage_for_test(&mut self) {
        self.backstage = true;
        self.backstage_page = if self.ed.is_some() {
            BackPage::Info
        } else {
            BackPage::Recent
        };
        self.export_pane.reset_for(&self.path);
    }
    /// kittest: jump straight to one backstage page ("info" | "recent"
    /// | "new" | "print" | "share" | "export").
    #[doc(hidden)]
    pub fn set_backstage_page_for_test(&mut self, name: &str) {
        if let Some(p) = BackPage::from_test_name(name) {
            self.backstage_page = p;
        }
    }
    /// kittest: open the Options dialog exactly like the rail's
    /// Options entry does (#268).
    #[doc(hidden)]
    pub fn open_options_for_test(&mut self) {
        self.open_options_dialog();
    }
    /// kittest: jump straight to one Options page ("general" | "save"
    /// | "language") — a no-op while the dialog is closed (#268).
    #[doc(hidden)]
    pub fn set_options_page_for_test(&mut self, name: &str) {
        if let Some(d) = self.options_draft.as_mut() {
            if let Some(p) = crate::options::Page::from_test_name(name) {
                d.page = p;
            }
        }
    }
    /// kittest: whether the Options dialog is showing (#268).
    #[doc(hidden)]
    pub fn debug_options_open(&self) -> bool {
        self.options_open
    }
    /// kittest: the live options store (#268 round-trip asserts).
    #[doc(hidden)]
    pub fn debug_options(&self) -> &crate::options::Options {
        &self.options_store
    }
    /// kittest: whether the File backstage is showing.
    #[doc(hidden)]
    pub fn debug_backstage(&self) -> bool {
        self.backstage
    }
    /// Current canvas zoom (fit-mode interaction tests).
    #[doc(hidden)]
    pub fn debug_zoom(&self) -> f32 {
        self.view.zoom
    }
    /// kittest: a manual zoom (#62's second ruler density) — leaves fit
    /// mode like the real control so the zoom actually applies.
    #[doc(hidden)]
    pub fn set_zoom_for_test(&mut self, z: f32) {
        self.view.zoom = z.clamp(0.1, 8.0);
        self.view.fit_mode = false;
        self.view.fit_pending = false;
    }

    /// #237: the view's (zoom, pan) for zoom-behavior assertions.
    /// #283: jump the view's vertical pan to the page's top (true) or
    /// bottom edge — Ctrl+Home/End. The horizontal pan is untouched
    /// (Visio's jump is vertical); the per-frame extent clamp in the
    /// canvas bounds it, and this pre-computes the same range so the
    /// jump lands exactly on the paper edge.
    pub(super) fn jump_view_vertical(&mut self, top: bool) {
        let geom = self.page_geom();
        let (origin, content) = canvas::content_area(self.canvas_rect, self.view.show_rulers);
        // The jump targets the PAPER edges literally (PR #284 review
        // round 5): the extent range bounds are the manual-pan
        // room (half-viewport around the fit center when pinned), and
        // riding them hid half the page on fitted drawings. The
        // per-frame extent clamp still validates the result.
        let page_px = geom.h_in as f32 * self.view.px_per_in();
        self.view.pan.y = if top {
            origin.y
        } else {
            origin.y + content.y - page_px
        };
        self.view.fit_mode = false;
    }

    pub fn debug_view_for_test(&self) -> (f32, f32, f32) {
        (self.view.zoom, self.view.pan.x, self.view.pan.y)
    }

    /// #224: arm/clear the splash moment directly (the kittest path —
    /// `App::new`'s production splash is ctor-bound).
    #[doc(hidden)]
    pub fn debug_splash_for_test(&mut self, started: Option<std::time::Instant>) {
        self.splash = started;
    }

    /// #224: whether the splash overlay is up.
    #[doc(hidden)]
    pub fn debug_splash_active_for_test(&self) -> bool {
        self.splash.is_some()
    }

    /// #224: whether a file open is in flight (the worker parse).
    #[doc(hidden)]
    pub fn debug_loading_for_test(&self) -> bool {
        self.loading.is_some()
    }

    /// #242: whether the landed document's first render is still
    /// behind the scrim.
    #[doc(hidden)]
    pub fn debug_rendering_for_test(&self) -> bool {
        self.rendering.is_some()
    }

    /// #242: whether the About window is open.
    #[doc(hidden)]
    pub fn debug_about_open_for_test(&self) -> bool {
        self.about_open
    }

    /// #242: a pointer click at the screen center, for the splash-skip
    /// test (the splash reads raw input, not widgets).
    #[doc(hidden)]
    pub fn debug_click_for_test(&mut self) {
        // The frame runs inside the harness; the click rides egui's
        // input replay — record it for the next update().
        self.test_click = true;
    }
    /// kittest: page names in tab order (#54).
    #[doc(hidden)]
    pub fn debug_page_names(&self) -> Vec<String> {
        self.ed()
            .map(|e| e.doc.pages.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// kittest: the first top-level shape's layer texture id (#92/#93)
    /// — `None` while its first raster is still on a worker.
    #[doc(hidden)]
    pub fn debug_page_tex_id_for_test(&self) -> Option<egui::TextureId> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let first = page.shapes.first()?;
        self.layers.tex_id_for_test(first.id)
    }

    /// kittest: one shape's layer texture id (#93) — stable across a
    /// pure move (the texture is translation-invariant).
    #[doc(hidden)]
    pub fn debug_layer_tex_id_for_test(&self, shape_id: i64) -> Option<egui::TextureId> {
        self.layers.tex_id_for_test(shape_id)
    }

    /// kittest: a shape layer's content version (#22 review) — the
    /// cache probe that a ppi-bucket re-raster cannot perturb.
    #[doc(hidden)]
    pub fn debug_layer_version_for_test(&self, shape_id: i64) -> u64 {
        self.layers.version_for_test(shape_id)
    }

    /// kittest (#124 item 4): total RGBA bytes of the cached layer
    /// textures — the LRU budget's volume probe.
    #[doc(hidden)]
    pub fn debug_layer_tex_bytes_for_test(&self) -> u64 {
        self.layers.tex_bytes_for_test()
    }

    /// kittest (#124 item 4): pin the texture byte budget for an
    /// eviction test (the production default is 768 MiB).
    #[doc(hidden)]
    pub fn set_layer_texture_budget_for_test(&mut self, bytes: u64) {
        self.layers.set_texture_budget_for_test(bytes)
    }

    /// kittest (#143): the Quick Shapes entry's cell keys from the
    /// last stencil layout, in display order.
    #[doc(hidden)]
    pub fn debug_quick_cells_for_test(&self) -> Vec<(StencilId, i64)> {
        self.stencil.quick_keys.clone()
    }

    /// kittest (#143): point Quick Shapes persistence at an explicit
    /// store (loading any existing membership) — the restart
    /// simulation. Never persisted by the ephemeral constructors.
    #[doc(hidden)]
    pub fn set_quick_shapes_store_for_test(&mut self, path: &std::path::Path) {
        self.quick_shapes = crate::quickshapes::QuickShapes::at(path.to_path_buf());
    }

    /// kittest (#137): one cell of a shape's OWN Geometry section-0
    /// row, as (value, formula) — the vertex-drag probe.
    #[doc(hidden)]
    pub fn debug_geo_row_cell_for_test(
        &self,
        shape: i64,
        row_ix: i64,
        cell: &str,
    ) -> Option<(Option<String>, Option<String>)> {
        let ed = self.ed()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape)?;
        let row = sh
            .sheet
            .sections_named("Geometry")
            .iter()
            .find(|s| s.ix == 0)?
            .rows
            .iter()
            .find(|r| r.ix == row_ix)?;
        let c = row.cell(cell)?;
        Some((c.v.clone(), c.f.clone()))
    }

    /// kittest (#138): how many `User.msvSDContainerShape` rows the
    /// page carries (any container) — the dangling-row detector.
    #[doc(hidden)]
    pub fn debug_membership_rows_for_test(&self) -> usize {
        self.ed()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .map(|p| {
                p.shapes
                    .iter()
                    .flat_map(|s| s.sheet.sections_named("User"))
                    .filter(|sec| sec.named_row("msvSDContainerShape").is_some())
                    .count()
            })
            .unwrap_or(0)
    }

    /// kittest (#75): the shapes whose membership row names
    /// `container` (Visio's `User.msvSDContainerShape` convention).
    #[doc(hidden)]
    pub fn debug_container_members_for_test(&self, container: i64) -> Vec<i64> {
        self.page_id()
            .map(|p| self.container_member_ids(p, container))
            .unwrap_or_default()
    }

    /// kittest (#69): a group's direct member ids.
    #[doc(hidden)]
    pub fn debug_group_members_for_test(&self, group: i64) -> Vec<i64> {
        self.ed
            .as_ref()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, group))
            .map(|g| g.sub_shapes.iter().map(|m| m.id).collect())
            .unwrap_or_default()
    }

    /// kittest (#67): the window title last pushed to the OS (name +
    /// dirty marker) — headless stands in for the OS chrome.
    #[doc(hidden)]
    pub fn debug_window_title_for_test(&self) -> String {
        self.window_title.clone()
    }

    /// kittest (#68): the active page's geometry in inches — an
    /// inserted page must carry its source's sheet size, not the
    /// 0.1-in missing-cell clamp.
    #[doc(hidden)]
    pub fn debug_page_geom_for_test(&self) -> (f64, f64) {
        let g = self.page_geom();
        (g.w_in, g.h_in)
    }

    /// kittest: how many live drag ghosts the last frame drew (#96).
    #[doc(hidden)]
    /// kittest: whether the last frame painted the styled
    /// endpoint-drag ghost (#226).
    pub fn debug_endpoint_ghost_for_test(&self) -> bool {
        self.last_endpoint_ghost
    }

    pub fn debug_drag_ghosts_for_test(&self) -> usize {
        self.last_drag_ghosts
    }

    /// kittest (#283 review): the live drag's PRIMARY ghost quad on
    /// screen — the #96 contract is "the shape lands where the ghost
    /// showed", so drag tests assert against this rect (snap included)
    /// instead of a raw pixel delta.
    pub fn debug_drag_ghost_rect_for_test(&self) -> Option<egui::Rect> {
        self.last_drag_ghost_rect
    }

    /// kittest: how many shape layers have landed (#93) — the post cap
    /// keeps the first frame from rasterizing the whole page inline.
    #[doc(hidden)]
    pub fn debug_layer_count_for_test(&self) -> usize {
        self.layers.landed_count_for_test()
    }

    /// kittest: how many stencil master thumbnails have landed (#92).
    #[doc(hidden)]
    pub fn debug_stencil_thumb_count_for_test(&self) -> usize {
        self.stencil.thumb_count_for_test()
    }
    /// kittest (#92): the per-frame post cap the async contract asserts
    /// against.
    #[doc(hidden)]
    pub fn debug_pool_post_cap_for_test(&self) -> usize {
        self.pool.post_cap()
    }
    /// kittest (#92): the most raster jobs any single frame ever
    /// queued — must never exceed the cap (the anti-burst invariant).
    #[doc(hidden)]
    pub fn debug_pool_peak_posts_per_frame_for_test(&self) -> usize {
        self.pool.debug_peak_posts_per_frame()
    }
    /// kittest (#92): lifetime queued raster jobs.
    #[doc(hidden)]
    pub fn debug_pool_total_posts_for_test(&self) -> u64 {
        self.pool.debug_total_posts()
    }
    /// kittest: a shape's INSTANCE-LOCAL User row value (#25) — None
    /// while the row is inherited from the master.
    #[doc(hidden)]
    pub fn debug_user_value_for_test(&self, shape_id: i64, row: &str) -> Option<f64> {
        let ed = self.ed.as_ref()?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        sh.sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row(row))?
            .cell("Value")?
            .numeric()
    }
    /// kittest: a shape's effective hyperlink rows as (address,
    /// sub-address) — instance rows first, then unshadowed
    /// master-inherited rows (#28, #139).
    #[doc(hidden)]
    pub fn debug_hyperlinks_for_test(&self, shape_id: i64) -> Vec<(String, String)> {
        self.shape_hyperlinks(shape_id)
            .into_iter()
            .map(|(a, s, _)| (a, s))
            .collect()
    }
    /// kittest: the shape's INSTANCE hyperlink rows as (ix, address,
    /// sub-address, description) — empty until a row is materialized
    /// onto the instance sheet (#139).
    #[doc(hidden)]
    pub fn debug_instance_hyperlinks_for_test(
        &self,
        shape_id: i64,
    ) -> Vec<(i64, String, String, String)> {
        self.hyperlink_rows(shape_id)
            .into_iter()
            .filter(|r| !r.inherited)
            .map(|r| (r.ix, r.addr, r.sub, r.desc))
            .collect()
    }
    /// kittest: the shape's MASTER's hyperlink rows verbatim as
    /// (address, sub-address) — the edit-materialization path must
    /// leave these unchanged (#139).
    #[doc(hidden)]
    pub fn debug_master_hyperlinks_for_test(&self, shape_id: i64) -> Vec<(String, String)> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.get(self.page_idx) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return Vec::new();
        };
        sh.master
            .and_then(|id| ed.doc.master(id))
            .map(|m| {
                m.shapes
                    .iter()
                    .flat_map(|s| s.sheet.sections_named("Hyperlink"))
                    .flat_map(|sec| sec.rows.iter())
                    .map(|r| {
                        (
                            r.cell("Address")
                                .and_then(|c| c.v.clone())
                                .unwrap_or_default(),
                            r.cell("SubAddress")
                                .and_then(|c| c.v.clone())
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    /// kittest: annotation shapes on the Annotation layer (#29) as
    /// (shape id, text).
    #[doc(hidden)]
    pub fn debug_annotations_for_test(&self) -> Vec<(i64, String)> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.get(self.page_idx) else {
            return Vec::new();
        };
        let layer_ix = dc_core::layers::layer_rows(&ed.doc, page.id)
            .unwrap_or_default()
            .into_iter()
            .find(|r| dc_core::layers::layer_name(r) == Some("Annotation"))
            .map(|r| r.ix);
        let Some(layer_ix) = layer_ix else {
            return Vec::new();
        };
        page.shapes
            .iter()
            .filter(|s| {
                s.sheet.cell("LayerMember").and_then(|c| c.numeric()) == Some(layer_ix as f64)
            })
            .map(|s| (s.id, s.sheet.text.clone().unwrap_or_default()))
            .collect()
    }
    /// kittest: annotation visibility state (#29).
    #[doc(hidden)]
    pub fn show_comments_for_test(&self) -> bool {
        self.show_comments
    }
    /// kittest: flip annotation visibility directly (#29) — the ribbon
    /// button shares the standard toggle dispatch.
    #[doc(hidden)]
    pub fn set_show_comments_for_test(&mut self, on: bool) {
        self.show_comments = on;
    }
    /// kittest: one cell of a shape's effective Prop row (#30) —
    /// instance first, then the master.
    #[doc(hidden)]
    pub fn debug_prop_cell_for_test(&self, shape_id: i64, row: &str, cell: &str) -> Option<String> {
        self.prop_rows(shape_id)
            .iter()
            .find(|(n, _)| n == row)?
            .1
            .cell(cell)
            .and_then(|c| c.v.clone())
    }
    /// kittest: give a placed shape a Prop row through the command
    /// layer (#30) — one undo step, like a real authoring edit.
    #[doc(hidden)]
    pub fn debug_add_prop_row_for_test(
        &mut self,
        shape_id: i64,
        name: &str,
        type_: &str,
        format: &str,
        value: &str,
    ) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let row = dc_core::commands::RowSel::Named(name.to_string());
        let cell = |v: &str| vsdx::model::Cell::value(v.to_string());
        self.apply(dc_core::Macro::new(
            crate::i18n::strs().msg_add_name_prop_row_test(name),
            vec![
                Box::new(dc_core::SetRowCell::new(
                    scope.clone(),
                    "Prop",
                    0,
                    row.clone(),
                    "Type",
                    cell(type_),
                )),
                Box::new(dc_core::SetRowCell::new(
                    scope.clone(),
                    "Prop",
                    0,
                    row.clone(),
                    "Format",
                    cell(format),
                )),
                Box::new(dc_core::SetRowCell::new(
                    scope,
                    "Prop",
                    0,
                    row,
                    "Value",
                    cell(value),
                )),
            ],
        ));
    }
    /// kittest: a shape's effective Prop row Value (#30) — instance
    /// first, then the master.
    #[doc(hidden)]
    pub fn debug_prop_value_for_test(&self, shape_id: i64, row: &str) -> Option<String> {
        let rows = self.prop_rows(shape_id);
        rows.iter()
            .find(|(n, _)| n == row)?
            .1
            .cell("Value")
            .and_then(|c| c.v.clone())
    }
    /// kittest: a shape's effective cell value (#31).
    #[doc(hidden)]
    pub fn debug_cell_for_test(&self, shape_id: i64, cell: &str) -> Option<String> {
        let ed = self.ed()?;
        let page_id = self.page_id()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        // Char.X / Para.X read the effective first Character/Paragraph
        // row cell.
        if let Some(row_cell) = cell.strip_prefix("Char.") {
            return ed
                .doc
                .effective_sheet(sh, &chain)
                .sections_named("Character")
                .first()
                .and_then(|sec| sec.rows.first())
                .and_then(|r| r.cell(row_cell))
                .and_then(|c| c.v.clone())
                .map(|v| v.trim().to_string());
        }
        if let Some(row_cell) = cell.strip_prefix("Para.") {
            return ed
                .doc
                .effective_sheet(sh, &chain)
                .sections_named("Paragraph")
                .first()
                .and_then(|sec| sec.rows.first())
                .and_then(|r| r.cell(row_cell))
                .and_then(|c| c.v.clone())
                .map(|v| v.trim().to_string());
        }
        ed.doc
            .resolve_cell(sh, &chain, cell)
            .and_then(|c| c.v.clone())
            .map(|v| v.trim().to_string())
    }
    /// kittest: the active page's name (#54).
    #[doc(hidden)]
    pub fn debug_active_page_name(&self) -> String {
        self.ed()
            .and_then(|e| e.doc.pages.get(self.page_idx))
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }
    /// A shape's current text (double-click edit tests).
    #[doc(hidden)]
    /// #267: the open text editor's current draft (kittest).
    #[doc(hidden)]
    pub fn debug_text_edit_draft(&self) -> Option<String> {
        self.text_edit.as_ref().map(|s| s.draft.clone())
    }

    /// #267: the shape's Field rows (Value formulas) and raw text, for
    /// the Insert Field kittest flow.
    #[doc(hidden)]
    pub fn debug_field_state(&self, shape_id: i64) -> Option<(Vec<String>, Option<String>)> {
        let s = self.ed.as_ref().and_then(|ed| {
            ed.doc
                .pages
                .get(self.page_idx)
                .and_then(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id))
        })?;
        let formulas = s
            .sheet
            .sections_named("Field")
            .iter()
            .flat_map(|sec| {
                sec.rows
                    .iter()
                    .filter_map(|r| r.cell("Value").and_then(|c| c.f.clone()))
            })
            .collect();
        Some((formulas, s.sheet.text_raw.clone()))
    }

    pub fn debug_shape_text(&self, shape_id: i64) -> Option<String> {
        self.selected_text(shape_id)
    }
    /// The page SVG the canvas is currently showing (scratch debugging).
    #[doc(hidden)]
    pub fn debug_page_svg(&self) -> Option<String> {
        let ed = self.ed.as_ref()?;
        let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
        dc::svg::render_page(&ed.doc, page).ok()
    }
    /// All four selection-frame corners in SCREEN coords, [tl, tr, bl,
    /// br] — the four-corner resize tests (GitHub #59).
    #[doc(hidden)]
    pub fn debug_sel_corners(&self) -> Option<[egui::Pos2; 4]> {
        let r = self.sel_page_rect()?;
        let ppi = self.view.px_per_in();
        let at = |x: f32, y: f32| egui::pos2(self.view.pan.x + x * ppi, self.view.pan.y + y * ppi);
        Some([
            at(r.left(), r.top()),
            at(r.right(), r.top()),
            at(r.left(), r.bottom()),
            at(r.right(), r.bottom()),
        ])
    }
    /// One selection handle's screen position (line endpoint/midpoint
    /// drag tests, GitHub #59).
    #[doc(hidden)]
    pub fn debug_handle_screen(&self, id: crate::canvas::HandleId) -> Option<egui::Pos2> {
        let h = self.selection_handles().into_iter().find(|h| h.id == id)?;
        let geom = self.page_geom();
        Some(crate::canvas::page_to_screen(
            h.pos.0, h.pos.1, &self.view, &geom,
        ))
    }
    /// #202: whether the selection layer skips the 1D begin→end chord
    /// (the selected connector carries a curve row).
    #[doc(hidden)]
    pub fn debug_sel_1d_curved(&self) -> bool {
        self.sel_1d_curved()
    }
    /// The status-bar text (guard-refusal tests).
    #[doc(hidden)]
    pub fn debug_status(&self) -> &str {
        &self.status
    }
    /// #149 item 2 tests: the Format pane section a mini-toolbar
    /// Format icon targeted. This is the LAST target (the one-shot
    /// scroll focus is consumed by the pane's first render).
    #[doc(hidden)]
    pub fn debug_format_pane_focus(&self) -> Option<&'static str> {
        self.format_pane_last_focus.map(|f| match f {
            FormatSection::Fill => "fill",
            FormatSection::Line => "line",
            FormatSection::Text => "text",
        })
    }
    /// One resolved cell of a shape, as (value, formula) — asserting
    /// guarded cells stay untouched (GitHub #59 tests).
    #[doc(hidden)]
    pub fn debug_cell(&self, page: i64, shape: i64, name: &str) -> Option<(String, String)> {
        let ed = self.ed.as_ref()?;
        let p = ed.doc.pages.iter().find(|p| p.id == page)?;
        let sh = vsdx::model::find_shape_by_id(&p.shapes, shape)?;
        let chain = shape_chain(&ed.doc, page, shape).ok()?;
        let c = ed.doc.resolve_cell(sh, &chain, name)?;
        Some((
            c.v.clone().unwrap_or_default(),
            c.f.clone().unwrap_or_default(),
        ))
    }
    /// Test hook: write a cell on a shape's own sheet, optionally with a
    /// formula (e.g. `GUARD(2 in)`), through the command layer.
    #[doc(hidden)]
    pub fn set_cell_for_test(
        &mut self,
        page: i64,
        shape: i64,
        name: &str,
        v: &str,
        f: Option<&str>,
    ) {
        let cell = vsdx::model::Cell {
            v: Some(v.to_string()),
            f: f.map(|s| s.to_string()),
            ..vsdx::model::Cell::default()
        };
        self.apply(SetCell::new(Scope::Shape { page, shape }, name, cell));
    }
    /// Test hook: turn a Protection-section lock on for a shape
    /// (LockWidth/LockHeight/LockBegin/LockEnd — GitHub #59 tests).
    #[doc(hidden)]
    pub fn set_protection_for_test(&mut self, page: i64, shape: i64, name: &str, on: bool) {
        use vsdx::model::{Row, Section, SheetChild};
        let Some(ed) = self.ed_mut() else { return };
        let Some(p) = ed.doc.pages.iter_mut().find(|p| p.id == page) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id_mut(&mut p.shapes, shape) else {
            return;
        };
        let val = if on { "1" } else { "0" };
        // Replace an existing lock cell in place (Row::cell reads the
        // first match — appending would make a later `on=false`
        // unreadable and the lock uncleearable); create the section
        // otherwise. Both paths bump the version so the view refreshes.
        let set_cell = |cells: &mut Vec<(String, vsdx::model::Cell)>| match cells
            .iter_mut()
            .find(|(n, _)| n == name)
        {
            Some((_, c)) => *c = vsdx::model::Cell::value(val),
            None => cells.push((name.to_string(), vsdx::model::Cell::value(val))),
        };
        if let Some(SheetChild::Section(sec)) = sh
            .sheet
            .children
            .iter_mut()
            .find(|c| matches!(c, SheetChild::Section(s) if s.name == "Protection"))
        {
            match sec.rows.first_mut() {
                Some(row) => set_cell(&mut row.cells),
                None => sec.rows.push(Row {
                    cells: vec![(name.to_string(), vsdx::model::Cell::value(val))],
                    ..Row::default()
                }),
            }
        } else {
            sh.sheet.children.push(SheetChild::Section(Section {
                name: "Protection".into(),
                rows: vec![Row {
                    cells: vec![(name.to_string(), vsdx::model::Cell::value(val))],
                    ..Row::default()
                }],
                ..Section::default()
            }));
        }
        self.dirty = true;
        self.version += 1;
    }
}
