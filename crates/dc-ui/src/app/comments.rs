//! Hyperlinks and shape comments: row accessors, following links,
//! and the comments window.

use super::*;

impl App {
    /// The shape's hyperlink rows (#28): instance rows first, then the
    /// master's (resolved like every other cell read). A master row
    /// whose ix an instance row already carries is SHADOWED — the
    /// local row is the override (first-edit materialization, #139) —
    /// so a materialized row replaces its inherited twin here instead
    /// of duplicating it.
    pub(super) fn shape_hyperlinks(&self, shape_id: i64) -> Vec<(String, String, String)> {
        self.hyperlink_rows(shape_id)
            .into_iter()
            .map(|r| (r.addr, r.sub, r.desc))
            .collect()
    }

    /// The shape's hyperlink rows with their origin (#139): instance
    /// rows first, then the master's (shadowed ixs dropped). The
    /// master lookup walks its top-level shapes; a multi-shape master
    /// with same-ix Hyperlink rows across sub-shapes is beyond v1
    /// (the blank-catalog masters are single-shape).
    pub(super) fn hyperlink_rows(&self, shape_id: i64) -> Vec<HyperRow> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.get(self.page_idx) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return Vec::new();
        };
        let collect = |shapes: &[Shape]| {
            shapes
                .iter()
                .flat_map(|s| s.sheet.sections_named("Hyperlink"))
                .flat_map(|sec| sec.rows.iter())
                .map(|r| HyperRow {
                    ix: r.ix,
                    inherited: false,
                    addr: r
                        .cell("Address")
                        .and_then(|c| c.v.clone())
                        .unwrap_or_default(),
                    sub: r
                        .cell("SubAddress")
                        .and_then(|c| c.v.clone())
                        .unwrap_or_default(),
                    desc: r
                        .cell("Description")
                        .and_then(|c| c.v.clone())
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>()
        };
        let mut out = collect(std::slice::from_ref(sh));
        if let Some(m) = sh.master.and_then(|id| ed.doc.master(id)) {
            let shadowed: std::collections::HashSet<i64> = out.iter().map(|r| r.ix).collect();
            out.extend(
                collect(&m.shapes)
                    .into_iter()
                    .filter(|r| !shadowed.contains(&r.ix))
                    .map(|mut r| {
                        r.inherited = true;
                        r
                    }),
            );
        }
        out
    }

    /// Follow a hyperlink (#28): a SubAddress naming a page navigates
    /// the document; anything else opens through the OS (fire-and-
    /// forget). Returns whether a link existed.
    pub(super) fn follow_first_hyperlink(&mut self, shape_id: i64) -> bool {
        let Some((addr, sub, _)) = self.shape_hyperlinks(shape_id).into_iter().next() else {
            return false;
        };
        self.follow_hyperlink(&addr, &sub);
        true
    }

    pub(super) fn follow_hyperlink(&mut self, address: &str, sub: &str) {
        let address = address.trim();
        let sub = sub.trim();
        if !sub.is_empty() {
            // GOTOPAGE-class navigation: SubAddress names a page.
            let target = self
                .ed()
                .and_then(|ed| ed.doc.pages.iter().position(|p| p.name == sub));
            match target {
                Some(i) => {
                    self.switch_page(i);
                    self.status = crate::i18n::strs().msg_went_to_page(sub);
                    return;
                }
                None => {
                    self.status = crate::i18n::strs().msg_page_not_found(sub);
                    return;
                }
            }
        }
        if address.is_empty() {
            self.status = crate::i18n::strs().msg_the_link_has_no_address();
            return;
        }
        // A stored relative address opens against the document's
        // directory (#71): the link survives the file and its folder
        // moving together.
        let address =
            crate::hyperlink::resolve_address(address, crate::hyperlink::doc_dir(&self.path));
        #[cfg(target_os = "linux")]
        let open = "xdg-open";
        #[cfg(target_os = "macos")]
        let open = "open";
        #[cfg(target_os = "windows")]
        let open = "cmd";
        #[cfg(target_os = "windows")]
        let flag = "/C";
        #[cfg(not(target_os = "windows"))]
        let flag = "";
        let mut cmd = std::process::Command::new(open);
        if !flag.is_empty() {
            cmd.arg(flag);
        }
        match cmd.arg(&address).spawn() {
            Ok(_) => self.status = crate::i18n::strs().msg_opened_address(address.clone()),
            Err(e) => {
                self.status =
                    crate::i18n::strs().msg_could_not_open_address_e(address, e.to_string())
            }
        }
    }
}

