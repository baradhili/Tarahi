//! Geometry-row resolution: [`row_cell_value`]'s inheritance ladder,
//! plus the `CubicRow`/`ArcRow` curve seats the #202/#226 handles
//! ride.

use super::hitframe::HitFrame;
use vsdx::model::{Cell, Row, Section, Shape};

/// The #202 curved-connector facts: one shape's first local
/// `CubicBezTo` row with its Bézier walk state, shape-local through
/// [`HitFrame`]'s formula-preferred resolution (the same precedence
/// the renderer's `row_num` uses for evaluable formulas).
#[derive(Debug, Clone, Copy)]
pub struct CubicRow {
    pub section_ix: i64,
    pub row_ix: i64,
    /// The previous row's endpoint — the Bézier's start.
    pub start: (f64, f64),
    /// The row's own endpoint.
    pub end: (f64, f64),
    /// The (A,B) control.
    pub ctrl1: (f64, f64),
    /// The (C,D) control.
    pub ctrl2: (f64, f64),
}

impl CubicRow {
    /// The cubic Bézier point at `t` (0..=1).
    pub fn at(&self, t: f64) -> (f64, f64) {
        let u = 1.0 - t;
        let b = |a: f64, c: f64, d: f64, e: f64| {
            u * u * u * a + 3.0 * u * u * t * c + 3.0 * u * t * t * d + t * t * t * e
        };
        (
            b(self.start.0, self.ctrl1.0, self.ctrl2.0, self.end.0),
            b(self.start.1, self.ctrl1.1, self.ctrl2.1, self.end.1),
        )
    }

    /// The t=0.5 point — the #202 `LineMid` seat on a curved
    /// connector (the chord midpoint is off the curve).
    pub fn mid(&self) -> (f64, f64) {
        self.at(0.5)
    }
}

/// The shape's first OWN-geometry `CubicBezTo` row, resolved through
/// `frame` (master-inherited rows stay unhanded, the #137 rule).
pub fn cubic_row(s: &Shape, frame: &HitFrame) -> Option<CubicRow> {
    for geo in s.sheet.sections_named("Geometry") {
        let mut cur: Option<(f64, f64)> = None;
        for row in &geo.rows {
            if row.is_deleted() {
                continue; // deleted rows stay deleted (#293)
            }
            let cell = |name: &str| -> f64 {
                row_cell_value(row.cell(name), None, 1.0, &frame.env).unwrap_or(0.0)
            };
            let (x, y) = (cell("X"), cell("Y"));
            if row.t.as_deref() == Some("CubicBezTo") {
                let (a, b) = (cell("A"), cell("B"));
                let (c, d) = (cell("C"), cell("D"));
                return Some(CubicRow {
                    section_ix: geo.ix,
                    row_ix: row.ix,
                    start: cur.unwrap_or((0.0, 0.0)),
                    end: (x, y),
                    ctrl1: (a, b),
                    ctrl2: (c, d),
                });
            }
            cur = Some((x, y));
        }
    }
    None
}

/// One shape's first `EllipticalArcTo` row — the #226 arc twin of
/// [`CubicRow`]: the corpus Line-arc masters (utilisation.vsdx) shape
/// their whole ink as one arc, so the `LineMid` handle must seat ON
/// the curve, not on the begin→end chord. Resolution mirrors the hit
/// walk's row walk (own rows + the master rows the instance dropped,
/// ix-ordered, Rel rows as size fractions, master-section caches
/// scaled) so the seat sits on the RENDERED curve — the renderer and
/// hit walk draw the same quadratic through the on-curve control.
#[derive(Debug, Clone, Copy)]
pub struct ArcRow {
    /// The previous row's endpoint — the arc's start.
    pub start: (f64, f64),
    /// The row's own endpoint.
    pub end: (f64, f64),
    /// The ON-curve control (the row's A/B).
    pub on_curve: (f64, f64),
}

impl ArcRow {
    /// The quadratic control reproducing an on-curve apex (the
    /// renderer's degenerate-arc fallback).
    fn ctrl(&self) -> (f64, f64) {
        (
            2.0 * self.on_curve.0 - (self.start.0 + self.end.0) / 2.0,
            2.0 * self.on_curve.1 - (self.start.1 + self.end.1) / 2.0,
        )
    }

    /// The curve point at `u` (0..=1).
    pub fn at(&self, u: f64) -> (f64, f64) {
        let (ctrl, v) = (self.ctrl(), 1.0 - u);
        (
            v * v * self.start.0 + 2.0 * v * u * ctrl.0 + u * u * self.end.0,
            v * v * self.start.1 + 2.0 * v * u * ctrl.1 + u * u * self.end.1,
        )
    }

    /// The t=0.5 point — the `LineMid` seat ON the curve (the chord
    /// midpoint hangs off it).
    pub fn mid(&self) -> (f64, f64) {
        self.at(0.5)
    }
}

