//! The placement frame: `HitFrame` (the renderer-parity transform the
//! hit walk maps row space through) and its builder `hit_frame`.

use vsdx::model::Shape;

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
