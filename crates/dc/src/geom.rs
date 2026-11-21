//! The geometry placement frame, hit walk, and connector-crossing
//! math (#187): the pieces the EXPORT path needs on the dc side —
//! moved verbatim from dc-ui's `connectors.rs` (which re-exports them,
//! so every call site stayed put). The line-jump overlay the canvas
//! paints and the one `dc::svg` now appends share THIS code, so the
//! two can't drift.

use vsdx::model::{Cell, Row, Section, Shape};

/// `ObjType=2` — the routable connector marker (corpus Dynamic
/// connector).
pub fn is_connector(s: &Shape) -> bool {
    s.sheet
        .cell("ObjType")
        .and_then(|c| c.numeric())
        .is_some_and(|v| v == 2.0)
}

/// The chain-aware form (#172): a master-based connector instance
/// carries `ObjType=2` on its MASTER, not locally — the local-only
/// check missed every Visio-authored connector (endpoint drags fell
/// to the plain-1D path and were refused by the glue formulas).
///
/// #226 (MisterDiagram parity evidence, utilisation.vsdx): Visio also
/// authors 1D glue connectors WITHOUT ObjType — the corpus Line-arc
/// master marks itself with the glue engine's signature instead
/// (`GlueType` + `BegTrigger`/`EndTrigger`, no ObjType at all). A
/// resolved `BeginX` plus a glue trigger on either end is that
/// signature: the endpoint cells exist (XForm1D) and the glue engine
/// owns them. Plain 1D lines (the blank template's Line master) carry
/// no triggers and stay out — they are not glue shapes.
pub fn is_connector_in(doc: &vsdx::Document, chain: &[&Shape], s: &Shape) -> bool {
    if is_connector(s) {
        return true;
    }
    // The #174 chain-resolved form: a master-based connector carries
    // ObjType=2 on its MASTER.
    if doc
        .resolve_cell(s, chain, "ObjType")
        .and_then(|c| c.numeric())
        .is_some_and(|v| v == 2.0)
    {
        return true;
    }
    let resolved = |n: &str| doc.resolve_cell(s, chain, n);
    let one_d = resolved("BeginX").is_some();
    let triggers = resolved("BegTrigger").is_some() || resolved("EndTrigger").is_some();
    // Glue-owned endpoints (#250): a STATIC glue write (`PAR(PNT(...))`)
    // leaves no ObjType and no trigger cells, but the formula is the
    // glue engine's own signature — user-authored connectors and
    // stencil masters statically glued in the app reroute through it
    // too. `_WALKGLUE` covers a dynamic end whose trigger write was
    // skipped.
    let glue_formula = |c: Option<&vsdx::Cell>| {
        c.and_then(|c| c.f.clone())
            .is_some_and(|f| f.contains("PAR(PNT(") || f.contains("_WALKGLUE("))
    };
    let glue_owned = triggers
        || glue_formula(resolved("BeginX").as_ref())
        || glue_formula(resolved("EndX").as_ref());
    one_d && glue_owned
}

/// The connector's ink as page-space segments for hit-testing (#172):
/// the rendered line follows the GEOMETRY rows — an arc or elbow
/// leaves the Begin→End chord, so chord-only hit-testing made curved
/// connectors unselectable where another shape's bbox overlaps the
/// belly. Rows are shape-local (0..w × 0..h, y-up) and map onto
/// `rect` (the #137 vertex-handle convention); relative rows carry
/// fractions of the owning size; arcs are sampled by the quadratic
/// through their on-curve control (the renderer's own degenerate-arc
/// fallback), 3 interior samples per curve. When the instance has no
/// local Geometry the master root's first section stands in, scaled
/// from the master's size to the instance's.
/// Everything the hit walk needs to place a shape's geometry rows on
/// the page (#173/#179): the placement frame INCLUDING rotation and
/// flips (the renderer's `translate(Pin) rotate(-Angle) scale(Flip)`
/// composed into page y-up space), and the cell-evaluation
/// environment — geometry row formulas (`Width*1`, `Height*0.5`)
/// evaluate against the INSTANCE's sizes.
#[derive(Debug, Clone)]

pub struct HitFrame {
    pub pin: (f64, f64),
    pub loc_pin: (f64, f64),
    pub size: (f64, f64),
    pub master_size: (f64, f64),
    pub angle: f64,
    /// (+1/−1, +1/−1) from FlipX/FlipY.
    pub flip: (f64, f64),
    pub env: vsdx::formula::Env,
}

impl HitFrame {
    /// Local (row-space, post-Rel scaling) → page: the renderer's
    /// transform in y-up coordinates.
    pub fn to_page(&self, p: (f64, f64)) -> (f64, f64) {
        let (dx, dy) = (
            (p.0 - self.loc_pin.0) * self.flip.0,
            (p.1 - self.loc_pin.1) * self.flip.1,
        );
        let (sn, cs) = self.angle.sin_cos();
        (
            self.pin.0 + dx * cs - dy * sn,
            self.pin.1 + dx * sn + dy * cs,
        )
    }

    /// Page → local: the exact inverse of [`HitFrame::to_page`] — the
    /// #202 curve-control drags map the pointer into row space through
    /// it (rotation and flips included).
    pub fn to_local(&self, p: (f64, f64)) -> (f64, f64) {
        let (dx, dy) = (p.0 - self.pin.0, p.1 - self.pin.1);
        let (sn, cs) = (-self.angle).sin_cos();
        let (dx, dy) = (dx * cs - dy * sn, dx * sn + dy * cs);
        (
            dx / self.flip.0 + self.loc_pin.0,
            dy / self.flip.1 + self.loc_pin.1,
        )
    }

    /// The axis-aligned bounds of the shape's local box through the
    /// full frame — a rotated connector's ink lives OUTSIDE its
    /// unrotated Pin/Width/Height rect, so gates must use this.
    pub fn page_bounds(&self) -> (f64, f64, f64, f64) {
        let (w, h) = self.size;
        let c = [
            self.to_page((0.0, 0.0)),
            self.to_page((w, 0.0)),
            self.to_page((w, h)),
            self.to_page((0.0, h)),
        ];
        let xs = [c[0].0, c[1].0, c[2].0, c[3].0];
        let ys = [c[0].1, c[1].1, c[2].1, c[3].1];
        (
            xs.iter().cloned().fold(f64::INFINITY, f64::min),
            ys.iter().cloned().fold(f64::INFINITY, f64::min),
            xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        )
    }
}

