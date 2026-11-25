//! Tests for shape_page_rect and the parent-local point mappings.
use super::*;

#[test]
fn member_page_rect_unrotated_group() {
    // Group 2×4 at Pin (2,3), LocPin centered by default (W=4,H=2
    // → LocPin (2,1)), on an 11-inch page. A member rect of
    // (0.5,0.5,1,1) in group-local y-up maps to the same numbers
    // in page space (translate-only: group-local (0..4, 0..2) ↔
    // page (0..4, 2..4)).
    let doc = Document::default();
    let mut g = xform_shape(&[
        ("PinX", 2.0),
        ("PinY", 3.0),
        ("Width", 4.0),
        ("Height", 2.0),
    ]);
    g.id = 9;
    g.type_ = Some("Group".into());
    let mut m = unit_member();
    for (n, v) in [
        ("PinX", 1.0),
        ("PinY", 1.0),
        ("Width", 1.0),
        ("Height", 1.0),
    ] {
        m.sheet.set_cell(
            n,
            Cell {
                v: Some(v.to_string()),
                ..Cell::default()
            },
        );
    }
    let chain = vec![&g, &m];
    assert_eq!(
        shape_page_rect(&doc, &chain, 11.0),
        Some((0.5, 2.5, 1.0, 1.0))
    );
}

#[test]
fn member_page_rect_rotated_group() {
    // The group rotated +90° (page CCW): a member at group-local
    // (1,1) has offset (−1,0) from the LocPin (2,1); CCW rotation
    // maps that to (0,−1), so the member center lands at page
    // Pin(4,5)+(0,−1) = (4,4) and its axis-aligned unit rect is
    // (3.5, 3.5, 1, 1).
    let doc = Document::default();
    let mut g = xform_shape(&[
        ("PinX", 4.0),
        ("PinY", 5.0),
        ("Width", 4.0),
        ("Height", 2.0),
        ("Angle", std::f64::consts::FRAC_PI_2),
    ]);
    g.id = 9;
    g.type_ = Some("Group".into());
    let mut m = unit_member();
    for (n, v) in [
        ("PinX", 1.0),
        ("PinY", 1.0),
        ("Width", 1.0),
        ("Height", 1.0),
    ] {
        m.sheet.set_cell(
            n,
            Cell {
                v: Some(v.to_string()),
                ..Cell::default()
            },
        );
    }
    let chain = vec![&g, &m];
    assert_eq!(
        shape_page_rect(&doc, &chain, 11.0),
        Some((3.5, 3.5, 1.0, 1.0))
    );
}

#[test]
fn page_point_round_trips_through_parent_local() {
    let doc = Document::default();
    let mut g = xform_shape(&[
        ("PinX", 4.0),
        ("PinY", 5.0),
        ("Width", 4.0),
        ("Height", 2.0),
        ("Angle", std::f64::consts::FRAC_PI_2),
    ]);
    g.id = 9;
    g.type_ = Some("Group".into());
    let m = unit_member();
    let chain = vec![&g, &m];
    for p in [(5.0, 4.0), (0.0, 0.0), (7.5, 9.25)] {
        let local = page_point_to_parent_local(&doc, &chain, 11.0, p);
        let (sx, sy) = ancestors_affine(&doc, &chain, 11.0).apply(local.0, -local.1);
        assert!((sx - p.0).abs() < 1e-9 && (11.0 - sy - p.1).abs() < 1e-9);
        // And back through the forward helper.
        let back = parent_local_point_to_page(&doc, &chain, 11.0, local);
        assert!((back.0 - p.0).abs() < 1e-9 && (back.1 - p.1).abs() < 1e-9);
    }
    // A top-level chain (no ancestors) is the identity in page
    // space: the parent frame IS the page.
    let top = vec![&g];
    let q = (3.25, 8.5);
    assert_eq!(parent_local_point_to_page(&doc, &top, 11.0, q), q);
}

#[test]
fn top_level_rect_is_identity_composition() {
    // A top-level shape's parent frame is the page: the affine is
    // identity and shape_page_rect matches the plain local rect.
    let doc = Document::default();
    let s = xform_shape(&[
        ("PinX", 3.0),
        ("PinY", 4.0),
        ("Width", 2.0),
        ("Height", 1.0),
    ]);
    let chain = vec![&s];
    assert_eq!(
        shape_page_rect(&doc, &chain, 11.0),
        Some((2.0, 3.5, 2.0, 1.0))
    );
}

// ---- #93: per-shape layer bounds + renderer ----
