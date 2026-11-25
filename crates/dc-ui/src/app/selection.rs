//! Selection geometry: hit testing, page/selection rects, handle
//! layout, and Geometry-row vertex editing.

use super::*;

impl App {
    /// AABB in page coords (inches, y-up): (x, y, w, h). Nested-aware
    /// (#78): a top-level shape keeps its plain local rect (its parent
    /// frame is the page); a group member maps its parent-local rect
    /// through the renderer's XForm chain, so frames and hit boxes
    /// land on the drawn pixels even under rotated/flipped groups.
    pub(super) fn shape_rect(&self, doc: &Document, page_id: i64, s: &Shape) -> Option<(f64, f64, f64, f64)> {
        let Ok(chain) = shape_chain(doc, page_id, s.id) else {
            return Self::shape_rect_static(doc, page_id, s);
        };
        if chain.len() <= 1 {
            return Self::shape_rect_static(doc, page_id, s);
        }
        let page_h = doc
            .pages
            .iter()
            .find(|p| p.id == page_id)?
            .sheet()
            .and_then(|sh| sh.cell("PageHeight"))
            .and_then(|c| c.numeric())
            .unwrap_or(11.0);
        dc::svg::shape_page_rect(doc, &chain, page_h)
    }

    /// Borrow-free bbox (page inches, y-up) for marquee gating.
    /// Normalized: the corpus connector model carries SIGNED
    /// Width/Height (`GUARD(EndX-BeginX)`), so the origin is min-corner
    /// and the extents absolute (#108).
    pub(super) fn shape_rect_static(doc: &Document, page_id: i64, s: &Shape) -> Option<(f64, f64, f64, f64)> {
        let chain = shape_chain(doc, page_id, s.id).ok()?;
        let get = |n: &str| doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
        let (pin_x, pin_y) = (get("PinX")?, get("PinY")?);
        let w = get("Width").unwrap_or(1.0);
        let h = get("Height").unwrap_or(1.0);
        let lx = get("LocPinX").unwrap_or(w / 2.0);
        let ly = get("LocPinY").unwrap_or(h / 2.0);
        let (x, y) = (pin_x - lx, pin_y - ly);
        Some((x.min(x + w), y.min(y + h), w.abs(), h.abs()))
    }