/// Build a shape's [`HitFrame`] from resolved cells (a minimal
/// renderer-parity env: the names geometry formulas reference).
pub fn hit_frame(
    doc: &vsdx::model::Document,
    chain: &[&Shape],
    s: &Shape,
    master_size: (f64, f64),
) -> HitFrame {
    let get = |n: &str| doc.resolve_cell(s, chain, n).and_then(|c| c.numeric());
    let size = (get("Width").unwrap_or(1.0), get("Height").unwrap_or(1.0));
    let mut env = vsdx::formula::Env::new();
    for n in [
        "PinX", "PinY", "Width", "Height", "LocPinX", "LocPinY", "Angle",
    ] {
        if let Some(v) = get(n) {
            env.insert(n.to_string(), v);
        }
    }
    HitFrame {
        pin: (get("PinX").unwrap_or(0.0), get("PinY").unwrap_or(0.0)),
        loc_pin: (
            get("LocPinX").unwrap_or(size.0 / 2.0),
            get("LocPinY").unwrap_or(size.1 / 2.0),
        ),
        size,
        master_size,
        angle: get("Angle").unwrap_or(0.0),
        flip: (
            if get("FlipX").unwrap_or(0.0) != 0.0 {
                -1.0
            } else {
                1.0
            },
            if get("FlipY").unwrap_or(0.0) != 0.0 {
                -1.0
            } else {
                1.0
            },
        ),
        env,
    }
}

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
/// Geometry (like [`geometry_hit_segments`]).
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

