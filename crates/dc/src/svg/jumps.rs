//! Line-jump rendering in page space: the hop arc that bridges a cut,
//! the per-hop clip that opens the jumper's shaft, and the canvas's
//! backdrop gap color (#324 phase 3 — the hop is part of the line:
//! the background shows through beneath it, no paper-white gap and no
//! under-line redraw, #322).
use super::*;
use crate::svg::ink::node_ink_box;
use crate::svg::text::hex_rgb;
use std::fmt::Write as _;

/// The hop arc's normal for a jumper direction (page space, y up):
/// Visio's default hop directions (PageLineJumpDirX/Y = 0) — a
/// horizontal-class jumper bulges UP, a vertical-class one bulges LEFT
/// (the b01 witness; "up" alone flipped arbitrarily on steep lines and
/// drew the sideways zigzags, #322). Shared with the canvas overlay.
pub fn jump_normal(dir: (f64, f64)) -> (f64, f64) {
    let mut n = (-dir.1, dir.0);
    let horizontal = dir.0.abs() >= dir.1.abs();
    if (horizontal && n.1 < 0.0) || (!horizontal && n.0 > 0.0) {
        n = (-n.0, -n.1);
    }
    n
}

/// The clip that cuts a jumping connector open around its hops: the
/// whole page plus, per hop, a slot across the line (2·r along it,
/// generously wide across), combined even-odd so the slots are holes.
/// Coordinates are page-space SVG (y down).
pub(crate) fn jump_cut_clip_d(j: &crate::geom::Jump, page_h: f64, page_w: f64, r: f64) -> String {
    let pad = 4.0 * page_w.max(page_h).max(1.0);
    let mut d = format!(
        "M {:.4} {:.4} L {:.4} {:.4} L {:.4} {:.4} L {:.4} {:.4} Z ",
        -pad,
        -pad,
        page_w + pad,
        -pad,
        page_w + pad,
        page_h + pad,
        -pad,
        page_h + pad
    );
    let (dx, dy) = j.dir;
    let n = (-dy, dx);
    // Across the line: well past the stroke, short of the arc top.
    let half_w = (j.weight * 2.0).max(r * 0.6);
    let c = |a: f64, b: f64| {
        let x = j.at.0 + dx * a + n.0 * b;
        let y = j.at.1 + dy * a + n.1 * b;
        format!("{:.5} {:.5}", x, page_h - y)
    };
    let _ = write!(
        d,
        "M {} L {} L {} L {} Z ",
        c(-r, -half_w),
        c(r, -half_w),
        c(r, half_w),
        c(-r, half_w)
    );
    d.trim_end().to_string()
}

/// One hop's arc in page-space SVG (y-down: `sy = page_h − y`): a
/// semicircle from `at − dir·r` to `at + dir·r` bulging along
/// [`jump_normal`], in the jumper's own color and weight. The jumper
/// itself is cut open beneath it ([`jump_cut_clip_d`]) and the arc
/// feet lead half a pen INTO the un-cut run so the joint closes — no
/// paper gap or under-line redraw is needed (the hop is part of the
/// line, #322/#324 phase 3). `radius` is the page's resolved jump
/// radius (#294 — LineToLineX × LineJumpFactorX / 2).
pub(crate) fn append_jump_scaled(
    svg: &mut String,
    page_h: f64,
    j: &crate::geom::Jump,
    radius: f64,
) {
    use crate::geom::JumpStyle;
    // Gap draws nothing — the cut slot is the whole treatment.
    if let JumpStyle::Gap = j.style {
        return;
    }
    let (r, w) = (radius, j.weight);
    let sy = |y: f64| page_h - y;
    let pt = |p: (f64, f64)| format!("{:.5} {:.5}", p.0, sy(p.1));
    let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    let n = jump_normal(j.dir);
    // The hop overlaps the cut line ends by half a pen so the joint
    // closes (round join, no notch at the foot).
    let lead = |s: f64| {
        (
            j.at.0 + j.dir.0 * s * (r + w * 0.5),
            j.at.1 + j.dir.1 * s * (r + w * 0.5),
        )
    };
    // The bump's vertices between the slot feet, per style: the arc
    // samples the semicircle, the square's top corners sit a full r
    // above the FEET, the n-sided hump takes the inscribed polygon's
    // interior vertices (bump(0)/bump(π) ARE the feet).
    let bump = |ang: f64| {
        (
            j.at.0 - j.dir.0 * r * ang.cos() + n.0 * r * ang.sin(),
            j.at.1 - j.dir.1 * r * ang.cos() + n.1 * r * ang.sin(),
        )
    };
    let apex = bump(std::f64::consts::FRAC_PI_2);
    let mids: Vec<(f64, f64)> = match j.style {
        JumpStyle::Arc => (1..=11)
            .map(|k| bump(std::f64::consts::PI * k as f64 / 12.0))
            .collect(),
        JumpStyle::Square => vec![
            (apex.0 - j.dir.0 * r, apex.1 - j.dir.1 * r),
            (apex.0 + j.dir.0 * r, apex.1 + j.dir.1 * r),
        ],
        JumpStyle::Sides(ns) => (1..ns)
            .map(|k| bump(std::f64::consts::PI * k as f64 / ns as f64))
            .collect(),
        JumpStyle::Gap => Vec::new(),
    };
    let mut path = format!("M {} L {}", pt(lead(-1.0)), pt(bump(0.0)));
    for p in mids {
        path.push_str(" L ");
        path.push_str(&pt(p));
    }
    path.push_str(&format!(
        " L {} L {}",
        pt(bump(std::f64::consts::PI)),
        pt(lead(1.0))
    ));
    let _ = writeln!(
        svg,
        r#"<path data-dc="hop-arc" d="{}" fill="none" stroke="{c}" stroke-width="{w:.4}" stroke-linejoin="round"/>"#,
        path.trim_end(),
        c = hex(j.color),
        w = w,
    );
}

