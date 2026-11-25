//! Affine transforms: the SVG-order affine behind every coordinate
//! mapping between shape-local, parent-local, and page space.
use super::*;
/// An SVG-order affine: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`
/// (y-down SVG space; the composition order matches the `transform=`
/// strings this module emits, so geometry derived here lands on the
/// same pixels the renderer draws).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub(crate) fn translate(tx: f64, ty: f64) -> Affine {
        Affine {
            e: tx,
            f: ty,
            ..Affine::IDENTITY
        }
    }

    /// SVG rotate(deg): clockwise on screen (y grows down).
    pub(crate) fn rotate_deg(deg: f64) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine {
            a: c,
            b: s,
            c: -s,
            d: c,
            ..Affine::IDENTITY
        }
    }

    pub(crate) fn scale(sx: f64, sy: f64) -> Affine {
        Affine {
            a: sx,
            d: sy,
            ..Affine::IDENTITY
        }
    }

    /// Matrix product `self · next` — `next` applies to points first.
    /// This is the reading order of an SVG `transform="A B C"` list
    /// (the leftmost matrix is the outermost).
    pub(crate) fn then(&self, next: &Affine) -> Affine {
        Affine {
            a: self.a * next.a + self.c * next.b,
            b: self.b * next.a + self.d * next.b,
            c: self.a * next.c + self.c * next.d,
            d: self.b * next.c + self.d * next.d,
            e: self.a * next.e + self.c * next.f + self.e,
            f: self.b * next.e + self.d * next.f + self.f,
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Inverse (undefined on singular matrices — callers use XForm
    /// chains, which carry non-zero scales).
    pub fn invert(&self) -> Affine {
        let det = self.a * self.d - self.b * self.c;
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        }
    }
}

/// The SVG-space affine the renderer composes for one level of the
/// `chain` (root-first, from `render_shape`/`render_shape_local`): a
/// top-level shape maps its local frame into page SVG space
/// (y-down, origin top-left); a nested shape maps into its parent's
/// local SVG frame with no flip scale of its own.
fn level_affine(doc: &Document, chain: &[&Shape], idx: usize, page_h: f64) -> Affine {
    let s = chain[idx];
    let env = shape_env(doc, s, &chain[..=idx]);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, &chain[..=idx], n)
            .and_then(|c| cell_num(&c, &env))
    };
    let width = get("Width").unwrap_or(1.0);
    let height = get("Height").unwrap_or(1.0);
    let (px, py) = (get("PinX").unwrap_or(0.0), get("PinY").unwrap_or(0.0));
    let (lx, ly) = (
        get("LocPinX").unwrap_or(width / 2.0),
        get("LocPinY").unwrap_or(height / 2.0),
    );
    let deg = -get("Angle").unwrap_or(0.0).to_degrees();
    let t = Affine::translate(px, if idx == 0 { page_h - py } else { -py })
        .then(&Affine::rotate_deg(deg));
    // Flip scale exists only at the page root (group-local rendering
    // applies no flip of its own).
    let t = if idx == 0 {
        let fx = if get("FlipX").unwrap_or(0.0) != 0.0 {
            -1.0
        } else {
            1.0
        };
        let fy = if get("FlipY").unwrap_or(0.0) != 0.0 {
            -1.0
        } else {
            1.0
        };
        t.then(&Affine::scale(fx, fy))
    } else {
        t
    };
    t.then(&Affine::translate(-lx, ly))
}

/// The composed SVG-space affine mapping the LAST chain entry's
/// PARENT-LOCAL frame to page SVG space: `T(root) · … · T(parent)`,
/// the same nesting the renderer emits. An empty/root chain yields
/// identity (a top-level shape's parent frame IS the page).
pub(crate) fn ancestors_affine(doc: &Document, chain: &[&Shape], page_h: f64) -> Affine {
    let mut m = Affine::IDENTITY;
    for idx in 0..chain.len().saturating_sub(1) {
        m = m.then(&level_affine(doc, chain, idx, page_h));
    }
    m
}

/// A shape's bounding box in PAGE coordinates (inches, y-up, origin
/// bottom-left): the shape's parent-local rect mapped through the
/// ancestor XForm chain — the same math the renderer composes, so
/// hit-testing and selection frames land on the drawn pixels.
/// `chain` is root-first and ends with the shape itself.
pub fn shape_page_rect(
    doc: &Document,
    chain: &[&Shape],
    page_h: f64,
) -> Option<(f64, f64, f64, f64)> {
    let s = *chain.last()?;
    let env = shape_env(doc, s, chain);
    let get = |n: &str| -> Option<f64> {
        doc.resolve_cell(s, chain, n)
            .and_then(|c| cell_num(&c, &env))
    };
    let (pin_x, pin_y) = (get("PinX")?, get("PinY")?);
    let w = get("Width").unwrap_or(1.0);
    let h = get("Height").unwrap_or(1.0);
    let lx = get("LocPinX").unwrap_or(w / 2.0);
    let ly = get("LocPinY").unwrap_or(h / 2.0);
    let m = ancestors_affine(doc, chain, page_h);
    // Parent-local y-up → SVG (y-down) → page y-up. With at least one
    // ancestor group the parent frame is a group-local y-down frame
    // (a y-up value enters it as -y); with none the parent frame IS
    // the page, whose own SVG conversion applies on the way in.
    let nested = chain.len() >= 2;
    let corner = |x: f64, y: f64| -> (f64, f64) {
        let iy = if nested { -y } else { page_h - y };
        let (sx, sy) = m.apply(x, iy);
        (sx, page_h - sy)
    };
    let pts = [
        corner(pin_x - lx, pin_y - ly),
        corner(pin_x - lx + w, pin_y - ly),
        corner(pin_x - lx, pin_y - ly + h),
        corner(pin_x - lx + w, pin_y - ly + h),
    ];
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    let (min_x, max_x) = (
        xs.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
    );
    let (min_y, max_y) = (
        ys.clone().fold(f64::INFINITY, f64::min),
        ys.fold(f64::NEG_INFINITY, f64::max),
    );
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}

/// Map a PAGE point (inches, y-up) into the last chain entry's
/// PARENT-LOCAL y-up frame — the inverse of [`shape_page_rect`]'s
/// mapping, used to write PinX/PinY of a nested shape during drags
/// (a member's Pin cells are parent-local by definition).
pub fn page_point_to_parent_local(
    doc: &Document,
    chain: &[&Shape],
    page_h: f64,
    p: (f64, f64),
) -> (f64, f64) {
    let inv = ancestors_affine(doc, chain, page_h).invert();
    let (x, y) = inv.apply(p.0, page_h - p.1);
    if chain.len() >= 2 {
        (x, -y)
    } else {
        (x, page_h - y)
    }
}

/// Map a point in the last chain entry's PARENT-LOCAL y-up frame to
/// PAGE inches y-up — the forward direction of
/// [`page_point_to_parent_local`], used to read a nested member's pin
/// into page space during drags.
pub fn parent_local_point_to_page(
    doc: &Document,
    chain: &[&Shape],
    page_h: f64,
    p: (f64, f64),
) -> (f64, f64) {
    let m = ancestors_affine(doc, chain, page_h);
    let iy = if chain.len() >= 2 { -p.1 } else { page_h - p.1 };
    let (sx, sy) = m.apply(p.0, iy);
    (sx, page_h - sy)
}

