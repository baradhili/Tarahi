//! Pictures: OS clipboard paste, the insert-picture flow, and
//! copy-as-picture (PNG/SVG export of the selection).

use super::*;

impl App {
    /// #113 (URS-1113): probe the OS clipboard for an image on a
    /// worker thread; the result lands via `os_image_rx` over the
    /// next frames and inserts as a picture.
    pub(super) fn paste_os_image(&mut self) {
        if self.os_image_rx.is_some() {
            return; // one probe at a time
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let img: Option<OsImage> = std::panic::catch_unwind(arboard::Clipboard::new)
                .ok()
                .and_then(|cb| cb.ok())
                .and_then(|mut cb| {
                    cb.get_image().ok().map(|i| {
                        // RGBA bytes, row-major, top-down.
                        let w = i.width;
                        let h = i.height;
                        let mut bytes = Vec::with_capacity(w * h * 4);
                        for row in 0..h {
                            let line = &i.bytes[row * w * 4..(row + 1) * w * 4];
                            bytes.extend_from_slice(line);
                        }
                        (w as u32, h as u32, bytes)
                    })
                });
            let _ = tx.send(img);
        });
        self.os_image_rx = Some(rx);
        self.status = crate::i18n::strs().msg_checking_the_clipboard_for_a_pic();
    }

    /// #113: poll the OS image probe; a hit inserts as a picture.
    pub(super) fn poll_os_image(&mut self) {
        let Some(rx) = self.os_image_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Some((w, h, rgba))) => {
                // Encode PNG so the insert path's sniffer accepts it.
                if let Some(img) = image::RgbaImage::from_raw(w, h, rgba) {
                    let mut png = Vec::new();
                    if image::DynamicImage::ImageRgba8(img)
                        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                        .is_ok()
                    {
                        self.insert_picture_bytes(&png);
                        return;
                    }
                }
                self.status = crate::i18n::strs().msg_clipboard_image_could_not_be_enc();
            }
            Ok(None) => {
                self.status = crate::i18n::strs().msg_no_picture_on_the_clipboard();
            }
            Err(mpsc::TryRecvError::Empty) => {
                // still probing: keep the receiver and retry next frame
                self.os_image_rx = Some(rx);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = crate::i18n::strs().msg_clipboard_probe_failed();
            }
        }
    }
}

impl App {
    /// #113 (URS-1012): open the picture picker (PNG/JPEG).
    pub(super) fn open_picture_dialog(&mut self) {
        self.begin_dialog(PendingKind::OpenPicture);
    }

