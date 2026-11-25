//! Tests for the ink-bounds walks (subtree boxes, focus gating).
use super::*;

/// #219: the focused bounds of a sub-shape's own subtree — the
/// damage region a localized edit re-rasters — stay inside the
/// whole-subtree bounds, equal the member's own box (plus the
/// stroke pad), and move with the member's cells.
#[test]
fn focused_bounds_scope_the_walk_to_one_subtree() {
    let mut member = shape_at(2, (0.4, 0.3), 0.5, 0.4, 0.0);
    push_geo(
        &mut member,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "LineTo", &[("X", 0.5), ("Y", 0.0)]),
            geo_row(3, "LineTo", &[("X", 0.5), ("Y", 0.4)]),
            geo_row(4, "LineTo", &[("X", 0.0), ("Y", 0.4)]),
            geo_row(5, "LineTo", &[("X", 0.0), ("Y", 0.0)]),
        ],
    );
    let mut group = shape_at(1, (2.0, 2.0), 2.0, 1.5, 0.0);
    group.type_ = Some("Group".into());
    let mut far = shape_at(3, (1.8, 1.3), 0.4, 0.3, 0.0);
    push_geo(
        &mut far,
        vec![
            geo_row(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
            geo_row(2, "LineTo", &[("X", 0.4), ("Y", 0.3)]),
        ],
    );
    group.sub_shapes = vec![member, far];
    let (doc, page) = page_fixture(vec![group]);

    let whole = shape_ink_bounds(&doc, &page, 1).unwrap();
    let focused = shape_ink_bounds_focused(&doc, &page, 1, Some(2)).unwrap();
    // The member's box (0.5×0.4 at group-local (0.15..0.65, …))
    // plus the same stroke-bleed pad — not the whole group.
    assert!((focused.2 - (0.5 + 0.16)).abs() < 1e-9, "w={}", focused.2);
    assert!((focused.3 - (0.4 + 0.16)).abs() < 1e-9, "h={}", focused.3);
    // Containment: the focused region sits inside the subtree's.
    assert!(focused.0 >= whole.0 - 1e-9 && focused.1 >= whole.1 - 1e-9);
    assert!(focused.0 + focused.2 <= whole.0 + whole.2 + 1e-9);
    assert!(focused.1 + focused.3 <= whole.1 + whole.3 + 1e-9);

    // The walk is honest about WHICH member: the far sibling's
    // focused bounds sit elsewhere.
    let far_focus = shape_ink_bounds_focused(&doc, &page, 1, Some(3)).unwrap();
    assert!((far_focus.0 - focused.0).abs() > 0.5);
    // An unknown focus yields nothing.
    assert!(shape_ink_bounds_focused(&doc, &page, 1, Some(99)).is_none());
}

/// Text taller than its frame: the ink box covers the overflow on
/// the side the alignment pushes it to (CodeRabbit on PR #325).
/// Top-aligned overflows downward only, bottom-aligned upward only.
#[test]
fn ink_bounds_cover_aligned_text_overflow() {
    let bounds = |va: &str| {
        let mut s = shape_at(1, (4.0, 5.0), 1.0, 1.0, 0.0);
        s.sheet.text = Some("1\n2\n3\n4\n5\n6\n7".into());
        s.sheet.set_cell("VerticalAlign", Cell::value(va));
        let (doc, page) = page_fixture(vec![s]);
        let (_x, y, _w, h) = shape_ink_bounds(&doc, &page, 1).unwrap();
        (y, y + h)
    };
    // The frame spans y 5.5..6.5 (page_h 11, y-down).
    let (t0, t1) = bounds("0");
    assert!(
        (t0 - (5.5 - PAD)).abs() < 1e-9,
        "top stays at the frame: {t0}"
    );
    assert!(t1 > 6.5 + PAD + 0.1, "top-aligned overflow below: {t1}");
    let (b0, b1) = bounds("2");
    assert!(b0 < 5.5 - PAD - 0.1, "bottom-aligned overflow above: {b0}");
    assert!(
        (b1 - (6.5 + PAD)).abs() < 1e-9,
        "bottom stays at the frame: {b1}"
    );
    let (m0, m1) = bounds("1");
    assert!(
        ((5.5 - m0) - (m1 - 6.5)).abs() < 1e-9 && m1 > 6.5 + PAD,
        "middle overflows both ways evenly: {m0}..{m1}"
    );
}