impl App {
    /// The Hyperlinks dialog (#28): rows of the selected shape with
    /// add / edit / remove through the command layer (one undo per
    /// change) and Go to follow a link. The draft lives OUTSIDE the
    /// window closure (a per-frame clone would drop typed text — the
    /// retired text-edit-window pattern).
    pub(super) fn add_comment(&mut self, shape_id: i64, body: &str) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let Some(ed) = self.ed() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        let g = |n: &str| {
            ed.doc
                .resolve_cell(sh, &chain, n)
                .and_then(|c| c.numeric())
                .unwrap_or(0.0)
        };
        let (px, py) = (g("PinX"), g("PinY"));
        let (w, h) = (g("Width"), g("Height"));
        let (lx, ly) = (g("LocPinX"), g("LocPinY"));
        let (x1, y1) = (px - lx + w, py - ly + h);

        // The Annotation layer, created on first use; LayerMember
        // references the layer row's IX (Visio's convention).
        let rows = dc_core::layers::layer_rows(&ed.doc, page_id).unwrap_or_default();
        let (layer_cmd, layer_ix) = match rows
            .iter()
            .find(|r| dc_core::layers::layer_name(r) == Some("Annotation"))
        {
            Some(r) => (None, r.ix),
            None => (
                Some(dc_core::commands::AddLayer::new(page_id, "Annotation")),
                rows.iter().map(|r| r.ix).max().unwrap_or(-1) + 1,
            ),
        };
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let author = self.comment_author();
        let date = format_now_utc();
        let text = format!("{} — {}: {}", author, date, body.trim());
        // Replies stack under the thread's first bubble (#29 rework).
        let thread_len = self.comments_for(shape_id).len();
        let stack = 0.55 * thread_len as f64;

        let set = |sheet: &mut vsdx::model::Sheet, n: &str, v: &str| {
            sheet.set_cell(n, vsdx::model::Cell::value(v.to_string()));
        };
        let (aw, ah) = (1.8_f64, 0.5_f64);
        let mut sheet = vsdx::model::Sheet::default();
        set(&mut sheet, "PinX", &format!("{:.4}", x1 + 0.1));
        set(
            &mut sheet,
            "PinY",
            &format!("{:.4}", ((y1 + 0.1) - stack).max(ah)),
        );
        set(&mut sheet, "Width", &format!("{aw}"));
        set(&mut sheet, "Height", &format!("{ah}"));
        set(&mut sheet, "LocPinX", "0");
        set(&mut sheet, "LocPinY", "0");
        set(&mut sheet, "LayerMember", &layer_ix.to_string());
        set(&mut sheet, "FillForegnd", "#FFF3C2");
        sheet.text = Some(text.clone());
        sheet.text_raw = Some(
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        );
        let row = |ix: i64, t: &str, x: &str, y: &str| vsdx::model::Row {
            ix,
            t: Some(t.into()),
            cells: vec![
                ("X".into(), vsdx::model::Cell::value(x.to_string())),
                ("Y".into(), vsdx::model::Cell::value(y.to_string())),
            ],
            ..vsdx::model::Row::default()
        };
        sheet
            .children
            .push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                name: "Geometry".into(),
                ix: 0,
                rows: vec![
                    row(1, "MoveTo", "0", "0"),
                    row(2, "LineTo", &format!("{aw}"), "0"),
                    row(3, "LineTo", &format!("{aw}"), &format!("{ah}")),
                    row(4, "LineTo", "0", &format!("{ah}")),
                    row(5, "LineTo", "0", "0"),
                ],
                ..vsdx::model::Section::default()
            }));
        let annotation = Shape {
            id,
            name_u: Some(format!("Comment.{id}")),
            type_: Some("Shape".into()),
            sheet,
            ..Shape::default()
        };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        if let Some(c) = layer_cmd {
            cmds.push(Box::new(c));
        }
        cmds.push(Box::new(AddShape {
            page: page_id,
            parent: None,
            shape: annotation,
        }));
        let user_row = |name: &str, val: &str| {
            Box::new(dc_core::SetRowCell::new(
                Scope::Shape {
                    page: page_id,
                    shape: id,
                },
                "User",
                0,
                dc_core::commands::RowSel::Named(name.into()),
                "Value",
                vsdx::model::Cell::value(val.to_string()),
            )) as Box<dyn dc_core::Command>
        };
        cmds.push(user_row("AnnotationRef", &shape_id.to_string()));
        cmds.push(user_row("AnnotationAuthor", &author));
        cmds.push(user_row("AnnotationDate", &date));
        self.apply(dc_core::Macro::new(
            crate::i18n::strs().msg_comment_add_inline(),
            cmds,
        ));
    }

    /// The comment author: the General page's user name (File ▸
    /// Options, #268) when set, else the environment (the pre-#268
    /// behavior), else "user".
    pub(super) fn comment_author(&self) -> String {
        let name = self.options_store.user_name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".into())
    }

    /// The comment-avatar initials for one annotation author (#268):
    /// the General page's initials for the configured user when set,
    /// else derived from the name — the first letter of up to three
    /// words, uppercase (Visio's avatar derives them the same way).
    pub(super) fn author_initials(&self, author: &str) -> String {
        if author == self.comment_author() {
            let ini = self.options_store.user_initials.trim();
            if !ini.is_empty() {
                return ini.to_string();
            }
        }
        author
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(3)
            .map(|c| c.to_uppercase().to_string())
            .collect()
    }
}

