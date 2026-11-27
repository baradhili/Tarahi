//! The Size & Position window (#210, URS-1065): Visio's task pane for
//! the primary selection. A 2-D shape lists X, Y, Width, Height and
//! Angle; a 1-D shape (resolved Begin/End, the XForm1D marker) lists
//! Begin X/Y, End X/Y, Length and Angle. Every field shows the resolved
//! cell in the display unit, and a committed edit writes through the
//! same command paths as the handles: one undo step each, guarded cells
//! (a formula, or a Protection lock) grayed and never written.

use super::*;

/// One editable row of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SizePosField {
    X,
    Y,
    Width,
    Height,
    Angle,
    BeginX,
    BeginY,
    EndX,
    EndY,
    Length,
}

impl SizePosField {
    fn label(self) -> String {
        let s = crate::i18n::strs();
        match self {
            SizePosField::X => s.msg_sp_x(),
            SizePosField::Y => s.msg_sp_y(),
            SizePosField::Width => s.msg_sp_width(),
            SizePosField::Height => s.msg_sp_height(),
            SizePosField::Angle => s.msg_sp_angle(),
            SizePosField::BeginX => s.msg_sp_begin_x(),
            SizePosField::BeginY => s.msg_sp_begin_y(),
            SizePosField::EndX => s.msg_sp_end_x(),
            SizePosField::EndY => s.msg_sp_end_y(),
            SizePosField::Length => s.msg_sp_length(),
        }
    }
    fn is_angle(self) -> bool {
        self == SizePosField::Angle
    }
}

/// The resolved transform of the primary selection.
struct SizePosState {
    one_d: bool,
    /// A group member: its Pin/Begin/End cells are parent-local, so
    /// the position rows show them but stay read-only here.
    member: bool,
    cells: std::collections::HashMap<&'static str, f64>,
}

impl App {
    fn size_pos_state(&self) -> Option<SizePosState> {
        let ed = self.ed()?;
        let (page_id, shape_id) = self.sel?;
        let page = ed.doc.pages.iter().find(|p| p.id == page_id)?;
        let sh = vsdx::model::find_shape_by_id(&page.shapes, shape_id)?;
        let chain = shape_chain(&ed.doc, page_id, shape_id).ok()?;
        let get = |n: &str| ed.doc.resolve_cell(sh, &chain, n).and_then(|c| c.numeric());
        let mut cells = std::collections::HashMap::new();
        for n in [
            "PinX", "PinY", "Width", "Height", "Angle", "BeginX", "BeginY", "EndX", "EndY",
        ] {
            if let Some(v) = get(n) {
                cells.insert(n, v);
            }
        }
        let one_d = ["BeginX", "BeginY", "EndX", "EndY"]
            .iter()
            .all(|n| cells.contains_key(n));
        Some(SizePosState {
            one_d,
            member: chain.len() >= 2,
            cells,
        })
    }

    /// The refusal reason for writing `field`, or `None` when it is
    /// free: a formula on the cell it writes, or the Protection lock
    /// that covers it (the corner/endpoint handles' rules).
    fn size_pos_guard(&self, st: &SizePosState, field: SizePosField) -> Option<String> {
        let member_ro = || Some(crate::i18n::strs().msg_sp_member_position_read_only());
        match field {
            SizePosField::X | SizePosField::Y if st.member => member_ro(),
            SizePosField::BeginX
            | SizePosField::BeginY
            | SizePosField::EndX
            | SizePosField::EndY
            | SizePosField::Length
                if st.member =>
            {
                member_ro()
            }
            SizePosField::X => self.line_drag_guard(&["PinX"], &["LockMoveX"]),
            SizePosField::Y => self.line_drag_guard(&["PinY"], &["LockMoveY"]),
            SizePosField::Width => self.resize_guard("Width", "LockWidth"),
            SizePosField::Height => self.resize_guard("Height", "LockHeight"),
            SizePosField::Angle if st.one_d => {
                self.line_drag_guard(&["EndX", "EndY"], &["LockEnd", "LockRotate"])
            }
            SizePosField::Angle => self.line_drag_guard(&["Angle"], &["LockRotate"]),
            SizePosField::BeginX | SizePosField::BeginY => {
                self.line_drag_guard(&["BeginX", "BeginY"], &["LockBegin"])
            }
            SizePosField::EndX | SizePosField::EndY | SizePosField::Length => {
                self.line_drag_guard(&["EndX", "EndY"], &["LockEnd"])
            }
        }
    }