    /// #113 (URS-1012): a picked picture file becomes a Foreign shape
    /// centered on the page at natural size (pixels/96 in, capped at
    /// 8 in per side) — the media part, the page rels entry, and the
    /// content-type default all ride `extra_media` to the writer.
    pub(super) fn insert_picture_file(&mut self, path: &Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.insert_picture_bytes(&bytes),
            Err(e) => {
                self.status = crate::i18n::strs()
                    .msg_could_not_read_path(path.display().to_string(), e.to_string())
            }
        }
    }

    /// The picture-insertion core (also the kittest path: bytes in,
    /// no native dialog).
    pub(super) fn insert_picture_bytes(&mut self, bytes: &[u8]) {
        let Some(page_id) = self.page_id() else {
            return;
        };
        let geom = self.page_geom();
        let Some(ed) = self.ed.as_mut() else { return };
        // Format sniff: PNG/JPEG magic.
        let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            "png"
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "jpeg"
        } else {
            self.status = crate::i18n::strs().msg_only_png_and_jpeg_pictures_are_s();
            return;
        };
        let compression = if ext == "png" { "PNG" } else { "JPG" };
        let Ok(img) = image::load_from_memory(bytes) else {
            self.status = crate::i18n::strs().msg_the_picture_could_not_be_decoded();
            return;
        };
        // Natural size, capped.
        let (mw, mh) = (img.width() as f64, img.height() as f64);
        let mut w = mw / 96.0;
        let mut h = mh / 96.0;
        let cap = 8.0_f64;
        if w.max(h) > cap {
            let k = cap / w.max(h);
            w *= k;
            h *= k;
        }
        let (w, h) = (w.max(0.1), h.max(0.1));
        let id = dc_core::commands::next_shape_id(&ed.doc, page_id);
        let mut sh = vsdx::model::Shape {
            id,
            type_: Some("Foreign".into()),
            name_u: Some(format!("Picture.{id}")),
            ..vsdx::model::Shape::default()
        };
        let mut set = |n: &str, v: f64| {
            sh.sheet
                .set_cell(n, vsdx::model::Cell::value(format!("{v:.6}")));
        };
        set("PinX", geom.w_in / 2.0);
        set("PinY", geom.h_in / 2.0);
        set("Width", w);
        set("Height", h);
        set("LocPinX", w / 2.0);
        set("LocPinY", h / 2.0);
        set("Angle", 0.0);
        set("FlipX", 0.0);
        set("FlipY", 0.0);
        set("ResizeMode", 0.0);
        // The image fills the shape box.
        let mut geo = vsdx::model::Section {
            name: "Geometry".into(),
            ix: 0,
            ..vsdx::model::Section::default()
        };
        for (n, v) in [("NoFill", "1"), ("NoLine", "0"), ("NoShow", "0")] {
            geo.cells.push((n.to_string(), vsdx::model::Cell::value(v)));
        }
        for (ix, t, x, y) in [
            (1i64, "MoveTo", 0.0, 0.0),
            (2, "LineTo", w, 0.0),
            (3, "LineTo", w, h),
            (4, "LineTo", 0.0, h),
            (5, "LineTo", 0.0, 0.0),
        ] {
            geo.rows.push(vsdx::model::Row {
                t: Some(t.into()),
                ix,
                cells: vec![
                    ("X".to_string(), vsdx::model::Cell::value(format!("{x:.6}"))),
                    ("Y".to_string(), vsdx::model::Cell::value(format!("{y:.6}"))),
                ],
                ..vsdx::model::Row::default()
            });
        }
        sh.sheet
            .children
            .push(vsdx::model::SheetChild::Section(geo));
        // Media part + rel id: next free across the package and
        // already-added media; the rid must not collide with the
        // page part's existing rels.
        let page = ed.doc.pages.iter().find(|p| p.id == page_id).expect("page");
        let part = page.part.clone();
        let n = ed
            .doc
            .extra_media
            .iter()
            .map(|m| m.part.clone())
            .chain(
                ed.doc
                    .package
                    .as_ref()
                    .map(|p| {
                        p.part_names()
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
            .filter_map(|name| {
                name.rsplit('/')
                    .next()
                    .and_then(|f| f.strip_prefix("image"))
                    .and_then(|rest| rest.split('.').next())
                    .and_then(|n| n.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let media_part = format!("visio/media/image{n}.{ext}");
        let rid_n = ed
            .doc
            .package
            .as_ref()
            .map(|p| p.relationships(&part))
            .unwrap_or_default()
            .iter()
            .filter_map(|(id, _, _)| id.strip_prefix("rId").and_then(|v| v.parse::<u32>().ok()))
            .chain(
                // Pictures already inserted on this page own rids the
                // package does not know (CodeRabbit: the second
                // picture would collide and render the first's bytes).
                ed.doc
                    .extra_media
                    .iter()
                    .filter(|m| m.page == page_id)
                    .filter_map(|m| {
                        m.rid
                            .strip_prefix("rId")
                            .and_then(|v| v.parse::<u32>().ok())
                    }),
            )
            .max()
            .unwrap_or(0)
            + 1;
        let rid = format!("rId{rid_n}");
        sh.raw_children.push(vsdx::model::RawEl {
            name: "ForeignData".into(),
            attrs: vec![
                ("Type".to_string(), "Bitmap".into()),
                ("ForeignType".to_string(), "Bitmap".into()),
                ("CompressionType".to_string(), compression.into()),
                ("r:id".to_string(), rid.clone()),
            ],
            inner: String::new(),
        });
        let _ = &mut ed.doc;
        // Own the page borrow scope.
        let doc = &mut ed.doc;
        doc.extra_media.push(vsdx::model::MediaRef {
            part: media_part,
            bytes: bytes.to_vec(),
            page: page_id,
            rid,
            ext: ext.to_string(),
        });
        let shape = sh;
        self.apply(dc_core::AddShape {
            page: page_id,
            parent: None,
            shape,
        });
        self.set_selection(Some((page_id, id)));
        self.sel_set = vec![(page_id, id)];
        self.status = crate::i18n::strs().msg_picture_inserted(mw as i64, mh as i64, w, h);
    }

    /// #113 (URS-1200): render the selection as a PNG at `factor`
    /// (1 screen, 2 high, 4 print): the full-page SVG with every
    /// non-selected shape hidden, rasterized, then cropped to the
    /// selection's bounds (the page-SVG viewBox is always the whole
    /// page). The page raster is capped at 4096 px per side.
    pub(super) fn picture_png(&self, factor: u32) -> Option<Vec<u8>> {
        let ed = self.ed.as_ref()?;
        let page_id = self.page_id()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sel: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, s)| *s)
            .collect();
        if sel.is_empty() {
            return None;
        }
        // Selection bounds (page inches, y-up → y-down for the crop):
        // union of the rects' corner pairs.
        let geom = self.page_geom();
        let mut b: Option<(f64, f64, f64, f64)> = None;
        for sid in &sel {
            let sh = vsdx::model::find_shape_by_id(&page.shapes, *sid)?;
            let r = self.shape_rect(&ed.doc, page_id, sh)?;
            let corners = (r.0, r.1, r.0 + r.2, r.1 + r.3);
            b = Some(match b {
                None => corners,
                Some((x0, y0, x1, y1)) => (
                    x0.min(corners.0),
                    y0.min(corners.1),
                    x1.max(corners.2),
                    y1.max(corners.3),
                ),
            });
        }
        let (bx0, by0, bx1, by1) = b?;
        // Hide every other shape.
        let hidden: Vec<i64> = page
            .shapes
            .iter()
            .map(|s| s.id)
            .filter(|id| !sel.contains(id))
            .collect();
        let svg = dc::svg::render_page_with(&ed.doc, page, &hidden).ok()?;
        let tree = crate::svgimg::parse_svg(&svg, crate::perf::Kind::Thumb).ok()?;
        // Raster the whole page, capped, then crop the selection.
        let max_side = 4096.0;
        let mut f = factor as f64;
        if geom.w_in * 96.0 * f > max_side || geom.h_in * 96.0 * f > max_side {
            f = (max_side / (96.0 * geom.w_in.max(geom.h_in))).min(f);
        }
        let f = f as f32;
        let img = crate::svgimg::render_tree(
            &tree,
            geom.w_in as f32 * 96.0 * f,
            crate::perf::Kind::Thumb,
        )
        .ok()?;
        let (iw, ih) = (img.width() as f64, img.height() as f64);
        let sx = iw / geom.w_in;
        let sy = ih / geom.h_in;
        let px0 = ((bx0) * sx).floor().max(0.0) as u32;
        let py0 = ((geom.h_in - by1) * sy).floor().max(0.0) as u32; // y-down
        let px1 = ((bx1) * sx).ceil().min(iw) as u32;
        let py1 = ((geom.h_in - by0) * sy).ceil().min(ih) as u32;
        let (cw, ch) = (
            px1.saturating_sub(px0).max(1),
            py1.saturating_sub(py0).max(1),
        );
        let mut out = image::RgbaImage::new(cw, ch);
        let (iw_us, ih_us) = (img.width(), img.height());
        for row in 0..ch {
            for col in 0..cw {
                let sx = ((px0 + col) as usize).min(iw_us - 1);
                let sy = ((py0 + row) as usize).min(ih_us - 1);
                let px = img[(sx, sy)]; // egui::Color32 (sRGB, premult)
                let [r, g, b, a] = px.to_srgba_unmultiplied();
                out.put_pixel(col, row, image::Rgba([r, g, b, a]));
            }
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(out)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .ok()?;
        Some(bytes)
    }
}

impl App {
    pub(super) fn copy_picture_window(&mut self, ctx: &egui::Context) {
        if !self.copypic_open {
            return;
        }
        let mut open = true;
        let mut copy = false;
        egui::Window::new(crate::i18n::strs().msg_copy_as_picture())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.with_layout(dialog_dir(), |ui| {
                    ui.strong(crate::i18n::strs().msg_resolution());
                    if ui
                        .radio(self.copypic_svg, crate::i18n::strs().msg_vector_svg())
                        .clicked()
                    {
                        self.copypic_svg = true;
                    }
                    for (f, label) in [
                        (1u32, crate::i18n::strs().msg_screen_96_dpi()),
                        (2, crate::i18n::strs().msg_high_192_dpi()),
                        (4, crate::i18n::strs().msg_print_384_dpi()),
                    ] {
                        if ui
                            .radio(!self.copypic_svg && self.copypic_factor == f, label)
                            .clicked()
                        {
                            self.copypic_factor = f;
                            self.copypic_svg = false;
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button(crate::i18n::strs().msg_copy_picture()).clicked() {
                        copy = true;
                    }
                    ui.weak(if self.copypic_svg {
                        crate::i18n::strs().msg_the_svg_markup_lands_on_the_clip()
                    } else {
                        crate::i18n::strs().msg_the_picture_lands_on_the_clipboa()
                    });
                });
            });
        if copy && self.copypic_svg {
            match self.picture_svg() {
                Some(svg) => {
                    let n = svg.len();
                    self.last_picture = Some(svg.clone().into_bytes());
                    ctx.copy_text(svg);
                    self.status = crate::i18n::strs().msg_svg_copied_n_bytes_vector_paste(n);
                }
                None => self.status = crate::i18n::strs().msg_nothing_selected_to_copy(),
            }
        }
        if copy && !self.copypic_svg {
            match self.picture_png(self.copypic_factor) {
                Some(bytes) => {
                    self.last_picture = Some(bytes.clone());
                    match image::load_from_memory(&bytes) {
                        Ok(dynimg) => {
                            let rgba = dynimg.to_rgba8();
                            let (w, h) = (rgba.width(), rgba.height());
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [w as usize, h as usize],
                                &rgba.into_raw(),
                            );
                            {
                                ctx.copy_image(ci);
                                self.status = crate::i18n::strs().msg_picture_copied(
                                    w as i64,
                                    h as i64,
                                    self.copypic_factor as i64,
                                );
                            }
                        }
                        Err(e) => {
                            self.status =
                                crate::i18n::strs().msg_copy_as_picture_failed_e(e.to_string())
                        }
                    }
                }
                None => self.status = crate::i18n::strs().msg_nothing_selected_to_copy(),
            }
        }
        if !open {
            self.copypic_open = false;
        }
    }

    /// #113 remainder: the vector copy — the page rendered with every
    /// non-selected shape hidden, the viewBox retargeted to the
    /// selection bounds, as SVG markup for the text clipboard (the
    /// interchange egui can carry; raster flavors stay on the image
    /// clipboard).
    pub(super) fn picture_svg(&self) -> Option<String> {
        let ed = self.ed.as_ref()?;
        let page_id = self.page_id()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sel: Vec<i64> = self
            .sel_set
            .iter()
            .filter(|(p, _)| *p == page_id)
            .map(|(_, s)| *s)
            .collect();
        if sel.is_empty() {
            return None;
        }
        let geom = self.page_geom();
        let mut b: Option<(f64, f64, f64, f64)> = None;
        for sid in &sel {
            let sh = vsdx::model::find_shape_by_id(&page.shapes, *sid)?;
            let r = self.shape_rect(&ed.doc, page_id, sh)?;
            let corners = (r.0, r.1, r.0 + r.2, r.1 + r.3);
            b = Some(match b {
                None => corners,
                Some((x0, y0, x1, y1)) => (
                    x0.min(corners.0),
                    y0.min(corners.1),
                    x1.max(corners.2),
                    y1.max(corners.3),
                ),
            });
        }
        let (bx0, by0, bx1, by1) = b?;
        let hidden: Vec<i64> = page
            .shapes
            .iter()
            .map(|s| s.id)
            .filter(|id| !sel.contains(id))
            .collect();
        let svg = dc::svg::render_page_with(&ed.doc, page, &hidden).ok()?;
        // Rebuild the ROOT element around the selection's box (page
        // y-up → the svg's y-down frame), padding a hair to keep
        // strokes intact. The root is written fresh — token-stripping
        // the emitted one would split its space-bearing viewBox value
        // and leave orphan tokens (invalid XML; CodeRabbit).
        let pad = 0.06;
        let (vx, vy, vw, vh) = (
            (bx0 - pad).max(0.0),
            (geom.h_in - by1 - pad).max(0.0),
            (bx1 - bx0 + 2.0 * pad).min(geom.w_in),
            (by1 - by0 + 2.0 * pad).min(geom.h_in),
        );
        let root_end = svg.find('>').map(|i| i + 1)?;
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{vx:.4} {vy:.4} {vw:.4} {vh:.4}\" width=\"{pw:.0}\" height=\"{ph:.0}\">",
            pw = vw * 96.0,
            ph = vh * 96.0,
        );
        out.push_str(&svg[root_end..]);
        Some(out)
    }
}