impl App {
    /// The shape's comment thread (#29): annotation shapes whose
    /// `User.AnnotationRef` names it, oldest first — (annotation id,
    /// author, date, body).
    pub(super) fn comments_for(&self, shape_id: i64) -> Vec<(i64, String, String, String)> {
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.get(self.page_idx) else {
            return Vec::new();
        };
        let user_val = |s: &Shape, row: &str| -> Option<String> {
            s.sheet
                .sections_named("User")
                .iter()
                .find_map(|sec| sec.named_row(row))
                .and_then(|r| r.cell("Value"))
                .and_then(|c| c.v.clone())
        };
        let mut out: Vec<(i64, String, String, String)> = page
            .shapes
            .iter()
            .filter(|s| {
                user_val(s, "AnnotationRef").as_deref() == Some(shape_id.to_string().as_str())
            })
            .map(|s| {
                let text = s.sheet.text.clone().unwrap_or_default();
                let author = user_val(s, "AnnotationAuthor").unwrap_or_default();
                let date = user_val(s, "AnnotationDate").unwrap_or_default();
                // Older bubbles carry "author — date: body" in the text
                // block; strip the header when the cells agree.
                let body = text
                    .strip_prefix(&format!("{author} — {date}: "))
                    .unwrap_or(&text)
                    .to_string();
                (s.id, author, date, body)
            })
            .collect();
        out.sort_by_key(|(id, _, _, _)| *id);
        out
    }