    /// The field's current value in display units (degrees for Angle).
    fn size_pos_value(&self, st: &SizePosState, field: SizePosField) -> Option<f64> {
        let c = |n: &str| st.cells.get(n).copied();
        let len = |v: f64| self.view.unit.from_in(v);
        Some(match field {
            SizePosField::X => len(c("PinX")?),
            SizePosField::Y => len(c("PinY")?),
            SizePosField::Width => len(c("Width")?),
            SizePosField::Height => len(c("Height")?),
            SizePosField::BeginX => len(c("BeginX")?),
            SizePosField::BeginY => len(c("BeginY")?),
            SizePosField::EndX => len(c("EndX")?),
            SizePosField::EndY => len(c("EndY")?),
            SizePosField::Length => {
                len((c("EndX")? - c("BeginX")?).hypot(c("EndY")? - c("BeginY")?))
            }
            SizePosField::Angle if st.one_d => (c("EndY")? - c("BeginY")?)
                .atan2(c("EndX")? - c("BeginX")?)
                .to_degrees(),
            SizePosField::Angle => c("Angle").unwrap_or(0.0).to_degrees(),
        })
    }

    /// Commit one field edit (`v` in display units / degrees) through
    /// the command path the handles use — one undo step.
    pub(crate) fn size_pos_commit(&mut self, field: SizePosField, v: f64) {
        let Some(st) = self.size_pos_state() else {
            return;
        };
        if !v.is_finite() {
            return;
        }
        if let Some(reason) = self.size_pos_guard(&st, field) {
            self.status = crate::i18n::strs().msg_sp_refused(reason);
            return;
        }
        let Some((page_id, shape_id)) = self.sel else {
            return;
        };
        let inch = self.view.unit.to_in(v);
        let c = |n: &str| st.cells.get(n).copied().unwrap_or(0.0);
        match field {
            SizePosField::X | SizePosField::Y => {
                let pin = if field == SizePosField::X {
                    (inch, c("PinY"))
                } else {
                    (c("PinX"), inch)
                };
                let mut cmds = vec![self.move_cmd_for(page_id, shape_id, pin)];
                if let Some(r) = self.reroute_cmd(page_id, &[shape_id]) {
                    cmds.push(r);
                }
                self.apply(Macro::new(
                    crate::i18n::strs().msg_sp_undo_position(shape_id),
                    cmds,
                ));
            }
            SizePosField::Width => self.resize_core(|_, h| (inch, h), None),
            SizePosField::Height => self.resize_core(|w, _| (w, inch), None),
            SizePosField::Angle if !st.one_d => {
                let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(SetCell::new(
                    Scope::Shape {
                        page: page_id,
                        shape: shape_id,
                    },
                    "Angle",
                    vsdx::Cell::value(format!("{:.8}", v.to_radians())),
                ))];
                if let Some(r) = self.reroute_cmd(page_id, &[shape_id]) {
                    cmds.push(r);
                }
                self.apply(Macro::new(
                    crate::i18n::strs().msg_sp_undo_angle(shape_id),
                    cmds,
                ));
            }
            SizePosField::BeginX => {
                self.apply_line_endpoint(HandleId::LineBegin, (inch, c("BeginY")))
            }
            SizePosField::BeginY => {
                self.apply_line_endpoint(HandleId::LineBegin, (c("BeginX"), inch))
            }
            SizePosField::EndX => self.apply_line_endpoint(HandleId::LineEnd, (inch, c("EndY"))),
            SizePosField::EndY => self.apply_line_endpoint(HandleId::LineEnd, (c("EndX"), inch)),
            // Length and Angle of a line keep the begin point and move
            // the end along / around it.
            SizePosField::Length | SizePosField::Angle => {
                let (bx, by) = (c("BeginX"), c("BeginY"));
                let (ex, ey) = (c("EndX"), c("EndY"));
                let (len, ang) = if field == SizePosField::Length {
                    (inch.max(1e-4), (ey - by).atan2(ex - bx))
                } else {
                    ((ex - bx).hypot(ey - by), v.to_radians())
                };
                self.apply_line_endpoint(
                    HandleId::LineEnd,
                    (bx + len * ang.cos(), by + len * ang.sin()),
                );
            }
        }
    }

    /// The window itself (View ▸ Size & Position).
    pub(super) fn size_pos_window(&mut self, ctx: &egui::Context) {
        if !self.size_pos_open {
            return;
        }
        let st = self.size_pos_state();
        let fields: &[SizePosField] = match &st {
            Some(st) if st.one_d => &[
                SizePosField::BeginX,
                SizePosField::BeginY,
                SizePosField::EndX,
                SizePosField::EndY,
                SizePosField::Length,
                SizePosField::Angle,
            ],
            Some(_) => &[
                SizePosField::X,
                SizePosField::Y,
                SizePosField::Width,
                SizePosField::Height,
                SizePosField::Angle,
            ],
            None => &[],
        };
        // The edit buffers belong to one selection: a new primary
        // selection reloads every field from the document.
        if self.size_pos_bufs.0 != self.sel {
            self.size_pos_bufs = (self.sel, Default::default());
        }
        let mut open = true;
        let mut commit: Option<(SizePosField, f64)> = None;
        let unit = self.view.unit.suffix();
        let mut bufs = std::mem::take(&mut self.size_pos_bufs.1);
        egui::Window::new(crate::i18n::strs().msg_size_position())
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                let Some(st) = &st else {
                    ui.label(crate::i18n::strs().msg_sp_no_selection());
                    return;
                };
                egui::Grid::new("dc-size-pos")
                    .num_columns(3)
                    .spacing(egui::vec2(8.0, 6.0))
                    .show(ui, |ui| {
                        for &f in fields {
                            let label = f.label();
                            ui.label(&label);
                            let current = self.size_pos_value(st, f);
                            let shown = current.map(fmt_field).unwrap_or_default();
                            let guard = self.size_pos_guard(st, f);
                            let buf = bufs.entry(f).or_insert_with(|| shown.clone());
                            // Not being edited: track the document (an
                            // undo, a drag) live.
                            let id = egui::Id::unique(("dc-size-pos-field", f));
                            if !ui.memory(|m| m.has_focus(id)) {
                                *buf = shown.clone();
                            }
                            let r = ui.add_enabled(
                                guard.is_none(),
                                egui::TextEdit::singleline(buf).id(id).desired_width(110.0),
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label)
                            });
                            if let Some(g) = &guard {
                                r.on_disabled_hover_text(g);
                            } else if r.lost_focus() && *buf != shown {
                                if let Some(v) = parse_field(buf) {
                                    commit = Some((f, v));
                                } else {
                                    *buf = shown.clone();
                                }
                            }
                            ui.label(if f.is_angle() { "deg" } else { unit });
                            ui.end_row();
                        }
                    });
            });
        self.size_pos_bufs.1 = bufs;
        if !open {
            self.size_pos_open = false;
        }
        if let Some((f, v)) = commit {
            self.size_pos_commit(f, v);
            self.size_pos_bufs.1.clear();
        }
    }
}

/// A field's display text: up to three decimals, trailing zeros off.
fn fmt_field(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Parse an entered number (a decimal comma is accepted; a trailing
/// unit word is ignored — the field's unit is the label beside it).
fn parse_field(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    let num: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        .collect();
    num.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_text_round_trips() {
        assert_eq!(fmt_field(25.4), "25.4");
        assert_eq!(fmt_field(-0.0001), "0");
        assert_eq!(fmt_field(12.0), "12");
        assert_eq!(parse_field(" 12,5 mm"), Some(12.5));
        assert_eq!(parse_field("abc"), None);
    }
}