    pub(super) fn shape_hit(&self, page_pos: (f64, f64)) -> Option<i64> {
        let ed = self.ed()?;
        // Guides sit above shapes in hit priority (#113): a press on
        // the line (within the screen threshold) grabs the guide, per
        // Visio. Not while group-editing — guides are page furniture.
        if self.group_edit.is_empty() {
            let threshold = 5.0 / self.view.px_per_in() as f64;
            if let Some(g) = crate::guides::guide_hit(&self.page_guides(), page_pos, threshold) {
                return Some(g.id);
            }
        }
        let page = &ed.doc.pages[self.page_idx.min(ed.doc.pages.len().saturating_sub(1))];
        // Connectors hit by their INK, not a degenerate bbox (#108 —
        // the corpus model's bbox can be a zero-height band). #172: the
        // ink follows the GEOMETRY rows — the Begin→End chord first
        // (the common straight route), then the row walk (arcs leave
        // the chord; a belly click would otherwise land on whatever
        // shape's bbox overlaps it). Master instances count (#172):
        // `ObjType=2` often lives on the master.
        if self.group_edit.is_empty() {
            let threshold = 5.0 / self.view.px_per_in() as f64;
            for shape in page.shapes.iter().rev() {
                if let Ok(chain) = shape_chain(&ed.doc, page.id, shape.id) {
                    if !crate::connectors::is_connector_in(&ed.doc, &chain, shape) {
                        continue;
                    }
                    let g = |n: &str| {
                        ed.doc
                            .resolve_cell(shape, &chain, n)
                            .and_then(|c| c.numeric())
                    };
                    if let (Some(bx), Some(by), Some(ex), Some(ey)) =
                        (g("BeginX"), g("BeginY"), g("EndX"), g("EndY"))
                    {
                        if segment_hit((bx, by), (ex, ey), page_pos, threshold) {
                            return Some(shape.id);
                        }
                    }
                    // The geometry walk (master root rows when the
                    // instance carries none), mapped through the
                    // chain-resolved bbox.
                    let (root, msize) = shape
                        .master
                        .and_then(|mid| {
                            ed.doc.master(mid).and_then(|m| {
                                m.shapes.first().map(|root| {
                                    let size = (
                                        root.sheet
                                            .cell("Width")
                                            .and_then(|c| c.numeric())
                                            .unwrap_or(1.0),
                                        root.sheet
                                            .cell("Height")
                                            .and_then(|c| c.numeric())
                                            .unwrap_or(1.0),
                                    );
                                    (root, size)
                                })
                            })
                        })
                        .map(|(r, s)| (Some(r), s))
                        .unwrap_or((None, (1.0, 1.0)));
                    // #179: the full placement frame (rotation +
                    // flips) — a rotated connector's ink lives outside
                    // its unrotated rect, so the gate uses the
                    // rotated corner union.
                    let frame = crate::connectors::hit_frame(&ed.doc, &chain, shape, msize);
                    let (bx0, by0, bx1, by1) = frame.page_bounds();
                    if page_pos.0 >= bx0 - threshold
                        && page_pos.0 <= bx1 + threshold
                        && page_pos.1 >= by0 - threshold
                        && page_pos.1 <= by1 + threshold
                    {
                        // #294: connector ink strips stored jump arcs —
                        // the hit walk agrees with the rendered ink,
                        // with the SAME radius evidence the strip uses.
                        let strip_jumps =
                            crate::connectors::is_connector_in(&ed.doc, &chain, shape);
                        let radius = crate::connectors::jump_radii_in(&ed.doc, page);
                        let segs = crate::connectors::geometry_hit_segments(
                            shape,
                            root,
                            &frame,
                            if strip_jumps { Some(radius) } else { None },
                        );
                        for (a, b) in segs {
                            if segment_hit(a, b, page_pos, threshold) {
                                return Some(shape.id);
                            }
                        }
                    }
                }
            }
        }
        // Group-edit mode (#78): only the innermost open group's
        // members are on the canvas — clicks resolve against them
        // (nested-aware rects), never falling through to page shapes.
        if let Some(g) = self.group_edit.last().copied() {
            let group = vsdx::model::find_shape_by_id(&page.shapes, g)?;
            let mut members = Vec::new();
            collect_members(&group.sub_shapes, &mut members);
            for m in members.iter().rev() {
                if let Some((x, y, w, h)) = self.shape_rect(&ed.doc, page.id, m) {
                    if hit_in_rect(page_pos, x, y, w, h) {
                        return Some(m.id);
                    }
                }
            }
            return None;
        }
        for shape in page.shapes.iter().rev() {
            // #179: a rotated shape's claim uses its ROTATED bounds —
            // the unrotated Pin/Width/Height rect of a rotated box is
            // far bigger than the visible ink and steals clicks from
            // thin connectors crossing the corners (utilisation.vsdx).
            let rotated = shape
                .sheet
                .cell("Angle")
                .and_then(|c| c.numeric())
                .filter(|a| a.abs() > 1e-6)
                .is_some()
                || shape_chain(&ed.doc, page.id, shape.id)
                    .ok()
                    .and_then(|ch| {
                        ed.doc
                            .resolve_cell(shape, &ch, "Angle")
                            .and_then(|c| c.numeric())
                    })
                    .is_some_and(|a| a.abs() > 1e-6);
            if rotated {
                let Ok(chain) = shape_chain(&ed.doc, page.id, shape.id) else {
                    continue;
                };
                let (x0, y0, x1, y1) =
                    crate::connectors::hit_frame(&ed.doc, &chain, shape, (1.0, 1.0)).page_bounds();
                if page_pos.0 >= x0 && page_pos.0 <= x1 && page_pos.1 >= y0 && page_pos.1 <= y1 {
                    return Some(shape.id);
                }
            } else if let Some((x, y, w, h)) = self.shape_rect(&ed.doc, page.id, shape) {
                if hit_in_rect(page_pos, x, y, w, h) {
                    return Some(shape.id);
                }
            }
        }
        None
    }

    /// Selection frame in canvas rect space (inches, y-down origin).
    pub(super) fn sel_extra_page_rects(&self) -> Vec<Rect> {
        let Some(ed) = self.ed.as_ref() else {
            return Vec::new();
        };
        let Some((page_id, primary)) = self.sel else {
            return Vec::new();
        };
        let page_h = self.page_geom().h_in;
        self.sel_set
            .iter()
            .filter(|(p, s)| *p == page_id && *s != primary)
            .filter_map(|(_, sid)| {
                let page = ed.doc.pages.get(self.page_idx)?;
                let sh = vsdx::model::find_shape_by_id(&page.shapes, *sid)?;
                let (x, y, w, h) = Self::shape_rect_static(&ed.doc, page_id, sh)?;
                Some(Rect::from_min_size(
                    Pos2::new(x as f32, (page_h - y - h) as f32),
                    Vec2::new(w as f32, h as f32),
                ))
            })
            .collect()
    }