    /// The Comments popup (#29 rework, after the shipped 2016
    /// reference): one card per comment — author + date header, body,
    /// Edit / Delete — with a Reply field at the bottom. The drafts
    /// live outside the window closure (the #28 lesson).
    pub(super) fn comment_window(&mut self, ctx: &egui::Context) {
        let Some(shape_id) = self.comment_target else {
            return;
        };
        let alive = self
            .ed()
            .and_then(|ed| ed.doc.pages.get(self.page_idx))
            .is_some_and(|p| vsdx::model::find_shape_by_id(&p.shapes, shape_id).is_some());
        if !alive {
            self.comment_target = None;
            self.comment_edit = None;
            self.comment_body.clear();
            return;
        }
        let Some(page_id) = self.page_id() else {
            return;
        };
        let thread = self.comments_for(shape_id);
        // #268: the avatar initials per row (the General page's
        // initials for the configured user, derived from the name
        // otherwise) — computed before the window closure so the
        // render loop doesn't borrow self.
        let initials: Vec<String> =
            thread.iter().map(|(_, a, _, _)| self.author_initials(a)).collect();
        let mut body = std::mem::take(&mut self.comment_body);
        let mut edit = self.comment_edit.take();
        let mut close = false;
        egui::Window::new(crate::i18n::strs().msg_comments())
            .resizable(false)
            .collapsible(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                // #257 item 3: the dialog pass mirrors the window.
                ui.with_layout(dialog_dir(), |ui| {
                    if thread.is_empty() {
                        ui.weak(crate::i18n::strs().msg_no_comments_on_this_shape_yet());
                    }
                    for ((ann_id, author, date, cbody), initial) in
                        thread.iter().zip(&initials)
                    {
                        let card = egui::Frame::new()
                            .inner_margin(egui::Margin::same(6))
                            .stroke(egui::Stroke::new(
                                1.0,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            ))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                // 2016 header: avatar initial + author + date
                                // (mirrored with the chrome under RTL).
                                row(ui, |ui| {
                                    // #268: the avatar shows the row's
                                    // initials (the General page's
                                    // setting for the configured
                                    // user, name-derived otherwise).
                                    let (c, _) =
                                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    ui.painter().circle_filled(
                                        c.center(),
                                        8.0,
                                        ui.visuals().selection.bg_fill,
                                    );
                                    // #304: the initials FIT the avatar
                                    // (the full author name stays in
                                    // the header beside it).
                                    let (fitted, size) =
                                        fit_initials_to_avatar(ui.ctx(), initial);
                                    ui.painter().text(
                                        c.center(),
                                        egui::Align2::CENTER_CENTER,
                                        fitted,
                                        egui::FontId::proportional(size),
                                        Color32::WHITE,
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{author}  {date}"))
                                            .small()
                                            .weak(),
                                    );
                                });
                                let editing_here =
                                    edit.as_ref().is_some_and(|(id, _)| id == ann_id);
                                let mut edit_buf = edit
                                    .as_mut()
                                    .filter(|(id, _)| id == ann_id)
                                    .map(|(_, b)| std::mem::take(b));
                                let mut edit_done = false;
                                if editing_here {
                                    let buf = edit_buf.as_mut().unwrap();
                                    let r = ui.add(
                                        egui::TextEdit::multiline(buf)
                                            .desired_rows(2)
                                            .desired_width(f32::INFINITY),
                                    );
                                    r.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::TextInput,
                                            true,
                                            crate::i18n::strs().msg_edit_comment(),
                                        )
                                    });
                                    row(ui, |ui| {
                                        if ui.button(crate::i18n::strs().msg_save()).clicked()
                                            && !buf.trim().is_empty()
                                        {
                                            let author = author.clone();
                                            let date = date.clone();
                                            let new_text =
                                                format!("{} — {}: {}", author, date, buf.trim());
                                            // One undo step incl. text-derived
                                            // cache refresh (#147).
                                            let scope = Scope::Shape {
                                                page: page_id,
                                                shape: *ann_id,
                                            };
                                            let doc = self.ed().map(|ed| &ed.doc);
                                            if let Some(doc) = doc {
                                                self.apply(dc::textfunc::text_edit_macro(
                                                    doc, &scope, &new_text,
                                                ));
                                            }
                                            edit_done = true;
                                        }
                                        if ui.button(crate::i18n::strs().msg_cancel()).clicked() {
                                            edit_done = true;
                                        }
                                    });
                                }
                                if !editing_here {
                                    ui.label(egui::RichText::new(cbody).size(12.0));
                                    row(ui, |ui| {
                                        if ui.button(crate::i18n::strs().msg_edit()).clicked() {
                                            edit = Some((*ann_id, cbody.clone()));
                                        }
                                        if ui.button(crate::i18n::strs().msg_delete()).clicked() {
                                            self.apply(RemoveShape::new(page_id, *ann_id));
                                        }
                                    });
                                }
                                // Write the (possibly typed-into) buffer
                                // back unless the editor closed.
                                if let Some(b) = edit_buf {
                                    if let Some((id, slot)) = edit.as_mut() {
                                        if id == ann_id && !edit_done {
                                            *slot = b;
                                        }
                                    }
                                }
                            });
                        let _ = card;
                        ui.add_space(4.0);
                    }
                    ui.separator();
                    // Reply (or the first comment when the thread is empty).
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut body)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text(if thread.is_empty() {
                                crate::i18n::strs().msg_add_a_comment()
                            } else {
                                crate::i18n::strs().msg_reply()
                            }),
                    );
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            true,
                            if thread.is_empty() {
                                crate::i18n::strs().msg_review_new_comment()
                            } else {
                                crate::i18n::strs().msg_reply()
                            },
                        )
                    });
                    row(ui, |ui| {
                        let caption = if thread.is_empty() {
                            crate::i18n::strs().msg_ok()
                        } else {
                            crate::i18n::strs().msg_reply()
                        };
                        if ui.button(caption).clicked() && !body.trim().is_empty() {
                            self.add_comment(shape_id, &body);
                            body.clear();
                        }
                        if ui.button(crate::i18n::strs().msg_close()).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if !close {
            self.comment_body = body;
            self.comment_edit = edit;
        } else {
            // Close means Close: drop the target too, or the window
            // re-renders next frame (#29 live report).
            self.comment_target = None;
            self.comment_edit = None;
            self.comment_body = String::new();
        }
    }
}