/// The shape's first `EllipticalArcTo` row through [`HitFrame`]'s
/// resolution; `master_root` stands in when the shape has no own
/// Geometry (like
/// [`geometry_hit_segments`](super::segments::geometry_hit_segments)).
pub fn arc_row(s: &Shape, master_root: Option<&Shape>, frame: &HitFrame) -> Option<ArcRow> {
    let mut sections: Vec<&Section> = s.sheet.sections_named("Geometry");
    let (own, scale) = if sections.is_empty() {
        let root = master_root?;
        sections = root.sheet.sections_named("Geometry");
        let ms = frame.master_size;
        (
            ms,
            if ms.0.abs() > 1e-6 && ms.1.abs() > 1e-6 {
                (frame.size.0 / ms.0, frame.size.1 / ms.1)
            } else {
                (1.0, 1.0)
            },
        )
    } else {
        (frame.size, (1.0, 1.0))
    };
    let master_rows: Vec<&Row> = master_root
        .and_then(|r| r.sheet.sections_named("Geometry").into_iter().next())
        .map(|g| g.rows.iter().collect())
        .unwrap_or_default();
    let mut walk: Vec<(&Row, Option<&Row>)> = Vec::new();
    for geo in &sections {
        for row in geo.rows.iter().take(64) {
            walk.push((row, master_rows.iter().find(|r| r.ix == row.ix).copied()));
        }
    }
    for r in master_rows.iter().take(64) {
        if !walk.iter().any(|(row, _)| row.ix == r.ix) {
            walk.push((r, None));
        }
    }
    // Deleted rows stay deleted (#293) — an instance marker wins over
    // the master row it shadows. The filter runs ONCE after the whole
    // merge (PR #299 review): inside the loop it would (a) never fire
    // for a master-less shape — deleted instance rows surviving the
    // walk — and (b) let a later master-row push resurrect the live
    // row a previously-filtered instance marker was hiding.
    walk.retain(|(row, _)| !row.is_deleted());
    walk.sort_by_key(|(row, _)| row.ix);
    let cell = |row: &Row, master: Option<&Row>, name: &str| -> Option<f64> {
        let axis = if name == "X" || name == "A" { scale.0 } else { scale.1 };
        row_cell_value(row.cell(name), master.and_then(|m| m.cell(name)), axis, &frame.env)
    };
    // The hit walk's endpoint placement: Rel rows resolve ×own size,
    // then master-section caches scale into instance units.
    let placed = |v: (f64, f64), rel: bool| -> (f64, f64) {
        let v = if rel { (v.0 * own.0, v.1 * own.1) } else { v };
        if scale != (1.0, 1.0) { (v.0 * scale.0, v.1 * scale.1) } else { v }
    };
    let mut cur: Option<(f64, f64)> = None;
    for (row, master) in &walk {
        let t = row
            .t
            .as_deref()
            .or(master.and_then(|m| m.t.as_deref()))
            .unwrap_or("");
        let rel = t.starts_with("Rel");
        // Per-coordinate defaults, mirroring the hit walk below: the
        // corpus arcs omit Y and inherit it — an X-only row ends at
        // (x, 0), not (0, 0) (#218 review).
        let end = match (cell(row, *master, "X"), cell(row, *master, "Y")) {
            (Some(x), Some(y)) => placed((x, y), rel),
            (Some(x), None) => placed((x, 0.0), rel),
            (None, Some(y)) => placed((0.0, y), rel),
            (None, None) => (0.0, 0.0),
        };
        if t == "EllipticalArcTo" || t == "RelEllipticalArcTo" {
            // #233: an authored D=0 arc renders its endpoint CHORD, so
            // the on-curve control is dead geometry — a `LineMid` seat
            // on the quadratic would float off the visible line, and
            // dragging it would change nothing the pen shows (Greptile
            // P1 / CodeRabbit, #278). Skip the row: the walk keeps
            // looking for a live arc, and a shape with none takes the
            // chord-midpoint seat like any line. D reads unscaled —
            // it is a ratio, and only its zero-ness matters here.
            let zero_width = row_cell_value(
                row.cell("D"),
                master.and_then(|m| m.cell("D")),
                1.0,
                &frame.env,
            )
            .is_some_and(|d| d.abs() < 1e-9);
            if !zero_width {
                // A/B ride `cell` alone — the hit walk applies no rel
                // or post-scale to the on-curve control.
                return match (cell(row, *master, "A"), cell(row, *master, "B"), cur) {
                    (Some(a), Some(b), Some(start)) => {
                        Some(ArcRow { start, end, on_curve: (a, b) })
                    }
                    _ => None,
                };
            }
        }
        cur = Some(end);
    }
    None
}

/// Resolve one geometry-row cell to a number through inheritance
/// (#179): the instance's literal, then the instance's formula
/// against the instance env, then the MASTER's formula (evaluated in
/// instance terms — `Width*1` means the instance's Width), then the
/// master's cached value scaled master→instance. Omitted cells
/// default to 0 (Visio's default) at the caller. `pub(crate)` so the
/// #202 curve-control handles position and commit through the SAME
/// resolution the renderer and hit walk use.
pub fn row_cell_value(
    inst: Option<&Cell>,
    master: Option<&Cell>,
    axis_scale: f64,
    env: &vsdx::formula::Env,
) -> Option<f64> {
    let eval = |f: &str| -> Option<f64> {
        vsdx::formula::eval(f, env)
            .ok()
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
    };
    let literal = |c: &Cell| -> Option<f64> {
        let plain =
            c.f.as_deref()
                .is_none_or(|f| f == "Inh" || f == "No Formula");
        plain.then(|| c.numeric()).flatten()
    };
    if let Some(c) = inst {
        if let Some(v) = literal(c) {
            return Some(v);
        }
        if let Some(v) = c.f.as_deref().and_then(&eval) {
            return Some(v);
        }
        // A formula that failed to evaluate: its cached V is the
        // last-known instance value — better than dropping the row.
        if let Some(v) = c.numeric() {
            return Some(v);
        }
    }
    if let Some(c) = master {
        if let Some(v) = c.f.as_deref().and_then(&eval) {
            return Some(v);
        }
        if let Some(v) = literal(c) {
            return Some(v * axis_scale);
        }
        if let Some(v) = c.numeric() {
            return Some(v * axis_scale);
        }
    }
    None
}