    /// The selection's union bounds in page inches y-up `(x, y, w, h)`
    /// (#237): the primary selection plus every multi-select member
    /// (the same shape rects the selection frames draw). Selected
    /// guides draw as their overlay and carry no frame, so they do
    /// not zoom-target either.
    pub(super) fn sel_union_page_rect(&self) -> Option<(f64, f64, f64, f64)> {
        let ed = self.ed.as_ref()?;
        let (page_id, primary) = self.sel?;
        let page = ed.doc.pages.get(self.page_idx)?;
        let rect_of = |sid: i64| -> Option<(f64, f64, f64, f64)> {
            let sh = vsdx::model::find_shape_by_id(&page.shapes, sid)?;
            if crate::guides::is_guide(sh) {
                return None;
            }
            // The nested-aware rect (#218 review round 3): a
            // group-edit selection's members carry parent-local
            // cells, and the raw Pin/Width read zooms to the wrong
            // spot.
            self.shape_rect(&ed.doc, page_id, sh)
        };
        let mut acc = rect_of(primary);
        for (p, sid) in &self.sel_set {
            if *p != page_id {
                continue;
            }
            if let Some(r) = rect_of(*sid) {
                acc = Some(match acc {
                    None => r,
                    Some((ax, ay, aw, ah)) => {
                        let (bx, by, bw, bh) = r;
                        let x0 = ax.min(bx);
                        let y0 = ay.min(by);
                        let x1 = (ax + aw).max(bx + bw);
                        let y1 = (ay + ah).max(by + bh);
                        (x0, y0, x1 - x0, y1 - y0)
                    }
                });
            }
        }
        acc
    }

    pub(super) fn sel_page_rect(&self) -> Option<Rect> {
        let ed = self.ed()?;
        let (page_id, shape_id) = self.sel?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let shape = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        // A selected guide draws as the guide overlay itself (#113):
        // no selection frame, no handles.
        if crate::guides::is_guide(shape) {
            return None;
        }
        let (x, y, w, h) = self.shape_rect(&ed.doc, page_id, shape)?;
        let geom = self.page_geom();
        Some(Rect::from_min_size(
            Pos2::new(x as f32, (geom.h_in - y - h) as f32),
            Vec2::new(w as f32, h as f32),
        ))
    }

