//! Tests for the page render entry points.
use super::*;

/// GitHub #22's no-ghost guarantee: `render_page_no_text` drops
/// exactly the named shape's text and keeps everything else.
#[test]
fn render_page_no_text_suppresses_only_that_shape() {
    let mut a = shape_at(1, (2.0, 8.0), 1.0, 1.0, 0.0);
    a.sheet.text = Some("alpha".into());
    let mut b = shape_at(2, (6.0, 8.0), 1.0, 1.0, 0.0);
    b.sheet.text = Some("beta".into());
    let (doc, page) = page_fixture(vec![a, b]);
    let full = render_page(&doc, &page).unwrap();
    assert!(full.contains("alpha") && full.contains("beta"));
    let skipped = render_page_no_text(&doc, &page, &[], 1).unwrap();
    assert!(!skipped.contains("alpha"), "edited shape's text is gone");
    assert!(skipped.contains("beta"), "other shapes keep their text");
    assert!(skipped.contains("<g transform"), "geometry keeps rendering");
}
