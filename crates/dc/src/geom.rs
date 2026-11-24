//! The geometry placement frame and hit walk (#187): the pieces the
//! EXPORT path needs on the dc side — moved verbatim from dc-ui's
//! `connectors.rs` (which re-exports them, so every call site stayed
//! put). The connector-crossing math is the line-jump engine, now in
//! [`crate::jumps`] (#324) and re-exported below for path
//! compatibility; the line-jump overlay the canvas paints and the one
//! `dc::svg` appends both come from that module's one crossing set.

use vsdx::model::{Cell, Row, Section, Shape};

// #324: the line-jump engine moved to `crate::jumps` (a pure move out
// of this file). These re-exports keep every `dc::geom::` jump path
// resolving, and put the stored-jump classifier in scope for the hit
// walk below, which straightens stored arcs through it. New code
// imports `dc::jumps`.
pub use crate::jumps::{
    is_stored_jump_arc, jump_radii_in, jump_radius_in, jump_radius_y_in, line_jumps, page_jumps,
    page_jumps_stripping, seg_cross, Jump, JumpLine,
};

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
    // Deleted rows stay deleted (#293) — an instance marker wins over
    // the master row it shadows. The filter runs ONCE after the whole
    // merge (PR #299 review): inside the loop it would (a) never fire
    // for a master-less shape — deleted instance rows surviving the
    // walk — and (b) let a later master-row push resurrect the live
    // row a previously-filtered instance marker was hiding.
    walk.retain(|(row, _)| !row.is_deleted());
    walk.sort_by_key(|(row, _)| row.ix);
    let cell = |row: &Row, master: Option<&Row>, name: &str| -> Option<f64> {
        let axis = if name == "X" || name == "A" {
            scale.0
        } else {
            scale.1
        };
        row_cell_value(
            row.cell(name),
            master.and_then(|m| m.cell(name)),
            axis,
            &frame.env,
        )
    };
    // The hit walk's endpoint placement: Rel rows resolve ×own size,
    // then master-section caches scale into instance units.
    let placed = |v: (f64, f64), rel: bool| -> (f64, f64) {
        let v = if rel { (v.0 * own.0, v.1 * own.1) } else { v };
        if scale != (1.0, 1.0) {
            (v.0 * scale.0, v.1 * scale.1)
        } else {
            v
        }
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
                    (Some(a), Some(b), Some(start)) => Some(ArcRow {
                        start,
                        end,
                        on_curve: (a, b),
                    }),
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

/// One walked geometry row of the hit walk: the row, its master
/// fallback, the section feed name, the type, the endpoint in the
/// LOCAL frame and the frame-scaled one, and the row's A/B/C/D
/// control cells CAPTURED at the row's own evaluation point — the
/// render (#249) reads a row's controls while the env carries the
/// feeds of the rows before it (and its own) only, so the hit walk
/// must too (PR #299 review: a two-pass read saw later rows' feeds,
/// and the clickable curve drifted from the drawn one after a
/// resize).
type EvaldRow<'a> = (
    &'a Row,
    Option<&'a Row>,
    &'a str,
    String,
    (f64, f64),
    (f64, f64),
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

/// The geometry walk's row budget: [`geometry_hit_segments`] reads at
/// most this many rows per section — a render-time horizon. Rows past
/// it are invisible to the walk (and to the jump overlay, which rides
/// the same walk), so a document edit that REPLACES rows from the
/// walk's output must skip longer sections or it erases their tails
/// (review on the #303 batch).
pub const MAX_WALK_ROWS: usize = 64;

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
    strip_stored_jumps: Option<(f64, f64)>,
) -> Vec<((f64, f64), (f64, f64))> {
    geometry_hit_segments_marked(s, master_root, frame, strip_stored_jumps, &[])
        .into_iter()
        .map(|(a, b, _)| (a, b))
        .collect()
}