    /// The grab handles for the primary selection (GitHub #59). A 1D
    /// shape (resolved `BeginX`/`EndX` — the XForm1D marker) gets a
    /// handle at each endpoint plus the midpoint; everything else gets
    /// the four resize corners of its bounding box.
    pub(super) fn selection_handles(&self) -> Vec<Handle> {
        // Group-edit mode (#95): member resize/endpoint handles work —
        // 2D corners come from `sel_page_rect`, which already maps a
        // member's parent-local rect through the renderer's XForm chain
        // to page space; 1D endpoints map their parent-local cells to
        // page below. The commit-side deltas convert through the same
        // chain (apply_resize / apply_line_endpoint).
        let Some(ed) = self.ed() else {
            return Vec::new();
        };
        let Some((page_id, shape_id)) = self.sel else {
            return Vec::new();
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return Vec::new();
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return Vec::new();
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return Vec::new();
        };
        // Guides drag by their body (#113) — no resize/endpoint
        // handles (tilting a guide is not a thing).
        if crate::guides::is_guide(sh) {
            return Vec::new();
        }
        let nested = chain.len() >= 2;
        let page_h = Self::page_height(ed, page_id);
        let to_page = |p: (f64, f64)| -> (f64, f64) {
            if nested {
                dc::svg::parent_local_point_to_page(&ed.doc, &chain, page_h, p)
            } else {
                p
            }
        };
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        if let (Some(bx), Some(by), Some(ex), Some(ey)) =
            (get("BeginX"), get("BeginY"), get("EndX"), get("EndY"))
        {
            // #202: a CubicBezTo row (the curved route) reshapes the
            // handle set — the midpoint seats ON the curve (t=0.5;
            // the chord midpoint hangs off it) and both Bézier
            // controls become draggable curve handles, positioned and
            // later committed through the same HitFrame resolution
            // the renderer and hit walk use.
            let msize = sh
                .master
                .and_then(|mid| ed.doc.master(mid))
                .and_then(|m| m.shapes.first())
                .map(|r| {
                    let g = |n: &str| {
                        ed.doc
                            .resolve_cell(r, &[r], n)
                            .and_then(|c| c.numeric())
                            .unwrap_or(1.0)
                    };
                    (g("Width"), g("Height"))
                })
                .unwrap_or((1.0, 1.0));
            let frame = crate::connectors::hit_frame(&ed.doc, &chain, sh, msize);
            let cubic = crate::connectors::cubic_row(sh, &frame);
            // #226: the arc masters (the corpus Line-arc connector) seat
            // the midpoint ON the rendered curve the same way — the
            // quadratic's t=0.5 through the same resolution the ink
            // draws with. Both mids are shape-local.
            let master_root = sh
                .master
                .and_then(|mid| ed.doc.master(mid))
                .and_then(|m| m.shapes.first());
            let arc = crate::connectors::arc_row(sh, master_root, &frame);
            let mid_pos = if let Some(c) = cubic.as_ref() {
                frame.to_page(c.mid())
            } else if let Some(a) = arc.as_ref() {
                frame.to_page(a.mid())
            } else {
                to_page(((bx + ex) / 2.0, (by + ey) / 2.0))
            };
            let mut out = vec![
                Handle {
                    id: HandleId::LineBegin,
                    pos: to_page((bx, by)),
                },
                Handle {
                    id: HandleId::LineEnd,
                    pos: to_page((ex, ey)),
                },
                Handle {
                    id: HandleId::LineMid,
                    pos: mid_pos,
                },
            ];
            if let Some(c) = cubic {
                out.push(Handle {
                    id: HandleId::GeoPoint {
                        section_ix: c.section_ix,
                        row_ix: c.row_ix,
                        cell: canvas::GeoCell::Ctrl,
                    },
                    pos: frame.to_page(c.ctrl1),
                });
                out.push(Handle {
                    id: HandleId::GeoPoint {
                        section_ix: c.section_ix,
                        row_ix: c.row_ix,
                        cell: canvas::GeoCell::Ctrl2,
                    },
                    pos: frame.to_page(c.ctrl2),
                });
            }
            // #203: spline-family rows on a connector — SplineStart/
            // SplineKnot/NURBSTo are point rows; their resolved
            // endpoints (and the NURBSTo E formula's interior
            // points) join the handles through the same frame. Own
            // rows only (master-inherited rows have no local cells
            // to drag — the #137 rule).
            for geo in sh.sheet.sections_named("Geometry") {
                for row in &geo.rows {
                    let Some(rt) = row.t.as_deref() else {
                        continue;
                    };
                    if !matches!(rt, "SplineStart" | "SplineKnot" | "NURBSTo") {
                        continue;
                    }
                    let cell = |n: &str| -> f64 {
                        crate::connectors::row_cell_value(row.cell(n), None, 1.0, &frame.env)
                            .unwrap_or(0.0)
                    };
                    let (rx, ry) = (cell("X"), cell("Y"));
                    out.push(Handle {
                        id: HandleId::GeoPoint {
                            section_ix: geo.ix,
                            row_ix: row.ix,
                            cell: canvas::GeoCell::X,
                        },
                        pos: frame.to_page((rx, ry)),
                    });
                    if rt == "NURBSTo" {
                        let e_text = row
                            .cell("E")
                            .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                            .unwrap_or_default();
                        if let Some(f) = dc::curve::parse_nurbs(&e_text) {
                            for (i, (gx, gy, _, _)) in f.points.iter().enumerate() {
                                // Per-axis type gates, matching the drag
                                // commit's rule: an axis with type 0 is
                                // fractional (scale by its dimension),
                                // an absolute axis keeps the raw value —
                                // otherwise the handle jumps on the
                                // first drag (#185 review).
                                out.push(Handle {
                                    id: HandleId::GeoPoint {
                                        section_ix: geo.ix,
                                        row_ix: row.ix,
                                        cell: canvas::GeoCell::NurbsPt(i),
                                    },
                                    pos: frame.to_page((
                                        if f.x_type == 0.0 {
                                            gx * frame.size.0.max(1e-6)
                                        } else {
                                            *gx
                                        },
                                        if f.y_type == 0.0 {
                                            gy * frame.size.1.max(1e-6)
                                        } else {
                                            *gy
                                        },
                                    )),
                                });
                            }
                        }
                    }
                }
            }
            return out;
        }
        let Some(r) = self.sel_page_rect() else {
            return Vec::new();
        };
        let page_h = self.page_geom().h_in;
        // The rect is y-down page inches; handles are y-up.
        let mut out: Vec<Handle> = [
            (canvas::Corner::TopLeft, r.left(), r.top()),
            (canvas::Corner::TopRight, r.right(), r.top()),
            (canvas::Corner::BottomLeft, r.left(), r.bottom()),
            (canvas::Corner::BottomRight, r.right(), r.bottom()),
        ]
        .into_iter()
        .map(|(c, x, y)| Handle {
            id: HandleId::Corner(c),
            pos: (x as f64, page_h - y as f64),
        })
        .collect();
        // Geometry vertex handles (#137): one per point row of the
        // shape's OWN Geometry sections (master-inherited geometry has
        // no local rows to drag — its handles arrive with master
        // editing, #122). Appended after the corners, so corner
        // handles win overlaps. #157: RELATIVE rows join (their X/Y
        // are fractions of Width/Height), and curve rows expose their
        // CONTROLS — an ArcTo's sagitta (a circle at the apex) and an
        // EllipticalArcTo's on-curve A/B point (NURBS/spline controls
        // stay unhanded: the renderer draws them as chords, so a
        // control drag would show no feedback).
        if let Some((bx, by, bw, bh)) = self.shape_rect(&ed.doc, page_id, sh) {
            let (w, h) = (bw.max(1e-6), bh.max(1e-6));
            for geo in sh.sheet.sections_named("Geometry") {
                let mut cur: Option<(f64, f64)> = None; // local end of the previous row
                for row in &geo.rows {
                    if row.is_deleted() {
                        continue; // deleted rows stay deleted (#293) — no handles for invisible ink
                    }
                    let t = row.t.as_deref().unwrap_or("");
                    let rel = t.starts_with("Rel");
                    let (Some(x), Some(y)) = (
                        row.cell("X").and_then(|c| c.numeric()),
                        row.cell("Y").and_then(|c| c.numeric()),
                    ) else {
                        continue;
                    };
                    let (lx, ly) = if rel { (x * w, y * h) } else { (x, y) };
                    let point = matches!(
                        t,
                        "MoveTo"
                            | "RelMoveTo"
                            | "LineTo"
                            | "RelLineTo"
                            | "ArcTo"
                            | "RelArcTo"
                            | "EllipticalArcTo"
                            | "RelEllipticalArcTo"
                            | "NURBSTo"
                            | "SplineKnot"
                            | "SplineStart"
                    );
                    if point {
                        for cell in [canvas::GeoCell::X, canvas::GeoCell::Y] {
                            out.push(Handle {
                                id: HandleId::GeoPoint {
                                    section_ix: geo.ix,
                                    row_ix: row.ix,
                                    cell,
                                },
                                pos: (bx + lx, by + ly),
                            });
                        }
                    }
                    // #157 curve controls.
                    match t {
                        "ArcTo" | "RelArcTo" => {
                            if let (Some(a), Some(c0)) =
                                (row.cell("A").and_then(|c| c.numeric()), cur)
                            {
                                let apex = canvas::arc_apex(c0, (lx, ly), a);
                                out.push(Handle {
                                    id: HandleId::GeoPoint {
                                        section_ix: geo.ix,
                                        row_ix: row.ix,
                                        cell: canvas::GeoCell::Apex,
                                    },
                                    pos: (bx + apex.0, by + apex.1),
                                });
                            }
                        }
                        "EllipticalArcTo" => {
                            if let (Some(a), Some(b)) = (
                                row.cell("A").and_then(|c| c.numeric()),
                                row.cell("B").and_then(|c| c.numeric()),
                            ) {
                                out.push(Handle {
                                    id: HandleId::GeoPoint {
                                        section_ix: geo.ix,
                                        row_ix: row.ix,
                                        cell: canvas::GeoCell::OnCurve,
                                    },
                                    pos: (bx + a, by + b),
                                });
                            }
                        }
                        "NURBSTo" => {
                            // #188: the E formula's interior control
                            // points (fractions of Width/Height when
                            // the x/y types are 0) join the row's own
                            // X/Y vertex handles as circles.
                            let e_text = row
                                .cell("E")
                                .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                                .unwrap_or_default();
                            if let Some(f) = dc::curve::parse_nurbs(&e_text) {
                                // Per-axis type gates, matching the drag
                                // commit: x_type gates X by Width,
                                // y_type gates Y by Height (#185 review).
                                let fx = if f.x_type == 0.0 { w } else { 1.0 };
                                let fy = if f.y_type == 0.0 { h } else { 1.0 };
                                for (i, (gx, gy, _, _)) in f.points.iter().enumerate() {
                                    out.push(Handle {
                                        id: HandleId::GeoPoint {
                                            section_ix: geo.ix,
                                            row_ix: row.ix,
                                            cell: canvas::GeoCell::NurbsPt(i),
                                        },
                                        pos: (bx + gx * fx, by + gy * fy),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                    cur = Some((lx, ly));
                }
            }
        }
        out
    }

    /// #202: the selected 1D shape carries a CURVE row — the
    /// selection layer then skips its straight begin→end chord (the
    /// ink IS the line; a straight chord over the curve reads as
    /// "the drag flattened it"). #226: generalised past the #202
    /// CubicBezTo check to every curve row type — the corpus Line-arc
    /// masters shape their whole ink as one EllipticalArcTo, and the
    /// chord-over-arc reads exactly the same way.
    pub(super) fn sel_1d_curved(&self) -> bool {
        let Some(ed) = self.ed() else {
            return false;
        };
        let Some((page_id, shape_id)) = self.sel else {
            return false;
        };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return false;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return false;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return false;
        };
        // The chord stroke only draws for line handles (1D shapes).
        if ed.doc.resolve_cell(sh, &chain, "BeginX").is_none() {
            return false;
        }
        // All-straight rows (the #176 line-frame set) keep the chord;
        // anything else — EllipticalArcTo, CubicBezTo, splines, NURBS,
        // polylines — is curve ink. The instance's own section when it
        // carries one, else the master root's (what renders).
        let straight = |t: &str| matches!(t, "" | "MoveTo" | "RelMoveTo" | "LineTo" | "RelLineTo");
        let curved = |rows: &[vsdx::model::Row]| {
            if rows
                .iter()
                .any(|r| r.t.as_deref().map(|t| !straight(t)).unwrap_or(false))
            {
                return true;
            }
            // A multi-SEGMENT polyline is not the chord either (#218
            // review): the right-angle route's elbow rows drew the
            // selection stroke diagonally over the bend. Degenerate
            // rows — the straight route's repeated endpoint — stay
            // straight.
            let mut cur: Option<(f64, f64)> = None;
            let mut segments = 0usize;
            for r in rows {
                let t = r.t.as_deref().unwrap_or("");
                let x = r.cell("X").and_then(|c| c.numeric()).unwrap_or(0.0);
                let y = r.cell("Y").and_then(|c| c.numeric()).unwrap_or(0.0);
                match t {
                    "MoveTo" | "RelMoveTo" => cur = Some((x, y)),
                    "LineTo" | "RelLineTo" => {
                        let same = cur
                            .is_some_and(|(cx, cy)| (cx - x).abs() < 1e-9 && (cy - y).abs() < 1e-9);
                        if !same {
                            segments += 1;
                        }
                        cur = Some((x, y));
                    }
                    _ => {}
                }
            }
            segments > 1
        };
        if let Some(g) = sh.sheet.sections_named("Geometry").first() {
            return curved(&g.rows);
        }
        let master_root = sh
            .master
            .and_then(|mid| ed.doc.master(mid))
            .and_then(|m| m.shapes.first());
        master_root
            .and_then(|r| r.sheet.sections_named("Geometry").into_iter().next())
            .is_some_and(|g| curved(&g.rows))
    }

    /// #137/#157: commit a geometry-handle drag — rewrite the row's
    /// X/Y (shape-local, y-up; RELATIVE rows keep their fractions,
    /// resolved ×Width/Height), an ArcTo's sagitta from the pointer,
    /// or an EllipticalArcTo's on-curve A/B — through one `SetRowCell`
    /// Macro. The #59 guard policy: a written cell that resolves to a
    /// formula refuses with a status note and is never written.
    pub(super) fn apply_geo_vertex(
        &mut self,
        section_ix: i64,
        row_ix: i64,
        cell: canvas::GeoCell,
        to_page: (f64, f64),
    ) {
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(page) = ed.doc.pages.iter().find(|p| p.id == page_id) else {
            return;
        };
        let Some(sh) = vsdx::model::find_shape_by_id(&page.shapes, shape_id) else {
            return;
        };
        let Ok(chain) = shape_chain(&ed.doc, page_id, shape_id) else {
            return;
        };
        // Page → shape-local: the inverse of the handle mapping (the
        // chain-mapped bbox's origin; unrotated v1).
        let Some((bx, by, _, _)) = self.shape_rect(&ed.doc, page_id, sh) else {
            return;
        };
        let Some(row) = sh
            .sheet
            .sections_named("Geometry")
            .iter()
            .find(|s| s.ix == section_ix)
            .and_then(|s| s.rows.iter().find(|r| r.ix == row_ix))
        else {
            return;
        };
        let t = row.t.as_deref().unwrap_or("");
        // #157: RELATIVE rows resolve their X/Y as fractions of the
        // shape's Width/Height (the hit walk's convention) — the drag
        // writes the fraction, keeping the row relative.
        let rel = t.starts_with("Rel");
        // The cells this drag writes (guards check each).
        let cells: &[&'static str] = match cell {
            canvas::GeoCell::X | canvas::GeoCell::Y => &["X", "Y"],
            canvas::GeoCell::Apex => &["A"],
            canvas::GeoCell::OnCurve => &["A", "B"],
            // #202: a CubicBezTo row's two Bézier controls.
            canvas::GeoCell::Ctrl => &["A", "B"],
            canvas::GeoCell::Ctrl2 => &["C", "D"],
            // #188: the NURBSTo row's E formula (all controls live in
            // it); the drag re-emits it with the moved group point.
            canvas::GeoCell::NurbsPt(_) => &["E"],
        };
        // #202/#188: curve-control drags CUSTOMIZE the route — the
        // controls are route formulas by default (`Width*0.5`…, the
        // NURBS() E text), and keeping the formula would make the
        // renderer ignore the dragged value (it prefers evaluable
        // formulas). So these cells are exempt from the #157 formula
        // refusal: the drag replaces the formula with the dragged
        // value (a status note says so).
        let customizes = matches!(
            cell,
            canvas::GeoCell::Ctrl | canvas::GeoCell::Ctrl2 | canvas::GeoCell::NurbsPt(_)
        );
        if !customizes {
            for name in cells {
                if dc_core::commands::effective_formula(
                    &ed.doc,
                    sh,
                    &chain,
                    &format!("Geometry{section_ix}.{name}{row_ix}"),
                )
                .is_some()
                {
                    self.status = crate::i18n::strs()
                        .msg_vertex_drag_refused(format!("Geometry{section_ix}.{name}{row_ix}"));
                    return;
                }
                // Row-cell formulas live on the row cell itself too.
                if row
                    .cell(name)
                    .and_then(|c| c.f.as_deref())
                    .is_some_and(|f| f != "Inh" && f != "No Formula")
                {
                    self.status = crate::i18n::strs()
                        .msg_vertex_drag_refused(format!("row {row_ix}'s {name}"));
                    return;
                }
            }
        }
        let scope = Scope::Shape {
            page: page_id,
            shape: shape_id,
        };
        let cellv = |v: f64| vsdx::Cell::value(format!("{v:.6}"));
        let (px, py) = (to_page.0 - bx, to_page.1 - by); // shape-local
                                                         // #203: 1D shapes (connectors) map drags through the HitFrame
                                                         // local frame — the bbox the 2D mapping uses starts at a
                                                         // different origin than the geometry rows for a rotated/
                                                         // signed-frame connector.
        let one_d = ed.doc.resolve_cell(sh, &chain, "BeginX").is_some();
        let hit = if one_d {
            let msize = sh
                .master
                .and_then(|mid| ed.doc.master(mid))
                .and_then(|m| m.shapes.first())
                .map(|r| {
                    let g = |n: &str| {
                        ed.doc
                            .resolve_cell(r, &[r], n)
                            .and_then(|c| c.numeric())
                            .unwrap_or(1.0)
                    };
                    (g("Width"), g("Height"))
                })
                .unwrap_or((1.0, 1.0));
            Some(crate::connectors::hit_frame(&ed.doc, &chain, sh, msize))
        } else {
            None
        };
        let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
        match cell {
            canvas::GeoCell::X | canvas::GeoCell::Y => {
                let (lx, ly) = if let Some(hf) = &hit {
                    // #203: 1D geometry rows are LOCAL-absolute in the
                    // connector frame.
                    hf.to_local(to_page)
                } else if rel {
                    let Some((_, _, bw, bh)) = self.shape_rect(&ed.doc, page_id, sh) else {
                        return;
                    };
                    (px / bw.max(1e-6), py / bh.max(1e-6))
                } else {
                    (px, py)
                };
                for (name, v) in [("X", lx), ("Y", ly)] {
                    cmds.push(Box::new(dc_core::SetRowCell::new(
                        scope.clone(),
                        "Geometry",
                        section_ix,
                        dc_core::commands::RowSel::Ix(row_ix),
                        name,
                        cellv(v),
                    )));
                }
            }
            canvas::GeoCell::Apex => {
                // The row's chord: its own end and the previous row's.
                let (ex, ey) = (
                    row.cell("X").and_then(|c| c.numeric()).unwrap_or(0.0),
                    row.cell("Y").and_then(|c| c.numeric()).unwrap_or(0.0),
                );
                let (ex, ey) = if rel {
                    let Some((_, _, bw, bh)) = self.shape_rect(&ed.doc, page_id, sh) else {
                        return;
                    };
                    (ex * bw, ey * bh)
                } else {
                    (ex, ey)
                };
                // The chord's start: the PREVIOUS row's local end
                // (Rel rows resolve ×Width/Height).
                let size = self
                    .shape_rect(&ed.doc, page_id, sh)
                    .map(|r| (r.2.max(1e-6), r.3.max(1e-6)))
                    .unwrap_or((1.0, 1.0));
                let prev: (f64, f64) = {
                    let secs = sh.sheet.sections_named("Geometry");
                    let sec = secs.iter().find(|s| s.ix == section_ix);
                    let mut p = (0.0, 0.0);
                    if let Some(sec) = sec {
                        for r in &sec.rows {
                            if r.ix == row_ix {
                                break;
                            }
                            let t = r.t.as_deref().unwrap_or("");
                            let (x, y) = (
                                r.cell("X").and_then(|c| c.numeric()).unwrap_or(0.0),
                                r.cell("Y").and_then(|c| c.numeric()).unwrap_or(0.0),
                            );
                            if t.starts_with("Rel") {
                                p = (x * size.0, y * size.1);
                            } else {
                                p = (x, y);
                            }
                        }
                    }
                    p
                };
                let a = canvas::arc_sagitta_from_pointer(prev, (ex, ey), (px, py));
                cmds.push(Box::new(dc_core::SetRowCell::new(
                    scope,
                    "Geometry",
                    section_ix,
                    dc_core::commands::RowSel::Ix(row_ix),
                    "A",
                    cellv(a),
                )));
            }
            canvas::GeoCell::OnCurve => {
                for (name, v) in [("A", px), ("B", py)] {
                    cmds.push(Box::new(dc_core::SetRowCell::new(
                        scope.clone(),
                        "Geometry",
                        section_ix,
                        dc_core::commands::RowSel::Ix(row_ix),
                        name,
                        cellv(v),
                    )));
                }
            }
            canvas::GeoCell::Ctrl | canvas::GeoCell::Ctrl2 => {
                // #202: map the pointer through the connector's
                // HitFrame (Pin/LocPin frame, rotation included) — NOT
                // the bbox origin the X/Y drags use; a connector's
                // row frame starts at its begin corner.
                let msize = sh
                    .master
                    .and_then(|mid| ed.doc.master(mid))
                    .and_then(|m| m.shapes.first())
                    .map(|r| {
                        let g = |n: &str| {
                            ed.doc
                                .resolve_cell(r, &[r], n)
                                .and_then(|c| c.numeric())
                                .unwrap_or(1.0)
                        };
                        (g("Width"), g("Height"))
                    })
                    .unwrap_or((1.0, 1.0));
                let frame = crate::connectors::hit_frame(&ed.doc, &chain, sh, msize);
                let (lx, ly) = frame.to_local(to_page);
                for (name, v) in [(cells[0], lx), (cells[1], ly)] {
                    // A literal write (f: None) REPLACES the cell —
                    // the route formula yields to the dragged value.
                    cmds.push(Box::new(dc_core::SetRowCell::new(
                        scope.clone(),
                        "Geometry",
                        section_ix,
                        dc_core::commands::RowSel::Ix(row_ix),
                        name,
                        cellv(v),
                    )));
                }
            }
            canvas::GeoCell::NurbsPt(idx) => {
                // #188: the E formula IS the data — parse it, move the
                // group's point to the drag (fractions of the bbox
                // extents when xType/yType are 0), re-emit, write the
                // cell with the formula text kept.
                let e_text = row
                    .cell("E")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .unwrap_or_default();
                let Some(mut f) = dc::curve::parse_nurbs(&e_text) else {
                    return;
                };
                if idx >= f.points.len() {
                    return;
                }
                // #203: 1D shapes map the drop through the HitFrame —
                // 2D shapes keep the bbox mapping. Per-axis type gates
                // (the render arm's rule): only an axis whose type is
                // 0 is fractional and divides by its dimension; an
                // absolute axis keeps the local drop value.
                let (gx, gy, drop) = if let Some(hf) = &hit {
                    let local = hf.to_local(to_page);
                    (hf.size.0.max(1e-6), hf.size.1.max(1e-6), local)
                } else {
                    let Some((_, _, bw, bh)) = self.shape_rect(&ed.doc, page_id, sh) else {
                        return;
                    };
                    (bw.max(1e-6), bh.max(1e-6), (px, py))
                };
                f.points[idx].0 = if f.x_type == 0.0 { drop.0 / gx } else { drop.0 };
                f.points[idx].1 = if f.y_type == 0.0 { drop.1 / gy } else { drop.1 };
                let text = dc::curve::render_nurbs(&f);
                cmds.push(Box::new(dc_core::SetRowCell::new(
                    scope,
                    "Geometry",
                    section_ix,
                    dc_core::commands::RowSel::Ix(row_ix),
                    "E",
                    vsdx::Cell {
                        v: Some(text.clone()),
                        f: Some(text),
                        ..vsdx::Cell::default()
                    },
                )));
            }
        }
        self.apply(Macro::new(
            crate::i18n::strs().msg_undo_move_vertex(row_ix),
            cmds,
        ));
        if customizes {
            if let canvas::GeoCell::NurbsPt(_) = cell {
                self.status = crate::i18n::strs().msg_nurbs_control_customized_the_e_f();
            } else {
                self.status = crate::i18n::strs().msg_curve_control_customized_row_row(row_ix);
            }
        }
    }

    /// kittest: one shape's bbox in SCREEN coords (y-down) — follows
    /// the live pan/zoom, for drag tests that must press the shape
    /// where it actually is (#59).
    #[doc(hidden)]
    pub fn debug_shape_screen_rect_for_test(&self, shape_id: i64) -> Option<egui::Rect> {
        self.shape_screen_rect(shape_id)
    }

    /// One shape's bbox in SCREEN coords (y-down), for anchoring the
    /// context menu beside the shape (issue #49).
    pub(super) fn shape_screen_rect(&self, shape_id: i64) -> Option<egui::Rect> {
        let ed = self.ed()?;
        let page_id = self.page_id()?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let shape = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let (x, y, w, h) = self.shape_rect(&ed.doc, page_id, shape)?;
        let geom = self.page_geom();
        let ppi = self.view.px_per_in();
        let min = Pos2::new(
            self.view.pan.x + x as f32 * ppi,
            self.view.pan.y + (geom.h_in - y - h) as f32 * ppi,
        );
        Some(egui::Rect::from_min_size(
            min,
            Vec2::new(w as f32 * ppi, h as f32 * ppi),
        ))
    }

    // -- interaction ------------------------------------------------------
}