/// The color visible BEHIND a jumping connector at page point `at`
/// (y up): the fill of the topmost filled shape painted before the
/// connector (`below` = the connector's shape id) whose ink box holds
/// the point, else the page's white. The canvas paints its hop gaps in
/// this color — it composites cached per-shape rasters, so it cannot
/// cut the connector's own ink the way the export does (#324 phase 3;
/// the old paper-white gap dashed every hop over a gray chassis
/// panel, #322).
pub fn backdrop_color_at(
    doc: &Document,
    page: &vsdx::Page,
    at: (f64, f64),
    below: i64,
    hidden: &[i64],
) -> (u8, u8, u8) {
    BackdropIndex::new(doc, page).color_at(at, below, hidden)
}

/// A flattened geometry segment, page y-up.
type Seg = ((f64, f64), (f64, f64));

/// A node of [`BackdropIndex`] (one painted shape or group child).
struct IndexNode {
    bx: (f64, f64, f64, f64),
    closed: Option<Vec<Seg>>,
    fill: Option<(String, f64)>,
}

/// A top-level shape of [`BackdropIndex`]; its nodes (walk order) are
/// built on the first query that reaches it.
struct TopNode<'a> {
    shape: &'a Shape,
    layer: Option<i64>,
    guide: bool,
    nodes: std::cell::OnceCell<Vec<IndexNode>>,
}

/// [`backdrop_color_at`]'s per-page work done ONCE (#389): every
/// shape's ink box, top-level contour and fill, in paint order. The
/// canvas builds one per jump pass and asks it per hop; the old per-hop
/// walk recomputed every ink box for every hop, every frame (a page
/// with a few hundred hops spent tens of seconds per frame).
pub struct BackdropIndex<'a> {
    doc: &'a Document,
    page_h: f64,
    tops: Vec<TopNode<'a>>,
}