#[test]
fn ink_bounds_cover_rotation() {
    // 2×1 shape rotated 90° about its pin (the Angle cell is
    // radians, like render_shape reads it): the ink swaps extents —
    // a plain W×H box (what master_bbox computes) would clip it.
    let (doc, page) = page_fixture(vec![shape_at(
        1,
        (4.0, 5.0),
        2.0,
        1.0,
        std::f64::consts::FRAC_PI_2,
    )]);
    let (x, y, w, h) = shape_ink_bounds(&doc, &page, 1).unwrap();
    assert!((w - (1.0 + 2.0 * PAD)).abs() < 1e-9, "w={w}");
    assert!((h - (2.0 + 2.0 * PAD)).abs() < 1e-9, "h={h}");
    // Same center as the unrotated box: pin (4,5), page_h 11.
    assert!((x + w / 2.0 - 4.0).abs() < 1e-9);
    assert!((y + h / 2.0 - 6.0).abs() < 1e-9);
}

#[test]
fn ink_bounds_union_subshapes_and_text() {
    // A 2×1 group whose sub-shape sits beyond the group's right
    // edge (local pin 2.5 of a 2-wide box) and whose text block is
    // wider than the shape: the union must cover both.
    let mut group = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    group
        .sub_shapes
        .push(shape_at(2, (2.5, 0.8), 1.0, 1.0, 0.0));
    group.sheet.text = Some("hello".into());
    group.sheet.set_cell("TxtWidth", Cell::value("3"));
    let (doc, page) = page_fixture(vec![group]);
    let (x, _y, w, h) = shape_ink_bounds(&doc, &page, 1).unwrap();
    // Group box page x∈[3,5]; sub-shape local x∈[2,3] → page
    // x∈[5,6]; text 3 wide centered → x∈[2.5,5.5].
    let right = x + w - PAD;
    assert!((right - 6.0).abs() < 1e-9, "right={right}");
    let left = x + PAD;
    assert!((left - 2.5).abs() < 1e-9, "left={left}");
    assert!(h > 1.0, "h={h}");
}