/// The connector hit segments of a shape's geometry in PAGE space
/// (#173/#179): walks the shape's Geometry rows — merging inherited
/// rows and cells from the master — sampling arcs, and mapping every
/// point through the full placement frame (rotation + flips).
/// `strip_stored_jumps` (callers gate on connector-ness) treats
/// stored jump `ArcTo` rows as straight runs (#294) — the hit
/// polyline, the crossing set and the rendered ink all agree on the
/// route without the stale arc.
pub fn geometry_hit_segments(
    s: &Shape,
    master_root: Option<&Shape>,
    frame: &HitFrame,
    strip_stored_jumps: bool,
) -> Vec<((f64, f64), (f64, f64))> {
    let mut sections: Vec<&Section> = s.sheet.sections_named("Geometry");
    // (owning width/height for Rel rows, x/y scale). The scale only
    // applies when the WHOLE section is the master's (its cached
    // literals are in master units); per-cell inheritance evaluates
    // master formulas in instance terms and scales only cached
    // literals (#179).
    let (own, scale) = if sections.is_empty() {
        let Some(root) = master_root else {
            return Vec::new();
        };
        sections = root.sheet.sections_named("Geometry");
        if sections.is_empty() {
            return Vec::new();
        }
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
    // The master rows of the FIRST geometry section, for per-cell
    // inheritance and master-only rows (shape 1017-style connectors
    // inherit their MoveTo entirely).
    let master_rows: Vec<&Row> = master_root
        .and_then(|r| r.sheet.sections_named("Geometry").into_iter().next())
        .map(|g| g.rows.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let master_row = |ix: i64| master_rows.iter().find(|r| r.ix == ix).copied();
    let axis_scale = |cell: &str| {
        if cell == "X" || cell == "A" {
            scale.0
        } else {
            scale.1
        }
    };
    let cell = |row: &Row, master: Option<&Row>, name: &str| -> Option<f64> {
        row_cell_value(
            row.cell(name),
            master.and_then(|m| m.cell(name)),
            axis_scale(name),
            &frame.env,
        )
    };
    let to_page = |p: (f64, f64)| -> (f64, f64) { frame.to_page(p) };
    let quad = |c0: (f64, f64), ctrl: (f64, f64), c1: (f64, f64), u: f64| -> (f64, f64) {
        (
            (1.0 - u).powi(2) * c0.0 + 2.0 * u * (1.0 - u) * ctrl.0 + u * u * c1.0,
            (1.0 - u).powi(2) * c0.1 + 2.0 * u * (1.0 - u) * ctrl.1 + u * u * c1.1,
        )
    };
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut cur: Option<(f64, f64)> = None; // shape-local
    // The same pen in the UNSCALED row frame — stored-jump
    // classification classifies there, matching the ink path (#294
    // review).
    let mut cur_local: Option<(f64, f64)> = None;
                                            // #188: the spline accumulator — SplineStart opens, SplineKnot
                                            // rows extend, anything else flushes; NURBSTo rows evaluate
                                            // standalone. The flattened samples ARE the hit polyline, so
                                            // hit-testing agrees with the rendered ink (crate::curve::flatten).
    let mut spline_acc: Option<crate::curve::SplineAcc> = None;
    const MAX_ROWS: usize = 64;
    // The ordered walk: the instance's rows, then master-only rows
    // (by ix) the instance never overrode.
    let mut walk: Vec<(&Row, Option<&Row>)> = Vec::new();
    for geo in &sections {
        for row in geo.rows.iter().take(MAX_ROWS) {
            walk.push((row, master_row(row.ix)));
        }
    }
    for r in master_rows.iter().take(MAX_ROWS) {
        if !walk.iter().any(|(row, _)| row.ix == r.ix) {
            walk.push((r, None));
        }
    }
    walk.sort_by_key(|(row, _)| row.ix);
    // A row's evaluated endpoint in the same frame the walk uses —
    // also the route-context lookahead for stored-jump classification
    // (#294 review). The bool: the row is a MoveTo (positions the pen,
    // draws no run).
    // Row kind: 0 = MoveTo (positions the pen, draws no run), 1 =
    // straight LineTo (the only run that counts as jump context —
    // curved neighbors' endpoint chords can lie about the route,
    // #294 review), 2 = anything else.
    let walk_row = |i: usize| -> Option<((f64, f64), u8)> {
        let (r, m) = walk.get(i)?;
        let t = r
            .t
            .as_deref()
            .or(m.and_then(|mm| mm.t.as_deref()))
            .unwrap_or("");
        let x = cell(r, *m, "X").unwrap_or(0.0);
        let y = cell(r, *m, "Y").unwrap_or(0.0);
        let rel = t.starts_with("Rel");
        let end = if rel { (x * own.0, y * own.1) } else { (x, y) };
        let kind = if t == "MoveTo" || t == "RelMoveTo" {
            0u8
        } else if t == "LineTo" || t == "RelLineTo" {
            1
        } else {
            2
        };
        Some((end, kind))
    };
    for (k, (row, master)) in walk.iter().take(MAX_ROWS).enumerate() {
        let t = row
            .t
            .as_deref()
            .or(master.and_then(|m| m.t.as_deref()))
            .unwrap_or("");
        // Omitted geometry cells default to 0 (the corpus arcs omit Y
        // and inherit it); a row nothing resolves is still walked as a
        // 0-coordinate row — never dropped (#179).
        let x = cell(row, *master, "X").unwrap_or(0.0);
        let y = cell(row, *master, "Y").unwrap_or(0.0);
        let rel = t.starts_with("Rel");
        let end_local = if rel { (x * own.0, y * own.1) } else { (x, y) };
        // Master-section cached literals are in master units.
        let end = if scale != (1.0, 1.0) {
            (end_local.0 * scale.0, end_local.1 * scale.1)
        } else {
            end_local
        };
        match t {
            "MoveTo" | "RelMoveTo" => {
                cur = Some(end);
                cur_local = Some(end_local);
                pts.push(to_page(end));
            }
            "ArcTo" | "RelArcTo" => {
                // |A| = sagitta, bulging right of travel; the
                // quadratic control that reproduces the apex.
                let raw_a = cell(row, *master, "A").unwrap_or(0.0);
                if let (Some(c0), Some(c0_local)) = (cur, cur_local) {
                    let d = (end.0 - c0.0, end.1 - c0.1);
                    // The classification chord in the LOCAL frame (the
                    // row coordinates before the section scale) — the
                    // same frame the ink path classifies in.
                    let d_class = (end_local.0 - c0_local.0, end_local.1 - c0_local.1);
                    // Route context (#294 review): the run continues
                    // collinear INTO the arc and OUT of it — Visio
                    // writes jumps mid-run. Only straight neighbors
                    // count (curved endpoint chords can lie about the
                    // route), and zero-length degenerate rows are
                    // skipped without crossing a MoveTo.
                    let mut prev_dir = None;
                    let mut pi = k as isize - 1;
                    while pi >= 1 {
                        let Some((pe, kind)) = walk_row(pi as usize) else { break };
                        if kind != 1 {
                            break;
                        }
                        let Some((pp, _)) = walk_row(pi as usize - 1) else { break };
                        let pd = (pe.0 - pp.0, pe.1 - pp.1);
                        if pd.0.hypot(pd.1) > 1e-9 {
                            prev_dir = Some(pd);
                            break;
                        }
                        pi -= 1;
                    }
                    let mut next_dir = None;
                    let mut ni = k + 1;
                    while ni < walk.len() {
                        let Some((ne, kind)) = walk_row(ni) else { break };
                        if kind != 1 {
                            break;
                        }
                        let p = if ni == k + 1 {
                            end
                        } else {
                            match walk_row(ni - 1) {
                                Some((q, _)) => q,
                                None => break,
                            }
                        };
                        let nd = (ne.0 - p.0, ne.1 - p.1);
                        if nd.0.hypot(nd.1) > 1e-9 {
                            next_dir = Some(nd);
                            break;
                        }
                        ni += 1;
                    }
                    // #294: a stored jump arc straightens — the
                    // polyline follows the chord, so the crossing set
                    // never sees the stale bump. Classified in the
                    // LOCAL frame (unscaled row coordinates), the same
                    // frame the ink path classifies in — instance
                    // scaling must not make the walks disagree (#294
                    // review).
                    let stored_jump = strip_stored_jumps
                        && is_stored_jump_arc(raw_a, d_class, prev_dir, next_dir);
                    if !stored_jump {
                        // The bump is geometry in the scaled frame:
                        // the local sagitta scales with the local
                        // chord's perpendicular axis.
                        let a = raw_a
                            * if d_class.0.abs() >= d_class.1.abs() {
                                scale.1
                            } else {
                                scale.0
                            };
                        let len = d.0.hypot(d.1).max(1e-9);
                        let mid = (
                            (c0.0 + end.0) / 2.0 + d.1 / len * a,
                            (c0.1 + end.1) / 2.0 - d.0 / len * a,
                        );
                        let ctrl = (
                            2.0 * mid.0 - (c0.0 + end.0) / 2.0,
                            2.0 * mid.1 - (c0.1 + end.1) / 2.0,
                        );
                        for u in [0.25, 0.5, 0.75] {
                            pts.push(to_page(quad(c0, ctrl, end, u)));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "EllipticalArcTo" | "RelEllipticalArcTo" => {
                // (A,B) is an ON-curve point → quadratic control.
                // #233: an authored D=0 is a ZERO-WIDTH arc — the
                // renderer draws the endpoint chord, so the hit walk
                // follows the ink (a quadratic through an off-chord
                // control would test the doubled-back curve that is
                // never drawn; CodeRabbit, #278).
                let zero_width = cell(row, *master, "D").is_some_and(|d| d.abs() < 1e-9);
                let a = cell(row, *master, "A");
                let b = cell(row, *master, "B");
                if !zero_width {
                    if let (Some(a), Some(b), Some(c0)) = (a, b, cur) {
                        let ctrl = (
                            2.0 * a - (c0.0 + end.0) / 2.0,
                            2.0 * b - (c0.1 + end.1) / 2.0,
                        );
                        for u in [0.25, 0.5, 0.75] {
                            pts.push(to_page(quad(c0, ctrl, end, u)));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "SplineStart" => {
                // #188: opens a spline (§2.4.2.15) — X/Y is the
                // second control point; A/B/C/D are second/first/
                // last knots and the degree.
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                spline_acc = Some(crate::curve::SplineAcc::start(
                    cur.unwrap_or(end),
                    end,
                    cell(row, *master, "A").unwrap_or(0.0),
                    cell(row, *master, "B").unwrap_or(0.0),
                    cell(row, *master, "C").unwrap_or(1.0),
                    cell(row, *master, "D").unwrap_or(3.0),
                ));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "SplineKnot" => match spline_acc.as_mut() {
                Some(acc) => {
                    acc.knot(end, cell(row, *master, "A").unwrap_or(0.0));
                    cur = Some(end);
                cur_local = Some(end_local);
                }
                None => {
                    pts.push(to_page(end));
                    cur = Some(end);
                cur_local = Some(end_local);
                }
            },
            "NURBSTo" => {
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                // §2.4.2.7: the E cell's NURBS() formula carries the
                // interior points (fractions of Width/Height when
                // xType/yType = 0) with their knots and weights; the
                // row cells frame them and X/Y is the endpoint.
                let e_text = row
                    .cell("E")
                    .and_then(|c| c.v.clone().or_else(|| c.f.clone()))
                    .unwrap_or_default();
                let mut drew = false;
                if let Some(f) = crate::curve::parse_nurbs(&e_text) {
                    // Per-axis, matching the renderer's corrected
                    // total (#218 review round): a fractional axis
                    // (type 0) lands directly in instance units —
                    // the frame's own dims, NO extra scale (`emit_pt`
                    // applies it exactly once); an absolute axis
                    // carries master-unit literals and takes only the
                    // scale. An inherited resized master keeps the
                    // hit polyline ON the rendered curve.
                    let (fx, fy) = (
                        if f.x_type == 0.0 { frame.size.0 } else { scale.0 },
                        if f.y_type == 0.0 { frame.size.1 } else { scale.1 },
                    );
                    let start = cur.unwrap_or(end);
                    let mut points = vec![start];
                    let mut knots = vec![cell(row, *master, "C").unwrap_or(0.0)];
                    let mut weights = vec![cell(row, *master, "D").unwrap_or(1.0)];
                    for (gx, gy, gk, gw) in &f.points {
                        points.push((gx * fx, gy * fy));
                        knots.push(*gk);
                        weights.push(*gw);
                    }
                    knots.push(cell(row, *master, "A").unwrap_or(0.0));
                    knots.push(f.last_knot);
                    weights.push(cell(row, *master, "B").unwrap_or(1.0));
                    points.push(end);
                    let built = crate::curve::SplineData {
                        degree: f.degree,
                        points,
                        weights,
                        knots,
                    }
                    .sanitized();
                    if let Some(data) = built {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                        cur = Some(end);
                cur_local = Some(end_local);
                        drew = true;
                    }
                }
                if !drew {
                    pts.push(to_page(end));
                    cur = Some(end);
                cur_local = Some(end_local);
                }
            }
            _ if t.ends_with("To") || t.contains("Spline") || t == "InfiniteLine" => {
                // LineTo plus the v0.2 polyline/infinite-line chords.
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
                pts.push(to_page(end));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            _ => {
                if let Some(acc) = spline_acc.take() {
                    if let Some(data) = acc.flush() {
                        for p in crate::curve::SplineData::flatten(&data, 16)
                            .into_iter()
                            .skip(1)
                        {
                            pts.push(to_page(p));
                        }
                    }
                }
            }
        }
    }
    if let Some(acc) = spline_acc.take() {
        if let Some(data) = acc.flush() {
            for p in crate::curve::SplineData::flatten(&data, 16)
                .into_iter()
                .skip(1)
            {
                pts.push(to_page(p));
            }
        }
    }
    pts.windows(2).map(|w| (w[0], w[1])).collect()
}

pub struct JumpLine {
    pub id: i64,
    pub z: usize,
    pub weight: f64,
    /// (r, g, b) — the stroke color both overlays repaint.
    pub color: (u8, u8, u8),
    pub pts: Vec<(f64, f64)>,
}

/// A computed hop (#108): where the jumping line crosses the under
/// line, both directions (unit), and both strokes. Visio's default
/// crossing treatment — the more-horizontal line hops (ties: the
/// later in z), arc style, never persisted.
#[derive(Debug, Clone, Copy)]
pub struct Jump {
    pub at: (f64, f64),
    /// Unit vector along the JUMPING line.
    pub dir: (f64, f64),
    /// Unit vector along the under line.
    pub under: (f64, f64),
    pub weight: f64,
    pub under_weight: f64,
    pub color: (u8, u8, u8),
    pub under_color: (u8, u8, u8),
}

/// Visio's jump radius (#294): `LineToLineX × LineJumpFactorX / 2`,
/// both page-layout cells resolved through the page's style
/// inheritance ([`vsdx::Document::page_line_to_line_x`]/[
/// `vsdx::Document::page_line_jump_factor`]). The #294 repro pins the
/// identity: its stored jump `ArcTo` carries `A = −0.04166667` —
/// exactly 0.125 in × 0.66667 / 2, the "No Style" values Visio
/// resolves for that page. (The old constant 0.12 was ~3× too large
/// and ignored the page cells entirely.)
pub fn jump_radius_in(doc: &vsdx::Document, page: &vsdx::Page) -> f64 {
    doc.page_line_to_line_x(page) * doc.page_line_jump_factor(page) / 2.0
}

/// The largest sagitta a stored jump `ArcTo` may carry (inches):
/// Visio's radius = LineToLineX × LineJumpFactorX / 2, and even a
/// doubled LineToLineX with factor 2 stays under it.
const MAX_STORED_JUMP_SAGITTA_IN: f64 = 0.5;

/// The run continues along the chord: the neighboring segment exists,
/// is collinear with it, and points the same way (Visio writes jumps
/// MID-RUN — an isolated authored semicircle has no such context and
/// stays a curve, #294 review).
fn run_continues(dir: Option<(f64, f64)>, chord: (f64, f64)) -> bool {
    let Some(d) = dir else {
        return false;
    };
    let dl = d.0.hypot(d.1);
    let cl = chord.0.hypot(chord.1);
    dl > 1e-9
        && (d.0 * chord.1 - d.1 * chord.0).abs() <= 0.02 * cl * dl
        && d.0 * chord.0 + d.1 * chord.1 > 0.0
}

/// A stored line-jump arc (#294): Visio saves a connector's jump as an
/// `ArcTo` row ON the orthogonal run — the chord lies along one axis,
/// the arc is a SEMICIRCLE over the gap (`|A| = radius = chord / 2`
/// exactly; the #294 repro's row: chord 0.0833, A −0.0416667 — the
/// band admits writer rounding), and the route continues collinear on
/// both sides (`prev_dir`/`next_dir`: the neighboring segments'
/// directions; `None` = no such run). Corner arcs cut ACROSS the run
/// (a diagonal chord), authored shallow bumps fall far below the
/// band, and whole-route semicircles have no continuing runs — all
/// stay curves. The callers additionally gate on connector-ness so
/// artwork arcs never strip. Once stripped, the route is straight at
/// the old arc and the jump overlay draws the ONE arc at the current
/// crossing with the page's own radius — a stored arc stops lingering
/// after a move (#294).
pub fn is_stored_jump_arc(
    sagitta: f64,
    chord: (f64, f64),
    prev_dir: Option<(f64, f64)>,
    next_dir: Option<(f64, f64)>,
) -> bool {
    let len = chord.0.hypot(chord.1);
    let a = sagitta.abs();
    len > 1e-9
        && a <= MAX_STORED_JUMP_SAGITTA_IN
        && a >= 0.4 * len
        && a <= 0.6 * len
        && chord.0.abs().min(chord.1.abs()) <= len * 0.02
        && run_continues(prev_dir, chord)
        && run_continues(next_dir, chord)
}


/// Proper segment intersection (interior of both).
pub fn seg_cross(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> Option<(f64, f64)> {
    seg_cross_open(a0, a1, b0, b1)
        .filter(|(_, t, u)| (1e-9..=1.0 - 1e-9).contains(t) && (1e-9..=1.0 - 1e-9).contains(u))
        .map(|(p, _, _)| p)
}

/// Open segment intersection: the point plus both window parameters —
/// the caller decides which endpoint coincidences count.
fn seg_cross_open(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> Option<((f64, f64), f64, f64)> {
    let (r, s) = ((a1.0 - a0.0, a1.1 - a0.1), (b1.0 - b0.0, b1.1 - b0.1));
    let denom = r.0 * s.1 - r.1 * s.0;
    if denom.abs() < 1e-12 {
        return None; // parallel
    }
    let qp = (b0.0 - a0.0, b0.1 - a0.1);
    let t = (qp.0 * s.1 - qp.1 * s.0) / denom;
    let u = (qp.0 * r.1 - qp.1 * r.0) / denom;
    Some(((a0.0 + t * r.0, a0.1 + t * r.1), t, u))
}

/// The two runs at the INTERIOR vertex `vx` sit on the same side of
/// the other segment's line — the polyline touches it and bounces
/// back (a tangency), it does not cross (#294 review).
fn same_side(vx: usize, pts: &[(f64, f64)], other: &[(f64, f64)]) -> bool {
    let (a, b) = (other[0], other[1]);
    let dir = (b.0 - a.0, b.1 - a.1);
    let side = |p: (f64, f64)| dir.0 * (p.1 - a.1) - dir.1 * (p.0 - a.0);
    let (s1, s2) = (side(pts[vx - 1]), side(pts[vx + 1]));
    s1 * s2 > 1e-12
}

/// The polyline index a crossing hugs, when it hugs a segment
/// endpoint at all (`t` ≈ 0 or 1).
fn hugged_vertex(t: f64, window_index: usize) -> Option<usize> {
    if t <= 1e-9 {
        Some(window_index)
    } else if t >= 1.0 - 1e-9 {
        Some(window_index + 1)
    } else {
        None
    }
}

/// Every crossing hop among the lines (#108): pairwise proper
/// segment intersections; the more-horizontal segment jumps.
/// `radius` is the page's resolved jump radius (inches —
/// [`jump_radius_in`]): it sizes the hop arc and bounds what counts
/// as one crossing REGION (#251) when converging crossings cluster.
pub fn line_jumps(lines: &[JumpLine], radius: f64) -> Vec<Jump> {
    // Degenerate rows (a straight route's elbow LineTos collapse onto
    // the end) repeat points; the endpoint classification below reads
    // indices, so normalize first or the connector's geometric END
    // looks like an interior vertex (#294 review).
    let pts: Vec<Vec<(f64, f64)>> = lines
        .iter()
        .map(|l| {
            let mut v: Vec<(f64, f64)> = Vec::with_capacity(l.pts.len());
            for p in &l.pts {
                if v.last().is_none_or(|q| (q.0 - p.0).hypot(q.1 - p.1) > 1e-9) {
                    v.push(*p);
                }
            }
            v
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..lines.len() {
        for j in (i + 1)..lines.len() {
            // Per-PAIR crossings (#251): one crossing REGION hops once.
            // Flattened curves produce entry+exit sample windows whose
            // crossings converge onto the same pixels — hopping each
            // drew overlapping arcs on one intersection (the #238
            // screenshot). Clusters within the hop radius merge.
            let mut pair: Vec<Jump> = Vec::new();
            for (k1, w1) in pts[i].windows(2).enumerate() {
                for (k2, w2) in pts[j].windows(2).enumerate() {
                    // #294 review: a crossing ON an interior vertex of
                    // either polyline (an elbow corner exactly on the
                    // other line) is a real crossing — the strict-
                    // interior test used to drop it and, with stored
                    // jump arcs straightened, the replacement hop
                    // vanished with it. A crossing at a polyline's
                    // first/last point stays excluded: that is the
                    // connector's glue end, not a crossing.
                    let Some((at, t, u)) = seg_cross_open(w1[0], w1[1], w2[0], w2[1]) else {
                        continue;
                    };
                    // Off-segment hits (the crossing sits on the
                    // segment's EXTENSION) are not crossings.
                    if !(-1e-9..=1.0 + 1e-9).contains(&t)
                        || !(-1e-9..=1.0 + 1e-9).contains(&u)
                    {
                        continue;
                    }
                    let at_end_i = hugged_vertex(t, k1)
                        .is_some_and(|ix| ix == 0 || ix == pts[i].len() - 1);
                    let at_end_j = hugged_vertex(u, k2)
                        .is_some_and(|ix| ix == 0 || ix == pts[j].len() - 1);
                    if at_end_i || at_end_j {
                        continue;
                    }
                    // An interior-vertex contact is a crossing only
                    // when the vertex's two runs straddle the other
                    // segment's line — same-side runs TOUCH the line
                    // and bounce back (a tangency), never a hop (#294
                    // review).
                    if hugged_vertex(t, k1)
                        .filter(|&ix| ix > 0 && ix + 1 < pts[i].len())
                        .is_some_and(|ix| same_side(ix, &pts[i], w2))
                    {
                        continue;
                    }
                    if hugged_vertex(u, k2)
                        .filter(|&ix| ix > 0 && ix + 1 < pts[j].len())
                        .is_some_and(|ix| same_side(ix, &pts[j], w1))
                    {
                        continue;
                    }
                    let unit = |a: (f64, f64), b: (f64, f64)| {
                        let d = (b.0 - a.0, b.1 - a.1);
                        let l = d.0.hypot(d.1).max(1e-9);
                        (d.0 / l, d.1 / l)
                    };
                    let (d1, d2) = (unit(w1[0], w1[1]), unit(w2[0], w2[1]));
                    // The more-horizontal line hops; a tie gives the
                    // later-in-z line the hop.
                    let i_jumps = d1.1.abs() < d2.1.abs()
                        || (d1.1.abs() - d2.1.abs()).abs() < 1e-9 && lines[i].z > lines[j].z;
                    let (jmp, und) = if i_jumps {
                        (&lines[i], &lines[j])
                    } else {
                        (&lines[j], &lines[i])
                    };
                    let (d, du) = if i_jumps { (d1, d2) } else { (d2, d1) };
                    pair.push(Jump {
                        at,
                        dir: d,
                        under: du,
                        weight: jmp.weight,
                        under_weight: und.weight,
                        color: jmp.color,
                        under_color: und.color,
                    });
                }
            }
            // Merge the pair's jumps inside a hop radius: the arc's
            // own reach (2× the page's jump radius) bounds what "one
            // region" means visually. Distinct crossings beyond it
            // stay.
            let mut clustered: Vec<Jump> = Vec::new();
            for jmp in pair {
                let reach =
                    (radius * 2.0).max((jmp.weight + jmp.under_weight) * 2.0);
                // Compare EVERY existing cluster (#218 review): the
                // last-only check (under the old x-sort) let a jump
                // near an earlier cluster open a duplicate instead of
                // merging. First hit wins; the centroid keeps the arc
                // on the intersection instead of stacking arcs.
                let mut hit = false;
                for c in clustered.iter_mut() {
                    let dd = (c.at.0 - jmp.at.0).hypot(c.at.1 - jmp.at.1);
                    if dd < reach {
                        c.at = ((c.at.0 + jmp.at.0) / 2.0, (c.at.1 + jmp.at.1) / 2.0);
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    clustered.push(jmp);
                }
            }
            out.extend(clustered);
        }
    }
    out
}

/// Parse a `#rrggbb` line-color cell into the overlay's (r, g, b).
fn parse_hex_rgb(v: &str) -> Option<(u8, u8, u8)> {
    let hex = v.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

/// The page's connector crossings (#187 export parity): every
/// top-level connector's polyline — the same `hit_frame` +
/// [`geometry_hit_segments`] walk the canvas jump overlay uses,
/// formula-preferred, curve-aware — crossed with Visio's rule
/// ([`line_jumps`]: the more-horizontal line hops, ties to the later
/// z). Hidden-layer members and guides skip like the render walk.
pub fn page_jumps(doc: &vsdx::Document, page: &vsdx::Page, hidden: &[i64]) -> Vec<Jump> {
    // #294: the hop radius is the page's own LineToLineX ×
    // LineJumpFactorX / 2, resolved through the style chain.
    let radius = jump_radius_in(doc, page);
    let mut lines: Vec<JumpLine> = Vec::new();
    for (z, s) in page.shapes.iter().enumerate() {
        if s.sheet
            .cell("LayerMember")
            .and_then(|c| c.numeric())
            .is_some_and(|m| hidden.contains(&(m as i64)))
        {
            continue;
        }
        if s.type_.as_deref() == Some("Guide") {
            continue;
        }
        // Top-level chain: the shape plus its master root.
        let mut chain = vec![s];
        let msize = s
            .master
            .and_then(|mid| doc.master(mid))
            .and_then(|m| m.shapes.first())
            .map(|r| {
                chain.push(r);
                let g = |n: &str| {
                    doc.resolve_cell(r, &[r], n)
                        .and_then(|c| c.numeric())
                        .unwrap_or(1.0)
                };
                (g("Width"), g("Height"))
            })
            .unwrap_or((1.0, 1.0));
        if !is_connector_in(doc, &chain, s) {
            continue;
        }
        let frame = hit_frame(doc, &chain, s, msize);
        let master_root = chain.get(1).copied();
        // Everything reaching here is a connector (the gate above), so
        // stored jump arcs strip from the polyline (#294) — the hop
        // computes on the straight run, never on a stale arc.
        let segs = geometry_hit_segments(s, master_root, &frame, true);
        if segs.is_empty() {
            continue;
        }
        // The polyline the crossing math walks: the segments' endpoints.
        let mut pts: Vec<(f64, f64)> = Vec::with_capacity(segs.len() + 1);
        pts.push(segs[0].0);
        for (_, b) in &segs {
            pts.push(*b);
        }
        let weight = doc
            .resolve_cell(s, &chain, "LineWeight")
            .and_then(|c| c.numeric())
            .unwrap_or(0.01);
        // #235 corollary (CodeRabbit, #278): an explicit-zero
        // LineWeight paints nothing since the fallback pen went, so
        // an invisible shaft must not make visible connectors hop
        // over it (and it has no hop of its own to draw). Drop it
        // from the crossing set entirely.
        if weight == 0.0 {
            continue;
        }
        // Resolve like the renderer (#218 review): a legacy palette
        // index (`LineColor V='0'`) is common, and the hex-only read
        // drew every such crossing's under line and hop arc in gray.
        let color = parse_hex_rgb(&crate::svg::themed_color_string(doc, s, &chain, "LineColor"))
            .unwrap_or((0x42, 0x42, 0x42));
        lines.push(JumpLine {
            id: s.id,
            z,
            weight,
            color,
            pts,
        });
    }
    line_jumps(&lines, radius)
}

#[cfg(test)]
mod tests {
    /// The tests' hop radius — Visio's default (0.125 in × 2/3 / 2,
    /// the #294 identity).
    const TEST_JUMP_RADIUS: f64 = 0.125 * (2.0 / 3.0) / 2.0;

    /// A plain black 0.01-in test line.
    fn jline(id: i64, z: usize, pts: &[(f64, f64)]) -> JumpLine {
        JumpLine {
            id,
            z,
            weight: 0.01,
            color: (0, 0, 0),
            pts: pts.to_vec(),
        }
    }

    /// #251: one crossing REGION hops once — a flattened curve's
    /// entry/exit sample windows converge onto the same pixels and
    /// merge to a single arc; distinct crossings beyond the hop
    /// radius stay separate.
    #[test]
    fn line_jumps_cluster_converging_crossings() {
        let l = |id: i64, z: usize, pts: &[(f64, f64)]| JumpLine {
            id,
            z,
            weight: 0.01,
            color: (0, 0, 0),
            pts: pts.to_vec(),
        };
        // A shallow arc's belly crossed by a line: entry and exit sit
        // within the hop radius of each other.
        let arc = vec![
            (0.0, 1.0),
            (0.5, 1.04),
            (1.0, 1.06),
            (1.5, 1.04),
            (2.0, 1.0),
        ];
        let js = line_jumps(
            &[
                l(1, 0, &arc),
                l(2, 1, &[(0.8, 2.0), (1.2, 0.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(js.len(), 1, "the converging pair merges: {js:?}");
        // Two crossings far apart stay two hops.
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (1.0, 1.0), (1.0, 3.0), (4.0, 3.0)]),
                l(2, 1, &[(2.0, 0.0), (2.0, 4.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(js.len(), 1, "one real crossing");
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (3.0, 1.0)]),
                l(2, 1, &[(1.0, 0.0), (1.0, 2.0), (2.5, 2.0), (2.5, 0.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(js.len(), 2, "distinct crossings stay separate: {js:?}");
    }

    /// #294: the stored-jump detector recognizes Visio's saved jump
    /// row (the repro's `X 1.4583→1.5417, A −0.0416667` — chord ON the
    /// run) and leaves corner arcs (diagonal chord), authored shallow
    /// bumps and artwork-sized arcs alone.
    #[test]
    fn stored_jump_arcs_match_the_repro_row() {
        // Mid-run with collinear continuation on both sides (the run).
        assert!(is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
        ));
        // A corner arc cuts across the run — never a jump.
        assert!(!is_stored_jump_arc(
            0.09,
            (0.5, 0.5),
            Some((1.0, 0.0)),
            Some((0.0, 1.0)),
        ));
        // An authored shallow bump (the #294 review's control) is a
        // curve: |A| ≪ the semicircle band.
        assert!(!is_stored_jump_arc(
            0.25,
            (4.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
        ));
        // Artwork-sized arcs stay.
        assert!(!is_stored_jump_arc(
            2.0,
            (0.0, 4.0),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
        ));
        // Degenerate: zero chord and a zero-sagitta straight row
        // (which the ink path already emits as a line).
        assert!(!is_stored_jump_arc(
            0.0,
            (0.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
        ));
        assert!(!is_stored_jump_arc(
            0.0,
            (0.0, 1.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
        ));
        // An authored whole-route semicircle (#294 review) has no
        // continuing runs — it stays a curve.
        assert!(!is_stored_jump_arc(0.25, (0.5, 0.0), None, None));
        // Mid-run geometry but a run that stops at the arc (an
        // authored end-arc) is not a jump either.
        assert!(!is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            None,
        ));
    }

    /// #294 review: a crossing exactly ON an interior vertex of the
    /// under polyline still hops — the strict-interior intersection
    /// used to drop it (and with stored jump arcs straightened, the
    /// replacement hop vanished with it). A crossing at a connector's
    /// glue END never hops.
    #[test]
    fn line_jumps_hop_at_interior_vertices_not_at_glue_ends() {
        // The vertical's corner (2,1) sits exactly on the horizontal.
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 1.0), (3.0, 1.0)]),
                jline(2, 1, &[(2.0, 0.0), (2.0, 1.0), (2.0, 2.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(
            js.len(),
            1,
            "the T-junction at the interior vertex hops: {js:?}"
        );
        // The same corner AT the horizontal's glue end: no hop.
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 1.0), (2.0, 1.0)]),
                jline(2, 1, &[(2.0, 0.0), (2.0, 1.0), (2.0, 2.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(js.len(), 0, "a glue end never hops: {js:?}");
        // A V that only TOUCHES the horizontal (both runs on the same
        // side) is a tangency, not a crossing — no hop (#294 review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 0.0), (3.0, 0.0)]),
                jline(2, 1, &[(0.0, 1.0), (1.0, 0.0), (2.0, 1.0)]),
            ],
            TEST_JUMP_RADIUS,
        );
        assert_eq!(js.len(), 0, "a tangency never hops: {js:?}");
    }

    /// A hit segment: start and end in page space.
    type Seg = ((f64, f64), (f64, f64));

    /// A minimal 1-section connector shape with the given geometry rows.
    fn conn_shape(id: i64, w: f64, h: f64, rows: Vec<Row>) -> Shape {
        let mut s = Shape {
            id,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        s.sheet.set_cell("ObjType", Cell::value("2"));
        s.sheet.set_cell("Width", Cell::value(fmt_width(w)));
        s.sheet.set_cell("Height", Cell::value(fmt_width(h)));
        let sec = Section {
            name: "Geometry".into(),
            ix: 0,
            rows,
            ..Section::default()
        };
        s.sheet.children.push(SheetChild::Section(sec));
        s
    }

    fn fmt_width(v: f64) -> String {
        format!("{v}")
    }

    fn mrow(ix: i64, t: &str, cells: &[(&str, f64)]) -> Row {
        Row {
            t: Some(t.into()),
            ix,
            cells: cells
                .iter()
                .map(|(n, v)| (n.to_string(), Cell::value(format!("{v}"))))
                .collect(),
            ..Row::default()
        }
    }

    /// The stored-jump row set of the #294 repro (cleanroom values):
    /// MoveTo, a run in, the jump arc, a run out.
    fn jump_rows() -> Vec<Row> {
        vec![
            mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            mrow(2, "LineTo", &[("X", 1.0), ("Y", 0.0)]),
            mrow(
                3,
                "ArcTo",
                &[("X", 1.083_333_3), ("Y", 0.0), ("A", -0.041_666_7)],
            ),
            mrow(4, "LineTo", &[("X", 3.0), ("Y", 0.0)]),
        ]
    }

    fn flat(pts: &[Seg]) -> bool {
        // Every segment horizontal at y = pts[0].0's y: a straight run
        // (a retained arc would sample off it).
        let y0 = pts.first().map(|(a, _)| a.1).unwrap_or(0.0);
        pts.iter()
            .all(|(a, b)| (a.1 - y0).abs() < 1e-6 && (b.1 - y0).abs() < 1e-6)
    }

    /// #294 review: a degenerate (zero-length) straight row next to
    /// the stored jump does not break the run lookup — the arc still
    /// strips.
    #[test]
    fn stored_jumps_survive_degenerate_neighbor_rows() {
        let mut rows = jump_rows();
        // A collapsed elbow between the arc and the continuing run.
        rows.insert(
            3,
            mrow(4, "LineTo", &[("X", 1.083_333_3), ("Y", 0.0)]),
        );
        rows[4].ix = 5;
        let s = conn_shape(7, 3.0, 0.25, rows);
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, true);
        assert!(!segs.is_empty());
        assert!(flat(&segs), "the arc strips past the degenerate row");
    }

    /// #294 review: classification happens in the LOCAL row frame in
    /// BOTH walks — instance scaling (uniform or stretched) must not
    /// make the hit walk retain a bump the ink path straightens.
    #[test]
    fn stored_jumps_classify_in_the_local_frame_at_any_scale() {
        for (iw, ih) in [(2.0, 2.0), (2.0, 1.0)] {
            let root = conn_shape(1, 2.0, 0.5, jump_rows());
            let mut inst = Shape {
                id: 2,
                type_: Some("Shape".into()),
                master: Some(1),
                ..Shape::default()
            };
            inst.sheet.set_cell("ObjType", Cell::value("2"));
            inst.sheet
                .set_cell("Width", Cell::value(format!("{}", 2.0 * iw)));
            inst.sheet
                .set_cell("Height", Cell::value(format!("{}", 0.5 * ih)));
            let doc = vsdx::Document::default();
            let chain = vec![&inst, &root];
            let frame = hit_frame(&doc, &chain, &inst, (2.0, 0.5));
            let segs = geometry_hit_segments(&inst, Some(&root), &frame, true);
            assert!(!segs.is_empty());
            assert!(
                flat(&segs),
                "the arc strips at instance scale ({iw}, {ih})"
            );
        }
    }

    use super::*;
    use vsdx::model::SheetChild;

    /// #188: a NURBSTo row's hit segments follow the EVALUATED curve
    /// (the E formula's interior points pull the polyline off the
    /// MoveTo→endpoint chord), so a click on the curve's belly hits.
    #[test]
    fn nurbs_hit_segments_leave_the_chord() {
        let mut s = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", 2.0),
            ("PinY", 1.0),
            ("Width", 4.0),
            ("Height", 2.0),
            ("LocPinX", 2.0),
            ("LocPinY", 1.0),
        ] {
            s.sheet.set_cell(n, Cell::value(format!("{v}")));
        }
        let e = "NURBS(1, 3, 0, 0, 1,0.5,0,1, 3,1.5,0,1)";
        let row = |t: &str, ix: i64, cells: Vec<(&str, f64)>| Row {
            t: Some(t.into()),
            ix,
            cells: cells
                .into_iter()
                .map(|(n, v)| (n.to_string(), Cell::value(format!("{v}"))))
                .collect(),
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            row("MoveTo", 1, vec![("X", 0.0), ("Y", 0.0)]),
            row(
                "NURBSTo",
                2,
                vec![
                    ("X", 4.0),
                    ("Y", 2.0),
                    ("A", 0.0),
                    ("B", 1.0),
                    ("C", 0.0),
                    ("D", 1.0),
                ],
            ),
        ];
        let nurbs_row = &mut sec.rows[1];
        nurbs_row.cells.push((
            "E".to_string(),
            Cell {
                v: Some(e.to_string()),
                f: Some(e.to_string()),
                ..Cell::default()
            },
        ));
        s.sheet
            .children
            .push(SheetChild::Section(std::mem::take(&mut sec)));

        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, false);
        // Segments are page space (the frame is identity here); some
        // sample must sit off the (0,0)→(4,2) chord — the interior
        // points pull the belly above it.
        assert!(
            segs.len() > 8,
            "the NURBS hit polyline is dense: {}",
            segs.len()
        );
        let off = segs
            .iter()
            .any(|(a, _)| (0.5 * a.0 + 2.0 - a.1).abs() > 0.05);
        assert!(off, "hit samples leave the chord");
    }

    /// #187: two crossing connectors produce one hop; a rotated
    /// jumper crosses too (slice 2 — the Angle gate is gone).
    #[test]
    fn zero_width_arcs_and_zero_weight_connectors_leave_interaction() {
        // #278 review (Greptile P1 + CodeRabbit): an authored D=0 arc
        // renders the chord, so its on-curve control is dead — no
        // curve handle seat floats off the ink — and a zero-weight
        // connector paints nothing, so nothing hops over it.
        let arc_shape = |d: &str| {
            let mut s = Shape {
                id: 1,
                type_: Some("Shape".into()),
                ..Shape::default()
            };
            for (n, v) in [
                ("Width", "2"),
                ("Height", "2"),
                ("LocPinX", "1"),
                ("LocPinY", "1"),
                ("ObjType", "2"),
            ] {
                s.sheet.set_cell(n, Cell::value(v));
            }
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![
                        ("X".to_string(), Cell::value("0")),
                        ("Y".to_string(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                Row {
                    t: Some("EllipticalArcTo".into()),
                    ix: 2,
                    cells: vec![
                        ("X".to_string(), Cell::value("1")),
                        ("Y".to_string(), Cell::value("0")),
                        ("A".to_string(), Cell::value("0.5")),
                        ("B".to_string(), Cell::value("1")),
                        ("C".to_string(), Cell::value("0")),
                        ("D".to_string(), Cell::value(d)),
                    ],
                    ..Row::default()
                },
            ];
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        let doc = vsdx::Document::default();
        let flat = arc_shape("0");
        let chain = [&flat];
        let frame = hit_frame(&doc, &chain, &flat, (1.0, 1.0));
        assert!(
            arc_row(&flat, None, &frame).is_none(),
            "a D=0 arc has no curve seat — the control is dead ink"
        );
        let live = arc_shape("0.5");
        let chain2 = [&live];
        let frame2 = hit_frame(&doc, &chain2, &live, (1.0, 1.0));
        let arc = arc_row(&live, None, &frame2).expect("a live arc yields its seat");
        assert_eq!(arc.on_curve, (0.5, 1.0));

        // A zero-weight connector is excluded from the crossing set:
        // the horizontal axis line no longer hops over it.
        let connector = |id: i64, pin: (f64, f64), w: f64, h: f64, angle: f64, weight: Option<f64>| {
            let mut c = Shape {
                id,
                type_: Some("Shape".into()),
                ..Shape::default()
            };
            for (n, v) in [
                ("PinX", pin.0),
                ("PinY", pin.1),
                ("Width", w),
                ("Height", h),
                ("LocPinX", w / 2.0),
                ("LocPinY", h / 2.0),
                ("Angle", angle),
                ("ObjType", 2.0),
            ] {
                c.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            if let Some(wt) = weight {
                c.sheet.set_cell("LineWeight", Cell::value(format!("{wt}")));
            }
            let row = |ix: i64, x: f64, y: f64| Row {
                t: Some("LineTo".into()),
                ix,
                cells: vec![
                    ("X".to_string(), Cell::value(format!("{x}"))),
                    ("Y".to_string(), Cell::value(format!("{y}"))),
                ],
                ..Row::default()
            };
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![
                        ("X".to_string(), Cell::value("0")),
                        ("Y".to_string(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                row(2, w, h),
            ];
            c.sheet.children.push(SheetChild::Section(sec));
            c
        };
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(connector(1, (3.0, 1.0), 4.0, 0.0, 0.0, Some(0.0)));
        page.shapes.push(connector(
            2,
            (3.0, 1.0),
            3.0,
            0.0,
            -std::f64::consts::FRAC_PI_4,
            None,
        ));
        let jumps = page_jumps(&doc, &page, &[]);
        assert!(
            jumps.is_empty(),
            "an invisible shaft forces no hop: {jumps:?}"
        );
    }

    #[test]
    fn page_jumps_crosses_axis_and_rotated() {
        let connector = |id: i64, pin: (f64, f64), w: f64, h: f64, angle: f64| {
            let mut s = Shape {
                id,
                type_: Some("Shape".into()),
                ..Shape::default()
            };
            for (n, v) in [
                ("PinX", pin.0),
                ("PinY", pin.1),
                ("Width", w),
                ("Height", h),
                ("LocPinX", w / 2.0),
                ("LocPinY", h / 2.0),
                ("Angle", angle),
                ("ObjType", 2.0),
            ] {
                s.sheet.set_cell(n, Cell::value(format!("{v}")));
            }
            let row = |ix: i64, x: f64, y: f64| Row {
                t: Some("LineTo".into()),
                ix,
                cells: vec![
                    ("X".to_string(), Cell::value(format!("{x}"))),
                    ("Y".to_string(), Cell::value(format!("{y}"))),
                ],
                ..Row::default()
            };
            let mut sec = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            sec.rows = vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![
                        ("X".to_string(), Cell::value("0")),
                        ("Y".to_string(), Cell::value("0")),
                    ],
                    ..Row::default()
                },
                row(2, w, h),
            ];
            s.sheet.children.push(SheetChild::Section(sec));
            s
        };
        // A horizontal connector (1,1)..(5,1) and a 45°-rotated one
        // through the same page area.
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        page.shapes.push(connector(1, (3.0, 1.0), 4.0, 0.0, 0.0));
        page.shapes.push(connector(
            2,
            (3.0, 1.0),
            3.0,
            0.0,
            -std::f64::consts::FRAC_PI_4,
        ));
        let doc = vsdx::Document::default();
        let jumps = page_jumps(&doc, &page, &[]);
        assert_eq!(jumps.len(), 1, "one crossing: {jumps:?}");
        // The more-horizontal line hops: the AXIS line jumps (dy 0
        // beats the 45° line's 0.707); the rotated one is the under.
        let j = &jumps[0];
        assert!(
            j.dir.1.abs() < 1e-6 && j.under.0.abs() > 0.5,
            "the axis line hops over the rotated one: dir {:?} under {:?}",
            j.dir,
            j.under
        );
    }
}