impl<'a> BackdropIndex<'a> {
    /// Index `page`: the top-level order and layer data now; each
    /// shape's subtree (ink boxes, contour, fill) on the first query
    /// that reaches it (CodeRabbit on PR #391: shapes above the jumper
    /// or behind an opaque panel are never built).
    pub fn new(doc: &'a Document, page: &'a vsdx::Page) -> BackdropIndex<'a> {
        let page_h = num(page.sheet().and_then(|s| s.cell("PageHeight"))).unwrap_or(11.0);
        let tops = page
            .shapes
            .iter()
            .map(|s| TopNode {
                shape: s,
                layer: s
                    .sheet
                    .cell("LayerMember")
                    .and_then(|c| c.numeric())
                    .map(|m| m as i64),
                guide: s.type_.as_deref() == Some("Guide"),
                nodes: std::cell::OnceCell::new(),
            })
            .collect();
        BackdropIndex { doc, page_h, tops }
    }

    /// The color visible behind connector `below` at page point `at`
    /// (y up) — see [`backdrop_color_at`].
    pub fn color_at(&self, at: (f64, f64), below: i64, hidden: &[i64]) -> (u8, u8, u8) {
        let pt = (at.0, self.page_h - at.1);
        let upto = self
            .tops
            .iter()
            .position(|t| t.shape.id == below)
            .unwrap_or(self.tops.len());
        // Topmost-first collection, then the painter's-algorithm blend:
        // each containing fill composites OVER the accumulated lower
        // backdrop until an opaque layer ends the search.
        let mut hits: Vec<(&str, f64)> = Vec::new();
        'shapes: for t in self.tops[..upto].iter().rev() {
            if t.guide {
                continue;
            }
            // Hidden layers leave the compositor — the export omits them,
            // so a hidden panel must not color a gap either (CodeRabbit
            // round on #324 phase 3).
            if t.layer.is_some_and(|m| hidden.contains(&m)) {
                continue;
            }
            let nodes = t.nodes.get_or_init(|| {
                let mut out = Vec::new();
                collect(
                    self.doc,
                    t.shape,
                    &[],
                    self.page_h,
                    true,
                    Affine::IDENTITY,
                    &mut out,
                );
                out
            });
            for n in nodes {
                let bx = n.bx;
                let inside =
                    pt.0 >= bx.0 && pt.0 <= bx.0 + bx.2 && pt.1 >= bx.1 && pt.1 <= bx.1 + bx.3;
                if !inside {
                    continue;
                }
                if let Some(closed) = &n.closed {
                    let q = (pt.0, self.page_h - pt.1); // the cast runs page y-up
                    let mut in_fill = false;
                    for (a, b) in closed {
                        if (a.1 > q.1) != (b.1 > q.1) {
                            let x = a.0 + (q.1 - a.1) / (b.1 - a.1) * (b.0 - a.0);
                            if x > q.0 {
                                in_fill = !in_fill;
                            }
                        }
                    }
                    if !in_fill {
                        continue;
                    }
                }
                if let Some((color, alpha)) = &n.fill {
                    hits.push((color.as_str(), *alpha));
                    // An opaque layer hides everything beneath it — deeper
                    // shapes cannot color the gap.
                    if *alpha >= 0.999 {
                        break 'shapes;
                    }
                }
            }
        }
        // Blend BOTTOM-UP (painter's algorithm): each fill composites over
        // the accumulated LOWER backdrop, so the topmost layer's color wins
        // by its own alpha (CodeRabbit round on #324 phase 3: red over blue
        // read blue).
        let mut acc = (0xffu8, 0xffu8, 0xffu8);
        for (color, alpha) in hits.iter().rev() {
            let c = hex_rgb(color);
            acc = (
                (c.0 as f64 * alpha + acc.0 as f64 * (1.0 - alpha)).round() as u8,
                (c.1 as f64 * alpha + acc.1 as f64 * (1.0 - alpha)).round() as u8,
                (c.2 as f64 * alpha + acc.2 as f64 * (1.0 - alpha)).round() as u8,
            );
        }
        acc
    }
}