#[test]
fn ink_bounds_cover_outdented_bullets() {
    // A bulleted block with IndFirst = −1 hangs its glyph a full
    // inch left of the text area — outside the 2-wide shape's box
    // AND its text frame (local x0 = 0 here). The ink box must
    // reach the glyph: the layer viewBox and the #236 cull are
    // built from these bounds, and a frame-only bound would clip
    // the bullet and skip the shape on a cull that covers only
    // the overhang (local CodeRabbit round 3).
    let mut s = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    s.sheet.text = Some("bullet".into());
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![
                ("Bullet".to_string(), Cell::value("1")),
                ("IndFirst".to_string(), Cell::value("-1")),
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let (doc, page) = page_fixture(vec![s.clone()]);
    let (x, _y, _w, _h) = shape_ink_bounds(&doc, &page, 1).unwrap();
    // The shape box spans page x∈[3,5] (frame x0 = 0 local); the
    // bullet reaches local ins_l − 1 − its advance. Assert the
    // bound's left edge tracks that escape exactly (PAD included).
    let chain = [&s];
    let tb = resolve_text_block(&doc, &s, &chain, 2.0, 1.0);
    let b = tb.bullet.as_ref().expect("the block is bulleted");
    let want_left = 3.0 + (tb.insets.0 + tb.ind_first) - bullet_advance_in(&tb, b) - PAD;
    assert!(
        (x - want_left).abs() < 1e-9,
        "left edge covers the outdented bullet: {x} vs {want_left}"
    );
    // The cull regression: a rect covering ONLY the bullet
    // overhang intersects the ink bounds (the shape must not be
    // skipped when just its bullet is damaged) — vertically
    // inside the box, horizontally past the shape box's left.
    let overhang = (want_left, _y + _h / 2.0 - 0.05, 0.2, 0.1);
    assert!(
        boxes_intersect((x, _y, _w, _h), overhang),
        "a bullet-only rect still intersects the ink bounds"
    );
}

/// The rotated twin (final local CodeRabbit pass + Greptile round
/// 8): under TxtAngle ≠ 0 the bounds must CONTAIN the rotated
/// EXTENDED frame — the old centered circumscribed square covered
/// only half of the one-sided bullet escape (and under-covered
/// oblique rotations of the frame itself). The corners are
/// rotated here independently of the production code (SVG rotate
/// about the TxtPin, net = −TxtAngle at no flip).
#[test]
fn ink_bounds_cover_outdented_bullets_rotated() {
    let mut s = shape_at(1, (4.0, 5.0), 2.0, 1.0, 0.0);
    s.sheet.text = Some("bullet".into());
    s.sheet.set_cell(
        "TxtAngle",
        Cell::value(std::f64::consts::FRAC_PI_2.to_string()),
    );
    s.sheet.children.push(SheetChild::Section(Section {
        name: "Paragraph".into(),
        rows: vec![Row {
            cells: vec![
                ("Bullet".to_string(), Cell::value("1")),
                ("IndFirst".to_string(), Cell::value("-1")),
            ],
            ..Row::default()
        }],
        ..Section::default()
    }));
    let (doc, page) = page_fixture(vec![s.clone()]);
    let (bx, by, bw, bh) = shape_ink_bounds(&doc, &page, 1).unwrap();
    // The escape: bullet origin 1 in left of the text area's left
    // inset, plus the glyph's own advance.
    let chain = [&s];
    let tb = resolve_text_block(&doc, &s, &chain, 2.0, 1.0);
    let b = tb.bullet.as_ref().expect("the block is bulleted");
    let e = -(tb.insets.0 + tb.ind_first) + bullet_advance_in(&tb, b);
    // Extended frame corners, shape-local y-down: [−e, 2] × [−1, 0]
    // (frame x0 = 0, th = 1). Net rotation −90° about the TxtPin
    // (1, −0.5): (dx, dy) → (dy, −dx). Shape-local → page: the
    // pin line (local y = 0) maps to page y 6.5, x = 3 + local.
    let pivot = (1.0, -0.5);
    let corners = [(-e, -1.0), (2.0, -1.0), (2.0, 0.0), (-e, 0.0)];
    let mut pts = Vec::new();
    for (cx, cy) in corners {
        let (dx, dy) = (cx - pivot.0, cy - pivot.1);
        let (rx, ry) = (pivot.0 + dy, pivot.1 - dx);
        pts.push((3.0 + rx, 6.5 + ry));
    }
    // The bounds must equal the EXACT hull of the rotated corners
    // union the shape box (page [3,5]×[5.5,6.5]) plus PAD —
    // containment alone let the round-8 ORDER bug pass (the
    // backwards composition built a giant hull around a
    // shape-local pivot that contained everything; Greptile
    // round 9).
    let hull_x = (
        pts.iter()
            .map(|p| p.0)
            .fold(f64::INFINITY, f64::min)
            .min(3.0),
        pts.iter()
            .map(|p| p.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .max(5.0),
    );
    let hull_y = (
        pts.iter()
            .map(|p| p.1)
            .fold(f64::INFINITY, f64::min)
            .min(5.5),
        pts.iter()
            .map(|p| p.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .max(6.5),
    );
    assert!(
        (bx - (hull_x.0 - PAD)).abs() < 1e-9
            && ((bx + bw) - (hull_x.1 + PAD)).abs() < 1e-9
            && (by - (hull_y.0 - PAD)).abs() < 1e-9
            && ((by + bh) - (hull_y.1 + PAD)).abs() < 1e-9,
            "the bounds are the exact rotated hull + PAD: got \
             [{bx:.4},{by:.4} {bw:.4}×{bh:.4}], want [{:.4},{:.4} \
         {:.4}×{:.4}] (e={e:.4})",
        hull_x.0 - PAD,
        hull_y.0 - PAD,
        hull_x.1 - hull_x.0 + 2.0 * PAD,
        hull_y.1 - hull_y.0 + 2.0 * PAD
    );
    // …and the fixture BITES on both prior forms: the far corner
    // sits past the old PIN-centered square's reach (the old
    // square centered on the TxtPin's page point (4, 6.0))…
    let far = pts[0].1;
    let old_reach = 6.0 + (2.0 + e) / 2.0;
    assert!(
        far > old_reach,
            "fixture: the far corner {far:.3} escapes the old square's \
         reach {old_reach:.3}"
    );
    // …and a wrong-order hull would tower over the tight one.
    assert!(
        bh < 6.0,
            "fixture: the tight hull is a few inches, not a giant \
         wrong-order hull (h={bh:.3})"
    );
}

/// The bounds walk the same inherited vertices the renderer draws:
/// a master vertex 3 in above the baseline widens the ink box
/// (Greptile on PR #329).
#[test]
fn inherited_polyline_vertices_bound_the_ink() {
    let row = geo_row(2, "PolylineTo", &[("X", 4.0), ("Y", 0.0)]);
    let (doc, page) = polyline_master_and_instance(row, "POLYLINE(1, 1, 0,3, 2,3)");
    let b = subtree_ink_boxes(&doc, &page, 1).unwrap()[&1];
    // (x, y, width, height): the shape is 1 in tall, the vertices
    // reach 3 in.
    assert!(b.3 > 2.5, "the box reaches the vertices: {b:?}");
}