/// [`geometry_hit_segments`] with each PEN-UP BRIDGE segment marked
/// `true` — a segment that exists only because a `MoveTo` jumped the
/// pen between strokes, not drawn ink. The crossing set
/// (`page_jumps_stripping`) bridges a break only when it is a
/// RECOGNIZED stored gap slot; authored stroke breaks stay out of the
/// crossing walk (#324 phase 5). Hit-testing keeps the unmarked form.
/// `skip` names sections (by pointer) the caller excludes from the
/// walk — the crossing engine's per-section NoShow/NoLine filter (PR
/// #339 round 5); hit-testing passes none.
pub fn geometry_hit_segments_marked(
    s: &Shape,
    master_root: Option<&Shape>,
    frame: &HitFrame,
    strip_stored_jumps: Option<(f64, f64)>,
    skip: &[&Section],
) -> Vec<MarkedSeg> {
    let mut sections: Vec<&Section> = s.sheet.sections_named("Geometry");
    // (owning width/height for Rel rows, x/y scale). The scale only
    // applies when the WHOLE section is the master's (its cached
    // literals are in master units — those rows walk with
    // `master=None`, so the one scaling happens at `end`); per-cell
    // inheritance evaluates master formulas in instance terms and
    // scales only cached literals (#179) — the ratio below serves that
    // path in BOTH branches (CodeRabbit round 2 on #121: with an own
    // section, a master-fallback cached literal stayed in MASTER units
    // while the ink path's merge scaled it, so the two walks
    // disagreed on inherited inline arcs).
    let master_ratio = master_root.and_then(|_| {
        let ms = frame.master_size;
        if ms.0.abs() > 1e-6 && ms.1.abs() > 1e-6 {
            Some((frame.size.0 / ms.0, frame.size.1 / ms.1))
        } else {
            None
        }
    });
    let (own, scale) = if sections.is_empty() {
        let Some(root) = master_root else {
            return Vec::new();
        };
        sections = root.sheet.sections_named("Geometry");
        if sections.is_empty() {
            return Vec::new();
        }
        (frame.master_size, master_ratio.unwrap_or((1.0, 1.0)))
    } else {
        (frame.size, (1.0, 1.0))
    };
    // The caller's per-section skip applies AFTER the own-vs-master
    // decision — an all-hidden OWN set must not fall through to the
    // master's (PR #339 round 5). ORDINALS survive the filter: the
    // env feed names (Geometry2.X2) and the master-row merge identity
    // key on the ORIGINAL position — renumbering after a hidden
    // Geometry1 once redirected a Geometry2.X2 formula at nothing
    // under its own name, and the merge appended the hidden first
    // section's master-only rows onto the visible second (CodeRabbit
    // round 6 on PR #339).
    let mut secs: Vec<(usize, &Section)> =
        (0..sections.len()).zip(sections.drain(..)).collect();
    secs.retain(|(_, g)| !skip.iter().any(|h| std::ptr::eq(*h, *g)));
    if secs.is_empty() {
        return Vec::new();
    }
    // The master rows of the FIRST geometry section, for per-cell
    // inheritance and master-only rows (shape 1017-style connectors
    // inherit their MoveTo entirely).
    let master_rows: Vec<&Row> = master_root
        .and_then(|r| r.sheet.sections_named("Geometry").into_iter().next())
        .map(|g| g.rows.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let master_row = |ix: i64| master_rows.iter().find(|r| r.ix == ix).copied();
    // Per-cell master-fallback literals scale master→instance by the
    // size ratio in both branches (see the head note).
    let per_cell = master_ratio.unwrap_or((1.0, 1.0));
    let axis_scale = |cell: &str| {
        if cell == "X" || cell == "A" {
            per_cell.0
        } else {
            per_cell.1
        }
    };
    // The walk's OWN env copy: evaluated row coordinates feed back into
    // it section-qualified (`Geometry1.X2`), exactly like the ink path —
    // row-to-row references resolve against the LIVE route, not the
    // stale cached V (CodeRabbit round 2 on #121: the reroute engine's
    // `Geometry1.X2+…` controls kept the hit walk at the authored pose
    // while the SVG walked the resized route).
    let mut env = frame.env.clone();
    let cell =
        |env: &vsdx::formula::Env, row: &Row, master: Option<&Row>, name: &str| -> Option<f64> {
            row_cell_value(
                row.cell(name),
                master.and_then(|m| m.cell(name)),
                axis_scale(name),
                env,
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
    // Stroke-break indices: pts positions a pen move OPENED (#324 phase 5).
    let mut pen_up_before: Vec<usize> = Vec::new();
    // The subset of those a SECTION BOUNDARY opened (the next
    // section's own MoveTo — never a stored-gap candidate, CodeRabbit
    // round 6 on PR #339).
    let mut section_starts: Vec<usize> = Vec::new();
    let mut prev_gname: Option<&str> = None;
    // shape-local
    let mut cur: Option<(f64, f64)> = None;
    // The same pen in the UNSCALED row frame — stored-jump
    // classification classifies there, matching the ink path (#294
    // review).
    let mut cur_local: Option<(f64, f64)> = None;
    // #188: the spline accumulator — SplineStart opens, SplineKnot
    // rows extend, anything else flushes; NURBSTo rows evaluate
    // standalone. The flattened samples ARE the hit polyline, so
    // hit-testing agrees with the rendered ink (crate::curve::flatten).
    let mut spline_acc: Option<crate::curve::SplineAcc> = None;
    // The ordered walk: SECTION BY SECTION in enumeration order, rows
    // by ix WITHIN each. A GLOBAL ix sort once interleaved two
    // sections' rows (each carries its own IX 1 MoveTo and IX 2
    // LineTo) into M-M-L-L — the pen walked section 2's start as a
    // break and then drew a phantom span from it onto section 1's
    // LineTo endpoint, and the crossing set treated that span as ink
    // (PR #339 round 5). Master-only rows merge into the section that
    // was ORIGINALLY first (the single-section inheritance assumption
    // the ink path's merge also makes) — and into NOTHING when that
    // section is filtered out: its inherited tail is hidden with it.
    // Each row carries its ORIGINAL section-qualified feed name
    // (`Geometry1.X2`) — the ink path's convention.
    let gnames: Vec<String> = secs
        .iter()
        .map(|(si, _)| format!("Geometry{}", si + 1))
        .collect();
    let mut walk: Vec<(&Row, Option<&Row>, &str)> = Vec::new();
    for (k, (si, geo)) in secs.iter().enumerate() {
        let mut sec_walk: Vec<(&Row, Option<&Row>, &str)> = geo
            .rows
            .iter()
            .take(MAX_WALK_ROWS)
            .map(|row| (row, master_row(row.ix), gnames[k].as_str()))
            .collect();
        if *si == 0 {
            for r in master_rows.iter().take(MAX_WALK_ROWS) {
                if !sec_walk.iter().any(|(row, _, _)| row.ix == r.ix) {
                    sec_walk.push((r, None, gnames[k].as_str()));
                }
            }
        }
        sec_walk.sort_by_key(|(row, _, _)| row.ix);
        walk.extend(sec_walk);
    }
    // `Del="1"` rows stay deleted (GitHub #293): the INSTANCE row's
    // marker wins when one is present, else the master-only row's own.
    // The #293 repro's connector inherits its master's hook row through
    // an instance `<Row Del="1" IX="3" T="LineTo"/>` with no cells.
    walk.retain(|(row, _, _)| !row.is_deleted());
    // Row kind: 0 = MoveTo (positions the pen, draws no run), 1 =
    // straight LineTo (the only run that counts as jump context —
    // curved neighbors' endpoint chords can lie about the route,
    // #294 review), 2 = anything else.
    let kind_of = |t: &str| -> u8 {
        if t == "MoveTo" || t == "RelMoveTo" {
            0u8
        } else if t == "LineTo" || t == "RelLineTo" {
            1
        } else {
            2
        }
    };
    // FIRST PASS — the ordered evaluation, the ink path's #249 rule:
    // each row's endpoint evaluates, feeds back into the env
    // section-qualified (`Geometry1.X2`), and only then do the row's
    // control cells read the env — a row sees the feeds of the rows
    // before it (and its own), never a later row's. The lookahead and
    // the classification both read these EVALUATED values, so
    // row-to-row references (`Geometry1.X2+0.083`, the reroute
    // engine's curved controls) resolve against the live route instead
    // of the stale cached V (CodeRabbit round 2 on #121: the ink walk
    // fed the env, the hit walk didn't, and a resized route stripped
    // in one walk but not the other).
    let mut evald: Vec<EvaldRow> = Vec::new();
    for (row, master, gname) in walk.iter().take(MAX_WALK_ROWS) {
        let t = row
            .t
            .as_deref()
            .or(master.and_then(|m| m.t.as_deref()))
            .unwrap_or("")
            .to_string();
        // Omitted geometry cells default to 0 (the corpus arcs omit Y
        // and inherit it); a row nothing resolves is still walked as a
        // 0-coordinate row — never dropped (#179).
        let x = cell(&env, row, *master, "X").unwrap_or(0.0);
        let y = cell(&env, row, *master, "Y").unwrap_or(0.0);
        let rel = t.starts_with("Rel");
        let end_local = if rel { (x * own.0, y * own.1) } else { (x, y) };
        // Master-section cached literals are in master units.
        let end = if scale != (1.0, 1.0) {
            (end_local.0 * scale.0, end_local.1 * scale.1)
        } else {
            end_local
        };
        env.insert(format!("{gname}.X{}", row.ix), end_local.0);
        env.insert(format!("{gname}.Y{}", row.ix), end_local.1);
        // The controls read NOW — the env carries feeds up to this
        // row's own endpoint, exactly the render's per-row order.
        let a = cell(&env, row, *master, "A");
        let b = cell(&env, row, *master, "B");
        let c = cell(&env, row, *master, "C");
        let d = cell(&env, row, *master, "D");
        evald.push((row, *master, gname, t, end_local, end, a, b, c, d));
    }
    // SECOND PASS — the geometry, against the evaluated values. The
    // route-context directions read LOCAL endpoints (the frame the
    // classification chord lives in) — the scaled frame would bend
    // collinearity under non-uniform instance scaling.
    // The master fallback rode the FIRST pass: every control read the
    // second pass makes is the captured per-row value (the round-3
    // review — a post-pass env read would sample later rows' feeds).
    for (k, (row, _master, _gname, t, end_local, end, ca, cb, cc, cd)) in
        evald.iter().take(MAX_WALK_ROWS).enumerate()
    {
        let t = t.as_str();
        let end_local = *end_local;
        let end = *end;
        let section_start = prev_gname != Some(_gname);
        prev_gname = Some(_gname);
        match t {
            "MoveTo" | "RelMoveTo" => {
                // A mid-route pen move records the STROKE BREAK: the
                // segment pts.last()→this point is a phantom bridge
                // the crossing set must not treat as ink (the marked
                // variant's contract; #324 phase-5 review). A
                // SECTION-BOUNDARY opening carries its own mark —
                // two sections are two strokes by authorship and the
                // blank between them is never a stored gap
                // (CodeRabbit round 6 on PR #339).
                if !pts.is_empty() {
                    pen_up_before.push(pts.len());
                    if section_start {
                        section_starts.push(pts.len());
                    }
                }
                cur = Some(end);
                cur_local = Some(end_local);
                pts.push(to_page(end));
            }
            "ArcTo" | "RelArcTo" => {
                // |A| = sagitta, bulging right of travel; the
                // quadratic control that reproduces the apex.
                let raw_a = (*ca).unwrap_or(0.0);
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
                        let (_, _, _, pt, pe, _, _, _, _, _) = &evald[pi as usize];
                        if kind_of(pt.as_str()) != 1 {
                            break;
                        }
                        let (_, _, _, _, pp, _, _, _, _, _) = &evald[pi as usize - 1];
                        let pd = (pe.0 - pp.0, pe.1 - pp.1);
                        if pd.0.hypot(pd.1) > 1e-9 {
                            prev_dir = Some(pd);
                            break;
                        }
                        pi -= 1;
                    }
                    let mut next_dir = None;
                    let mut ni = k + 1;
                    while ni < evald.len() {
                        let (_, _, _, nt, ne, _, _, _, _, _) = &evald[ni];
                        if kind_of(nt.as_str()) != 1 {
                            break;
                        }
                        let p = if ni == k + 1 {
                            // The arc's LOCAL end — the lookahead must
                            // stay in the classification frame under
                            // instance scaling (#294 review).
                            end_local
                        } else {
                            evald[ni - 1].4
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
                    //
                    // The radius needs no frame conversion: every
                    // geometry frame is INCHES (master-local,
                    // instance-local, page — the instance/master ratios
                    // are geometric RESCALES applied after authoring),
                    // and the stored arc's A was authored at the page's
                    // radius. The comparison is authored inches against
                    // page inches in every frame.
                    let stored_jump = match strip_stored_jumps {
                        Some(r) => is_stored_jump_arc(raw_a, d_class, prev_dir, next_dir, r.0, r.1),
                        None => false,
                    };
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
                let zero_width = (*cd).is_some_and(|d| d.abs() < 1e-9);
                let a = *ca;
                let b = *cb;
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
                    (*ca).unwrap_or(0.0),
                    (*cb).unwrap_or(0.0),
                    (*cc).unwrap_or(1.0),
                    (*cd).unwrap_or(3.0),
                ));
                cur = Some(end);
                cur_local = Some(end_local);
            }
            "SplineKnot" => match spline_acc.as_mut() {
                Some(acc) => {
                    acc.knot(end, (*ca).unwrap_or(0.0));
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
                        if f.x_type == 0.0 {
                            frame.size.0
                        } else {
                            scale.0
                        },
                        if f.y_type == 0.0 {
                            frame.size.1
                        } else {
                            scale.1
                        },
                    );
                    let start = cur.unwrap_or(end);
                    let mut points = vec![start];
                    let mut knots = vec![(*cc).unwrap_or(0.0)];
                    let mut weights = vec![(*cd).unwrap_or(1.0)];
                    for (gx, gy, gk, gw) in &f.points {
                        points.push((gx * fx, gy * fy));
                        knots.push(*gk);
                        weights.push(*gw);
                    }
                    knots.push((*ca).unwrap_or(0.0));
                    knots.push(f.last_knot);
                    weights.push((*cb).unwrap_or(1.0));
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
    geometry_segments_from_pts(pts, &pen_up_before, &section_starts)
}

/// One hit-walk segment, with the PEN-UP BRIDGE flag (`true` = the
/// segment exists only because a `MoveTo` jumped the pen).
/// What one walked segment IS: drawn ink, a pen-up bridge inside one
/// section (a `MoveTo` mid-route — the stored-gap candidate), or a
/// bridge a SECTION BOUNDARY opened (the next section's own MoveTo —
/// never a stored gap: two sections are two strokes by authorship,
/// CodeRabbit round 6 on PR #339).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegMark {
    Ink,
    PenUp,
    SectionStart,
}

pub type MarkedSeg = ((f64, f64), (f64, f64), SegMark);

/// The walk's polyline + stroke-break indices → segments, each bridge
/// marked (a segment that exists only because a pen move jumped the
/// pen — NOT drawn ink; a SECTION-BOUNDARY bridge is its own mark —
/// never a stored-gap candidate). [`geometry_hit_segments`] drops the
/// marks (hit-testing keeps its long-standing behavior); the crossing
/// set keeps them and drops UNRECOGNIZED bridges
/// (`page_jumps_stripping`, #324 phase 5).
fn geometry_segments_from_pts(
    pts: Vec<(f64, f64)>,
    pen_up_before: &[usize],
    section_starts: &[usize],
) -> Vec<MarkedSeg> {
    pts.windows(2)
        .enumerate()
        .map(|(k, w)| {
            let mark = if section_starts.contains(&(k + 1)) {
                SegMark::SectionStart
            } else if pen_up_before.contains(&(k + 1)) {
                SegMark::PenUp
            } else {
                SegMark::Ink
            };
            (w[0], w[1], mark)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// #293: an instance `<Row Del="1" IX="3" T="LineTo"/>` (no
    /// cells) kills the master's row 3 — the merged walk skips it, so
    /// the master's hook never joins the hit polyline (and the
    /// crossing set). A master-only deleted row is dead too; an
    /// instance re-declaring the row alive keeps it.
    #[test]
    fn deleted_row_kills_the_merged_master_row() {
        let master_rows = |del_master: bool| {
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![("X".to_string(), Cell::value("0.125"))],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 2,
                    cells: vec![
                        ("X".to_string(), Cell::value("0.125")),
                        ("Y".to_string(), Cell::value("2.2")),
                    ],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 3,
                    attrs: if del_master {
                        vec![("Del".to_string(), "1".to_string())]
                    } else {
                        Vec::new()
                    },
                    cells: vec![
                        ("X".to_string(), Cell::value("1")),
                        ("Y".to_string(), Cell::value("-1")),
                    ],
                    ..Row::default()
                },
            ]
        };
        let root = |del_master: bool| {
            let mut r = Shape {
                id: 1,
                ..Shape::default()
            };
            r.sheet.set_cell("Width", Cell::value("0.25"));
            r.sheet.set_cell("Height", Cell::value("2.2"));
            r.sheet.children.push(SheetChild::Section(Section {
                name: "Geometry".into(),
                ix: 0,
                rows: master_rows(del_master),
                ..Section::default()
            }));
            r
        };
        let inst = |row3: Option<Row>| {
            let mut s = Shape {
                id: 2,
                master: Some(1),
                ..Shape::default()
            };
            let mut rows = vec![Row {
                t: Some("MoveTo".into()),
                ix: 1,
                cells: vec![("X".to_string(), Cell::value("0.125"))],
                ..Row::default()
            }];
            if let Some(r3) = row3 {
                rows.push(Row {
                    t: Some("LineTo".into()),
                    ix: 2,
                    cells: vec![
                        ("X".to_string(), Cell::value("0.125")),
                        ("Y".to_string(), Cell::value("2.2")),
                    ],
                    ..Row::default()
                });
                rows.push(r3);
            }
            s.sheet.children.push(SheetChild::Section(Section {
                name: "Geometry".into(),
                ix: 0,
                rows,
                ..Section::default()
            }));
            s
        };
        let del_row = || Row {
            t: Some("LineTo".into()),
            ix: 3,
            attrs: vec![("Del".to_string(), "1".to_string())],
            ..Row::default()
        };
        let doc = vsdx::Document::default();
        // The #293 repro: the instance deletes the master's hook row.
        let root1 = root(false);
        let s1 = inst(Some(del_row()));
        let frame = hit_frame(&doc, &[&s1, &root1], &s1, (0.25, 2.2));
        let segs = geometry_hit_segments(&s1, Some(&root1), &frame, None);
        // Exactly the surviving shaft: one segment on the run's page x
        // spanning the frame height (the hook's page x would differ by
        // LocPinX, and an empty walk would also pass a vaguer check).
        assert_eq!(segs.len(), 1, "only the shaft survives: {segs:?}");
        let (a, b) = segs[0];
        assert!(
            (a.0 - b.0).abs() < 1e-9 && (b.1 - a.1).abs() - 2.2 < 1e-6,
            "the shaft is one straight span of the frame height: {segs:?}"
        );
        // The marker lives on the MASTER row alone (a master-only
        // deletion) — dead as well.
        let root2 = root(true);
        let s2 = inst(None);
        let frame = hit_frame(&doc, &[&s2, &root2], &s2, (0.25, 2.2));
        let segs = geometry_hit_segments(&s2, Some(&root2), &frame, None);
        assert!(
            segs.iter().all(|(a, b)| a.0.abs() < 1.0 && b.0.abs() < 1.0),
            "a master-only deleted row is dead too: {segs:?}"
        );
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
        rows.insert(3, mrow(4, "LineTo", &[("X", 1.083_333_3), ("Y", 0.0)]));
        rows[4].ix = 5;
        let s = conn_shape(7, 3.0, 0.25, rows);
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, Some((0.041_666_7, 0.041_666_7)));
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
            let segs =
                geometry_hit_segments(&inst, Some(&root), &frame, Some((0.041_666_7, 0.041_666_7)));
            assert!(!segs.is_empty());
            assert!(flat(&segs), "the arc strips at instance scale ({iw}, {ih})");
        }
    }

    /// CodeRabbit rounds 1–2 on #121: the shape checks alone cannot
    /// tell an AUTHORED semicircle between two straight runs from a
    /// stored jump. Through the hit walk: the authored arc PRESERVES
    /// with a non-matching page radius; a stored jump with the page's
    /// own radius strips; and the walks agree — the ink path's strip
    /// carries the same evidence.
    #[test]
    fn authored_semicircles_preserve_unless_the_page_radius_matches() {
        // An authored 1-inch semicircle (A=−0.5) mid-run, straight
        // continuation on both sides — every geometric test passes.
        let rows = vec![
            mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            mrow(2, "LineTo", &[("X", 0.5), ("Y", 0.0)]),
            mrow(3, "ArcTo", &[("X", 1.5), ("Y", 0.0), ("A", -0.5)]),
            mrow(4, "LineTo", &[("X", 2.5), ("Y", 0.0)]),
        ];
        let s = conn_shape(7, 2.5, 1.0, rows);
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        // The page's radius is the default jump spacing — far from 0.5:
        // the arc is AUTHORED ink and stays a curve.
        let segs = geometry_hit_segments(&s, None, &frame, Some((0.041_666_7, 0.041_666_7)));
        assert!(!segs.is_empty());
        assert!(!flat(&segs), "authored arc preserved: {segs:?}");
    }

    /// CodeRabbit round 1: a full-route `EllipticalArcTo` (the drawn
    /// curve IS the route) never strips — the classifier is an ArcTo
    /// row's evidence chain, and elliptical rows keep their ink.
    #[test]
    fn full_route_elliptical_arcs_never_strip() {
        let rows = vec![
            mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            mrow(2, "LineTo", &[("X", 0.5), ("Y", 0.0)]),
            mrow(
                3,
                "EllipticalArcTo",
                &[("X", 1.5), ("Y", 0.0), ("A", 1.0), ("B", 0.25)],
            ),
            mrow(4, "LineTo", &[("X", 2.5), ("Y", 0.0)]),
        ];
        let s = conn_shape(7, 2.5, 1.0, rows);
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, Some((0.041_666_7, 0.041_666_7)));
        assert!(!segs.is_empty());
        assert!(!flat(&segs), "the full-route arc stays curved: {segs:?}");
    }

    /// CodeRabbit round 2: the hit walk's per-cell master-fallback
    /// literals scale master→instance like the ink path's merge, so an
    /// inherited inline arc classifies in the SAME (instance) frame the
    /// ink walks — the stretched bump appears in BOTH walks' verdicts
    /// (here: preserved, since the stretched radius no longer matches
    /// the page's).
    #[test]
    fn inherited_inline_arcs_scale_like_the_ink_path() {
        // Master: a stored-jump-shaped arc row with CACHED LITERALS
        // (chord 0.0833 over the run, A −0.0416667 — the #294 shape).
        let mut root = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        root.sheet.set_cell("ObjType", Cell::value("2"));
        root.sheet.set_cell("Width", Cell::value("3"));
        root.sheet.set_cell("Height", Cell::value("0.25"));
        let sec = Section {
            name: "Geometry".into(),
            ix: 0,
            rows: vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                mrow(2, "LineTo", &[("X", 1.4583), ("Y", 0.0)]),
                mrow(
                    3,
                    "ArcTo",
                    &[("X", 1.5417), ("Y", 0.0), ("A", -0.041_666_7)],
                ),
                mrow(4, "LineTo", &[("X", 3.0), ("Y", 0.0)]),
            ],
            ..Section::default()
        };
        root.sheet.children.push(SheetChild::Section(sec));
        // Instance: an OWN section whose rows are all F="Inh" (the
        // owner-file pattern), at 2× the master width. The inherited
        // literals scale into instance units — the arc's chord doubles.
        let mut inst = Shape {
            id: 2,
            type_: Some("Shape".into()),
            master: Some(1),
            ..Shape::default()
        };
        inst.sheet.set_cell("ObjType", Cell::value("2"));
        inst.sheet.set_cell("Width", Cell::value("6"));
        inst.sheet.set_cell("Height", Cell::value("0.25"));
        let inh = || Cell {
            f: Some("Inh".into()),
            ..Cell::default()
        };
        let sec = Section {
            name: "Geometry".into(),
            ix: 0,
            rows: vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: vec![("X".to_string(), inh())],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 2,
                    cells: vec![("X".to_string(), inh())],
                    ..Row::default()
                },
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: vec![("X".to_string(), inh()), ("A".to_string(), inh())],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 4,
                    cells: vec![("X".to_string(), inh())],
                    ..Row::default()
                },
            ],
            ..Section::default()
        };
        inst.sheet.children.push(SheetChild::Section(sec));
        let doc = vsdx::Document::default();
        let chain = vec![&inst, &root];
        let frame = hit_frame(&doc, &chain, &inst, (3.0, 0.25));
        // The stretched bump: chord 0.1666, rendered A 0.0833 — twice
        // the page radius, so the evidence declines and the arc keeps
        // its ink. The decisive assertion is the WALKS AGREE: the hit
        // polyline keeps the bump exactly like the ink does (the old
        // code retained it here while the ink stripped — or vice versa
        // — depending on the scale).
        let segs =
            geometry_hit_segments(&inst, Some(&root), &frame, Some((0.041_666_7, 0.041_666_7)));
        assert!(!segs.is_empty());
        assert!(
            !flat(&segs),
            "the stretched stored arc keeps its bump in BOTH walks: {segs:?}"
        );
    }

    /// CodeRabbit round 2: the hit walk feeds evaluated row
    /// coordinates back into the env (`Geometry1.X2`) like the ink
    /// path — row-to-row references resolve against the LIVE route,
    /// not the stale cached V.
    #[test]
    fn hit_walk_feeds_row_coordinates_for_formula_references() {
        let mut s = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        s.sheet.set_cell("ObjType", Cell::value("2"));
        s.sheet.set_cell("Width", Cell::value("2"));
        s.sheet.set_cell("Height", Cell::value("0.5"));
        let sec = Section {
            name: "Geometry".into(),
            ix: 0,
            rows: vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                // A live formula whose cached V is STALE (1 vs 2): the
                // ink path evaluates the formula; the hit walk must
                // agree.
                Row {
                    t: Some("LineTo".into()),
                    ix: 2,
                    cells: vec![(
                        "X".to_string(),
                        Cell {
                            v: Some("2".into()),
                            f: Some("1".into()),
                            ..Cell::default()
                        },
                    )],
                    ..Row::default()
                },
                // The arc rides Geometry1.X2: live 1 + 0.5 = 1.5, not
                // the stale 2.5.
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: vec![(
                        "X".to_string(),
                        Cell {
                            v: Some("2.5".into()),
                            f: Some("Geometry1.X2+0.5".into()),
                            ..Cell::default()
                        },
                    )],
                    ..Row::default()
                },
            ],
            ..Section::default()
        };
        s.sheet.children.push(SheetChild::Section(sec));
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, Some((0.041_666_7, 0.041_666_7)));
        let last = segs.last().expect("segments").1;
        // Local (1.5, 0) through the frame (pin 0, locpin 1/0.25).
        let expected = frame.to_page((1.5, 0.0));
        assert!(
            (last.0 - expected.0).abs() < 1e-9 && (last.1 - expected.1).abs() < 1e-9,
            "the arc lands at the LIVE Geometry1.X2+0.5 = 1.5 (page {expected:?}), \
             not the stale 2.5: {last:?}"
        );
        assert!(
            (last.0 - 2.5 + 1.0).abs() > 1e-9,
            "guard: the stale reading would land elsewhere"
        );
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
        let segs = geometry_hit_segments(&s, None, &frame, None);
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

    /// #278 review (Greptile P1 + CodeRabbit): dead ink leaves the
    /// interaction — an authored D=0 arc renders the chord, so its
    /// on-curve control is dead and no curve handle seat floats off
    /// the ink; and a zero-weight connector paints nothing, so nothing
    /// hops over it.
    #[test]
    fn zero_width_arcs_and_zero_weight_connectors_leave_interaction() {
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
        let connector =
            |id: i64, pin: (f64, f64), w: f64, h: f64, angle: f64, weight: Option<f64>| {
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
        page.shapes
            .push(connector(1, (3.0, 1.0), 4.0, 0.0, 0.0, Some(0.0)));
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
}