/// The fill's color and ALPHA (1 − the chosen cell's
/// transparency): a fully transparent fill is no fill; a partial
/// one composites over whatever is lower (CodeRabbit round on
/// #324 phase 3 — the old 0.5 threshold threw away a 0.75 panel
/// and painted uncomposited color at 0.25).
fn filled(doc: &Document, s: &Shape, chain: &[&Shape]) -> Option<(String, f64)> {
    let sheet = doc.effective_sheet(s, chain);
    let geos = sheet.sections_named("Geometry");
    // A section the renderer skips (NoShow, the same rule
    // `section_flag` reads) contributes no fill to the backdrop
    // (CodeRabbit round on #324 phase 5).
    let env = super::shape_env(doc, s, chain);
    let any_fill = geos.iter().any(|g| {
        // NoShow AND NoFill through the renderer's own
        // `section_flag` rule (formula first, cached V as
        // fallback) — a NoFill formula evaluating to 1 with a
        // stale 0 cache paints nothing (CodeRabbit round on
        // #324 phase 5).
        !super::shape_layer::section_flag(g, "NoShow", &env)
            && !super::shape_layer::section_flag(g, "NoFill", &env)
            && g.rows.iter().any(|r| r.t.is_some())
    });
    if !any_fill {
        return None;
    }
    let num_cell = |n: &str| doc.resolve_cell(s, chain, n).and_then(|c| c.numeric());
    let pattern = num_cell("FillPattern").unwrap_or(1.0);
    if pattern == 0.0 {
        return None;
    }
    // Fill color per the RENDERER's own pattern rules
    // (`hatch_fill`, #352): line hatches 2–7 draw fore strokes
    // over the bkgnd ground (the ground dominates a gap's sample);
    // patterns 8–23 are the dot/dither MIXES the renderer
    // composites (8 ≈ ¾ foreground, the rest ½); 24–40 are
    // gradients — the fore stop stands in (a single-color
    // approximation); 0/1 are the solid fore (CodeRabbit round on
    // #324 phase 5: pattern 8's backdrop used to sample the pure
    // background against a rendered mix).
    let pat = pattern.round() as i64;
    // The painter's transparency rule (render_geometry): every
    // non-gradient pattern 2-23 — line hatches AND the mixes —
    // takes FillBkgndTrans; solids keep FillForegndTrans
    // (CodeRabbit round on #324 phase 5: an opaque mixed fill was
    // discarded in favor of the lower backdrop).
    let alpha = 1.0
        - num_cell(if (2..=23).contains(&pat) {
            "FillBkgndTrans"
        } else {
            "FillForegndTrans"
        })
        .unwrap_or(0.0);
    if alpha <= 0.0 {
        return None;
    }
    if (8..=23).contains(&pat) {
        let k = if pat == 8 { 0.75 } else { 0.5 };
        let (fa, fb) = (
            hex_rgb(&themed_color_string(doc, s, chain, "FillForegnd")),
            hex_rgb(&themed_color_string(doc, s, chain, "FillBkgnd")),
        );
        let mix = |x: u8, y: u8| (f64::from(x) * k + f64::from(y) * (1.0 - k)).round() as u8;
        return Some((
            format!(
                "#{:02x}{:02x}{:02x}",
                mix(fa.0, fb.0),
                mix(fa.1, fb.1),
                mix(fa.2, fb.2)
            ),
            alpha,
        ));
    }
    let cell = if (2..=7).contains(&pat) {
        "FillBkgnd"
    } else {
        "FillForegnd"
    };
    Some((themed_color_string(doc, s, chain, cell), alpha))
}
/// The walk, run ONCE per top-level shape: every node in the order the old
/// per-hop walk visited it (later children first, then the parent),
/// so a lookup reproduces its hits exactly (#389: the per-hop walk
/// recomputed every ink box for every hop, every frame).
#[allow(clippy::too_many_arguments)] // the walk carries its whole environment
fn collect(
    doc: &Document,
    s: &Shape,
    chain: &[&Shape],
    page_h: f64,
    top: bool,
    to_page: Affine,
    out: &mut Vec<IndexNode>,
) {
    let mut chain: Vec<&Shape> = chain.to_vec();
    chain.push(s);
    let (bx, a) = node_ink_box(doc, s, &chain, page_h, top, to_page);
    // Later children paint over earlier ones and over the parent.
    for sub in s.sub_shapes.iter().rev() {
        collect(doc, sub, &chain, page_h, false, a, out);
    }
    // The ink box only REJECTS — a triangle's empty corner is
    // inside its box but outside its fill, and the gap would paint
    // a color nothing draws there (CodeRabbit round on #324 phase
    // 3). TOP-LEVEL shapes get the exact test: an even-odd ray
    // cast over the same flattened geometry the crossing set
    // walks. Group children keep the box alone — their frames are
    // parent-local here, and panels behind hops are top-level.
    let closed = if top {
        let msize = s
            .master
            .and_then(|mid| doc.master(mid))
            .and_then(|m| m.shapes.first())
            .map(|root| {
                let g = |n: &str| {
                    doc.resolve_cell(root, &[root], n)
                        .and_then(|c| c.numeric())
                        .unwrap_or(1.0)
                };
                (g("Width"), g("Height"))
            })
            .unwrap_or((1.0, 1.0));
        let frame = crate::geom::hit_frame(doc, &chain, s, msize);
        let segs = crate::geom::geometry_hit_segments(s, chain.get(1).copied(), &frame, None);
        (!segs.is_empty()).then(|| close_contours(&segs))
    } else {
        None
    };
    out.push(IndexNode {
        bx,
        closed,
        fill: filled(doc, s, &chain),
    });
}
/// CLOSE each contour before the cast: the renderer fills the
/// implied closing edge too (an open M→L→L triangle fills its
/// diagonal), and the open cast counted crossings without it.
/// Contours break at pen-ups — a segment that does not begin where
/// the previous one ended — and never join across a MoveTo
/// (CodeRabbit round on #324 phase 3).
fn close_contours(segs: &[Seg]) -> Vec<Seg> {
    let mut closed: Vec<Seg> = Vec::new();
    let mut cur_start: Option<(f64, f64)> = None;
    let mut prev_end: Option<(f64, f64)> = None;
    for (a, b) in segs {
        match (cur_start, prev_end) {
            (None, _) => cur_start = Some(*a),
            (Some(st), Some(pe)) => {
                if (pe.0 - a.0).abs() > 1e-9 || (pe.1 - a.1).abs() > 1e-9 {
                    closed.push((pe, st));
                    cur_start = Some(*a);
                }
            }
            (Some(st), None) => cur_start = Some(st),
        }
        closed.push((*a, *b));
        prev_end = Some(*b);
    }
    if let (Some(st), Some(pe)) = (cur_start, prev_end) {
        if (pe.0 - st.0).abs() > 1e-9 || (pe.1 - st.1).abs() > 1e-9 {
            closed.push((pe, st));
        }
    }
    closed
}
