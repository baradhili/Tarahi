//! The find-in-drawing search engine (GitHub #115, phase 1): one test
//! per target class × scope × option, over in-code documents — the
//! `dc-core` harness pattern (`tests/spell.rs`). The contract under
//! test is the issue's phase-1 spec: matches in each target class
//! (shape text, shape data, shape name, user-defined cell), honoring
//! scope (Selection / Current page / All pages) and options (match
//! case, whole words only), with nested `sub_shapes` traversed and
//! the deterministic Find-Next order (page order, shape-tree order,
//! target class).

use dc_core::find::{
    find_matches, FindOptions, FindScope, FindTarget, FindTargetClass, FindTargets,
};
use dc_core::{Command as _, ReplaceTextRange, Scope};
use vsdx::model::{
    Cell, Document, IndexChild, Master, Page, Row, Section, Shape, Sheet, SheetChild,
};

// -- fixtures ---------------------------------------------------------

fn page(id: i64, shapes: Vec<Shape>) -> Page {
    Page {
        id,
        name: format!("P{id}"),
        attrs: vec![
            ("ID".into(), id.to_string()),
            ("NameU".into(), format!("P{id}")),
        ],
        children: vec![IndexChild::Sheet {
            tag: "PageSheet".into(),
            sheet: Sheet::default(),
        }],
        shapes,
        ..Page::default()
    }
}

fn doc(pages: Vec<Page>) -> Document {
    Document {
        pages,
        ..Document::default()
    }
}

/// A shape with plain text (`text_raw` seed, the package-parsed form).
fn text_shape(id: i64, name_u: Option<&str>, raw: &str) -> Shape {
    Shape {
        id,
        name_u: name_u.map(str::to_string),
        type_: Some("Shape".into()),
        sheet: Sheet {
            text_raw: Some(raw.to_string()),
            text: Some(raw.to_string()),
            ..Sheet::default()
        },
        ..Shape::default()
    }
}

fn group(id: i64, name_u: Option<&str>, sub_shapes: Vec<Shape>) -> Shape {
    Shape {
        id,
        name_u: name_u.map(str::to_string),
        type_: Some("Group".into()),
        sub_shapes,
        ..Shape::default()
    }
}

fn value_row(name: &str, value: &str) -> Row {
    Row {
        n: Some(name.to_string()),
        cells: vec![("Value".into(), Cell::value(value))],
        ..Row::default()
    }
}

fn deleted_value_row(name: &str, value: &str) -> Row {
    Row {
        attrs: vec![("Del".into(), "1".into())],
        ..value_row(name, value)
    }
}

fn section(name: &str, rows: Vec<Row>) -> SheetChild {
    SheetChild::Section(Section {
        name: name.to_string(),
        rows,
        ..Section::default()
    })
}

/// A shape carrying text plus `Prop`/`User` rows.
fn rich_shape(
    id: i64,
    name_u: Option<&str>,
    raw: Option<&str>,
    prop: Vec<Row>,
    user: Vec<Row>,
) -> Shape {
    let mut children = Vec::new();
    if !prop.is_empty() {
        children.push(section("Prop", prop));
    }
    if !user.is_empty() {
        children.push(section("User", user));
    }
    Shape {
        id,
        name_u: name_u.map(str::to_string),
        type_: Some("Shape".into()),
        sheet: Sheet {
            text_raw: raw.map(str::to_string),
            text: raw.map(str::to_string),
            children,
            ..Sheet::default()
        },
        ..Shape::default()
    }
}

/// A page instance of master `mid` (no local sheet content).
fn instance(id: i64, mid: i64) -> Shape {
    Shape {
        id,
        master: Some(mid),
        type_: Some("Shape".into()),
        ..Shape::default()
    }
}

fn all_targets() -> FindOptions {
    FindOptions {
        targets: FindTargets::all(),
        ..FindOptions::default()
    }
}

/// Every match as `(page, shape, class, row-or-None)` — compact to
/// assert whole ordering vectors at once.
fn brief(ms: &[dc_core::FindMatch]) -> Vec<(i64, i64, FindTargetClass, Option<&str>)> {
    ms.iter()
        .map(|m| {
            let row = match &m.target {
                FindTarget::ShapeData { row, .. } | FindTarget::UserCell { row, .. } => {
                    Some(row.as_str())
                }
                _ => None,
            };
            (m.page, m.shape, m.target.class(), row)
        })
        .collect()
}

// -- scope × text ------------------------------------------------------

#[test]
fn text_all_pages_in_page_and_tree_order() {
    let d = doc(vec![
        page(
            0,
            vec![
                text_shape(1, None, "hit one"),
                group(
                    2,
                    None,
                    vec![
                        text_shape(3, None, "hit two"),
                        text_shape(4, None, "hit three"),
                    ],
                ),
                text_shape(5, None, "miss"),
            ],
        ),
        page(7, vec![text_shape(6, None, "hit four")]),
    ]);
    let got = find_matches(&d, "hit", FindScope::AllPages, &FindOptions::default());
    // Page order (0 before 7), shape-tree pre-order (1, then the
    // group's members 3 and 4), miss excluded.
    assert_eq!(
        brief(&got),
        vec![
            (0, 1, FindTargetClass::Text, None),
            (0, 3, FindTargetClass::Text, None),
            (0, 4, FindTargetClass::Text, None),
            (7, 6, FindTargetClass::Text, None),
        ]
    );
}

#[test]
fn text_scope_current_page_only() {
    let d = doc(vec![
        page(0, vec![text_shape(1, None, "hit here")]),
        page(7, vec![text_shape(2, None, "hit there")]),
    ]);
    let got = find_matches(
        &d,
        "hit",
        FindScope::CurrentPage(7),
        &FindOptions::default(),
    );
    assert_eq!(brief(&got), vec![(7, 2, FindTargetClass::Text, None)]);
    // A page id nothing owns matches nothing.
    assert!(find_matches(
        &d,
        "hit",
        FindScope::CurrentPage(9),
        &FindOptions::default()
    )
    .is_empty());
}

#[test]
fn text_scope_selection_includes_the_group_subtree() {
    let d = doc(vec![page(
        0,
        vec![
            text_shape(1, None, "hit one"),
            group(
                2,
                None,
                vec![
                    text_shape(3, None, "hit two"),
                    text_shape(4, None, "hit three"),
                ],
            ),
            text_shape(5, None, "hit four"),
        ],
    )]);
    // Selecting the group searches its members; the rest of the page
    // stays out.
    let got = find_matches(
        &d,
        "hit",
        FindScope::Selection(&[(0, 2)]),
        &FindOptions::default(),
    );
    assert_eq!(
        brief(&got),
        vec![
            (0, 3, FindTargetClass::Text, None),
            (0, 4, FindTargetClass::Text, None),
        ]
    );
    // Selecting one sub-shape (group-edit style) searches only it.
    let got = find_matches(
        &d,
        "hit",
        FindScope::Selection(&[(0, 4)]),
        &FindOptions::default(),
    );
    assert_eq!(brief(&got), vec![(0, 4, FindTargetClass::Text, None)]);
    // A multi-selection keeps tree order.
    let got = find_matches(
        &d,
        "hit",
        FindScope::Selection(&[(0, 5), (0, 1)]),
        &FindOptions::default(),
    );
    assert_eq!(
        brief(&got),
        vec![
            (0, 1, FindTargetClass::Text, None),
            (0, 5, FindTargetClass::Text, None),
        ]
    );
    // Selection on another page leaves this page alone.
    let got = find_matches(
        &d,
        "hit",
        FindScope::Selection(&[(3, 1)]),
        &FindOptions::default(),
    );
    assert!(got.is_empty());
}

// -- target classes ----------------------------------------------------

#[test]
fn default_targets_are_shape_text_only() {
    let d = doc(vec![page(
        0,
        vec![rich_shape(
            1,
            Some("alpha"),
            Some("alpha text"),
            vec![value_row("k", "alpha")],
            vec![value_row("u", "alpha")],
        )],
    )]);
    let got = find_matches(&d, "alpha", FindScope::AllPages, &FindOptions::default());
    assert_eq!(brief(&got), vec![(0, 1, FindTargetClass::Text, None)]);
}

#[test]
fn target_class_order_within_a_shape_is_text_data_name_user() {
    let d = doc(vec![page(
        0,
        vec![rich_shape(
            1,
            Some("alpha"),
            Some("alpha beta alpha"),
            vec![value_row("k", "alpha")],
            vec![value_row("u", "alpha")],
        )],
    )]);
    let got = find_matches(&d, "alpha", FindScope::AllPages, &all_targets());
    // Spec 2's checkbox order inside one shape: text spans ascending,
    // then shape data, then shape name, then user-defined cell.
    assert_eq!(
        brief(&got),
        vec![
            (0, 1, FindTargetClass::Text, None),
            (0, 1, FindTargetClass::Text, None),
            (0, 1, FindTargetClass::ShapeData, Some("k")),
            (0, 1, FindTargetClass::ShapeName, None),
            (0, 1, FindTargetClass::UserCell, Some("u")),
        ]
    );
    // The two text matches are the plain spans, in order.
    assert_eq!(got[0].target, FindTarget::Text { start: 0, len: 5 });
    assert_eq!(got[1].target, FindTarget::Text { start: 11, len: 5 });
}

#[test]
fn shape_data_matches_prop_values_with_row_name_and_span() {
    let d = doc(vec![page(
        0,
        vec![rich_shape(
            1,
            None,
            None,
            vec![
                value_row("Owner", "Ada hit"),
                value_row("Note", "hit and hit again"),
            ],
            vec![],
        )],
    )]);
    let opts = FindOptions {
        targets: FindTargets {
            shape_data: true,
            ..FindTargets::default()
        },
        ..FindOptions::default()
    };
    let got = find_matches(&d, "hit", FindScope::AllPages, &opts);
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![
            FindTarget::ShapeData {
                row: "Owner".into(),
                start: 4,
                len: 3
            },
            FindTarget::ShapeData {
                row: "Note".into(),
                start: 0,
                len: 3
            },
            FindTarget::ShapeData {
                row: "Note".into(),
                start: 8,
                len: 3
            },
        ]
    );
}

#[test]
fn user_cell_matches_user_values() {
    let d = doc(vec![page(
        0,
        vec![rich_shape(
            1,
            None,
            None,
            vec![],
            vec![value_row("Status", "hit ready")],
        )],
    )]);
    let opts = FindOptions {
        targets: FindTargets {
            user_cell: true,
            ..FindTargets::default()
        },
        ..FindOptions::default()
    };
    let got = find_matches(&d, "hit", FindScope::AllPages, &opts);
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![FindTarget::UserCell {
            row: "Status".into(),
            start: 0,
            len: 3
        }]
    );
}

#[test]
fn shape_name_matches_name_u() {
    let d = doc(vec![page(
        0,
        vec![text_shape(1, Some("hit box"), "unrelated text")],
    )]);
    let opts = FindOptions {
        targets: FindTargets {
            shape_name: true,
            ..FindTargets::default()
        },
        ..FindOptions::default()
    };
    let got = find_matches(&d, "hit", FindScope::AllPages, &opts);
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![FindTarget::ShapeName { start: 0, len: 3 }]
    );
}

// -- options -----------------------------------------------------------

#[test]
fn match_case_option_distinguishes() {
    let d = doc(vec![page(
        0,
        vec![text_shape(1, None, "Draft draft DRAFT")],
    )]);
    // Default: case-insensitive — all three.
    let got = find_matches(&d, "draft", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 3);
    // Match case: only the lowercase one.
    let opts = FindOptions {
        match_case: true,
        ..FindOptions::default()
    };
    let got = find_matches(&d, "draft", FindScope::AllPages, &opts);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target, FindTarget::Text { start: 6, len: 5 });
    // Case-insensitivity folds per character: "GROSS" finds "gross",
    // while "ß" never folds to "SS" under the simple fold (a full
    // string fold would change the char count and shift every span).
    let d = doc(vec![page(0, vec![text_shape(1, None, "Größe gross")])]);
    let got = find_matches(&d, "GROSS", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 1);
}

#[test]
fn whole_words_option_requires_word_edges() {
    let d = doc(vec![page(
        0,
        vec![text_shape(
            1,
            None,
            "connect. connector connects connect_size reconnect connect",
        )],
    )]);
    let opts = FindOptions {
        whole_words: true,
        ..FindOptions::default()
    };
    let got = find_matches(&d, "connect", FindScope::AllPages, &opts);
    // Only the punctuation-bounded and edge occurrences: the
    // underscore neighbor and every suffix/prefix derivation stay
    // out.
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].target, FindTarget::Text { start: 0, len: 7 });
    assert_eq!(got[1].target, FindTarget::Text { start: 51, len: 7 });
    // Without the option, everything matches.
    let got = find_matches(&d, "connect", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 6);
}

// -- text projection ---------------------------------------------------

#[test]
fn field_cached_display_is_searched_and_spans_feed_replace_text_range() {
    // The plain projection includes the field's cached display; the
    // span coordinates are the ones ReplaceTextRange splices — the
    // marker survives byte-identical (never SetText's flattening
    // store).
    let raw = "Draft <fld IX='0'>1.5m</fld> tail";
    let mut d = doc(vec![page(0, vec![text_shape(1, None, raw)])]);
    let got = find_matches(&d, "tail", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 1);
    // "Draft " (6) + "1.5m" (4) + " " (1) = 11 plain chars first.
    assert_eq!(got[0].target, FindTarget::Text { start: 11, len: 4 });
    // The field's cached display itself is searchable.
    let got = find_matches(&d, "1.5m", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target, FindTarget::Text { start: 6, len: 4 });
    // Phase 3's write path consumes the coordinates today: splice the
    // "tail" match and the marker survives.
    let m = find_matches(&d, "tail", FindScope::AllPages, &FindOptions::default())
        .pop()
        .expect("tail match");
    let FindTarget::Text { start, len } = m.target else {
        panic!("text match");
    };
    let mut cmd = ReplaceTextRange::new(Scope::Shape { page: 0, shape: 1 }, start, len, "end");
    cmd.apply(&mut d).expect("splice");
    let sh = vsdx::model::find_shape_by_id(&d.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("Draft <fld IX='0'>1.5m</fld> end")
    );
}

#[test]
fn master_inherited_text_and_rows_match_against_the_instance() {
    // The spell check's effective text: an instance with no local text
    // carries its master's. Cell rows inherit the same way (the Shape
    // Data dialog's effective view) and match against the INSTANCE —
    // the scope phase 3's SetRowCell edit targets.
    let master_shape = rich_shape(
        1,
        Some("M root"),
        Some("master hit text"),
        vec![value_row("Owner", "hit owner")],
        vec![value_row("Status", "hit status")],
    );
    let d = Document {
        pages: vec![page(0, vec![instance(9, 5), text_shape(2, None, "miss")])],
        masters: vec![Master {
            id: 5,
            name_u: Some("M".into()),
            shapes: vec![master_shape],
            ..Master::default()
        }],
        ..Document::default()
    };
    let got = find_matches(&d, "hit", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![
            (0, 9, FindTargetClass::Text, None),
            (0, 9, FindTargetClass::ShapeData, Some("Owner")),
            (0, 9, FindTargetClass::UserCell, Some("Status")),
        ],
        "the master's name_u never matches — NameU targets the shape itself"
    );
}

#[test]
fn local_value_row_shadows_same_named_master_row() {
    let master_shape = rich_shape(
        1,
        None,
        None,
        vec![value_row("Owner", "beta"), value_row("Note", "gamma")],
        vec![],
    );
    let mut inst = rich_shape(9, None, None, vec![value_row("Owner", "alpha")], vec![]);
    inst.master = Some(5);
    let d = Document {
        pages: vec![page(0, vec![inst])],
        masters: vec![Master {
            id: 5,
            name_u: Some("M".into()),
            shapes: vec![master_shape],
            ..Master::default()
        }],
        ..Document::default()
    };
    // The instance's row shadows the master's same-named row…
    assert!(find_matches(&d, "beta", FindScope::AllPages, &all_targets()).is_empty());
    let got = find_matches(&d, "alpha", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![(0, 9, FindTargetClass::ShapeData, Some("Owner"))]
    );
    // …while a master-only row stays visible.
    let got = find_matches(&d, "gamma", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![(0, 9, FindTargetClass::ShapeData, Some("Note"))]
    );
}

#[test]
fn inh_local_row_does_not_shadow_the_master_value() {
    // A local `F='Inh'` row carries no value of its own — the master's
    // value is what shows, so it is what matches.
    let master_shape = rich_shape(1, None, None, vec![value_row("Owner", "beta")], vec![]);
    let inh_row = Row {
        n: Some("Owner".into()),
        cells: vec![(
            "Value".into(),
            Cell {
                f: Some("Inh".into()),
                ..Cell::default()
            },
        )],
        ..Row::default()
    };
    let mut inst = rich_shape(9, None, None, vec![inh_row], vec![]);
    inst.master = Some(5);
    let d = Document {
        pages: vec![page(0, vec![inst])],
        masters: vec![Master {
            id: 5,
            name_u: Some("M".into()),
            shapes: vec![master_shape],
            ..Master::default()
        }],
        ..Document::default()
    };
    let got = find_matches(&d, "beta", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![(0, 9, FindTargetClass::ShapeData, Some("Owner"))]
    );
}

#[test]
fn deleted_rows_are_not_searched() {
    let d = doc(vec![page(
        0,
        vec![rich_shape(
            1,
            None,
            None,
            vec![deleted_value_row("Gone", "hit"), value_row("Here", "hit")],
            vec![],
        )],
    )]);
    let got = find_matches(&d, "hit", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![(0, 1, FindTargetClass::ShapeData, Some("Here"))]
    );
}

#[test]
fn local_tombstone_deletes_the_master_row_too() {
    // A local `Del='1'` row hides the master's same-named row (#293:
    // Visio honors the tombstone on open) — the inherited pass must
    // not resurrect it (CodeRabbit round on phase 1).
    let master_shape = rich_shape(
        1,
        None,
        None,
        vec![value_row("Owner", "beta"), value_row("Note", "gamma")],
        vec![],
    );
    let tombstone = Row {
        n: Some("Owner".into()),
        attrs: vec![("Del".into(), "1".into())],
        ..Row::default()
    };
    let mut inst = rich_shape(9, None, None, vec![tombstone], vec![]);
    inst.master = Some(5);
    let d = Document {
        pages: vec![page(0, vec![inst])],
        masters: vec![Master {
            id: 5,
            name_u: Some("M".into()),
            shapes: vec![master_shape],
            ..Master::default()
        }],
        ..Document::default()
    };
    assert!(find_matches(&d, "beta", FindScope::AllPages, &all_targets()).is_empty());
    // A master row the tombstone does not name still shows.
    let got = find_matches(&d, "gamma", FindScope::AllPages, &all_targets());
    assert_eq!(
        brief(&got),
        vec![(0, 9, FindTargetClass::ShapeData, Some("Note"))]
    );
}

// -- matcher shape -----------------------------------------------------

#[test]
fn empty_needle_matches_nothing() {
    let d = doc(vec![page(0, vec![text_shape(1, Some("hit"), "hit")])]);
    assert!(find_matches(&d, "", FindScope::AllPages, &all_targets()).is_empty());
}

#[test]
fn unicode_offsets_count_characters_not_bytes() {
    let d = doc(vec![page(
        0,
        vec![text_shape(1, None, "Größe und Maßstab")],
    )]);
    let got = find_matches(&d, "und", FindScope::AllPages, &FindOptions::default());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target, FindTarget::Text { start: 6, len: 3 });
    let plain: String = vsdx::text::text_pieces("Größe und Maßstab")
        .iter()
        .map(|p| p.plain())
        .collect();
    let sliced: String = plain.chars().skip(6).take(3).collect();
    assert_eq!(sliced, "und");
}

#[test]
fn matches_are_leftmost_and_non_overlapping() {
    let d = doc(vec![page(0, vec![text_shape(1, None, "aaaa")])]);
    let got = find_matches(&d, "aa", FindScope::AllPages, &FindOptions::default());
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![
            FindTarget::Text { start: 0, len: 2 },
            FindTarget::Text { start: 2, len: 2 }
        ]
    );
    // A needle longer than the text never matches.
    assert!(find_matches(&d, "aaaaaa", FindScope::AllPages, &FindOptions::default()).is_empty());
}

// -- caret codes and width folding (phase 2) --------------------------

#[test]
fn caret_codes_search_the_special_characters() {
    // a, TAB, b, LF, c, soft hyphen, d, ^, e — one of each Special
    // entry's literal.
    let raw = "a\tb\nc\u{AD}d^e";
    let d = doc(vec![page(0, vec![text_shape(1, None, raw)])]);
    let find = |needle: &str| {
        find_matches(&d, needle, FindScope::AllPages, &FindOptions::default())
            .into_iter()
            .map(|m| m.target)
            .collect::<Vec<_>>()
    };
    assert_eq!(find("^t"), vec![FindTarget::Text { start: 1, len: 1 }]);
    assert_eq!(find("^p"), vec![FindTarget::Text { start: 3, len: 1 }]);
    assert_eq!(find("^-"), vec![FindTarget::Text { start: 5, len: 1 }]);
    assert_eq!(find("^^"), vec![FindTarget::Text { start: 7, len: 1 }]);
    // Codes combine with literals.
    assert_eq!(find("b^pc"), vec![FindTarget::Text { start: 2, len: 3 }]);
}

#[test]
fn unrecognized_or_trailing_caret_stays_literal() {
    let d = doc(vec![page(0, vec![text_shape(1, None, "a^x b^")])]);
    let got = find_matches(&d, "^x", FindScope::AllPages, &FindOptions::default());
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![FindTarget::Text { start: 1, len: 2 }]
    );
    let got = find_matches(&d, "b^", FindScope::AllPages, &FindOptions::default());
    assert_eq!(
        got.iter().map(|m| m.target.clone()).collect::<Vec<_>>(),
        vec![FindTarget::Text { start: 4, len: 2 }]
    );
}

#[test]
fn any_character_wildcard_matches_exactly_one_char() {
    let d = doc(vec![page(0, vec![text_shape(1, None, "hot hat ht ho")])]);
    let got = find_matches(&d, "h^?t", FindScope::AllPages, &FindOptions::default());
    let spans: Vec<FindTarget> = got.iter().map(|m| m.target.clone()).collect();
    assert_eq!(
        spans,
        vec![
            FindTarget::Text { start: 0, len: 3 }, // hot
            FindTarget::Text { start: 4, len: 3 }, // hat
        ],
        "the two-letter `ht` never matches; each wildcard eats one char"
    );
}

#[test]
fn match_width_folds_fullwidth_forms() {
    // ＡＢＣ is fullwidth; 　(U+3000) is the ideographic space.
    let d = doc(vec![page(0, vec![text_shape(1, None, "ＡＢＣ　Ｄ")])]);
    let plain_opts = FindOptions::default();
    assert!(
        find_matches(&d, "abc", FindScope::AllPages, &plain_opts).is_empty(),
        "without the fold the widths differ"
    );
    let opts = FindOptions {
        match_width: true,
        ..FindOptions::default()
    };
    assert_eq!(find_matches(&d, "abc", FindScope::AllPages, &opts).len(), 1);
    assert_eq!(find_matches(&d, "c d", FindScope::AllPages, &opts).len(), 1);
    // The fold is symmetric: a fullwidth NEEDLE finds halfwidth text.
    let d = doc(vec![page(0, vec![text_shape(1, None, "width fold")])]);
    assert_eq!(
        find_matches(&d, "ＷＩＤＴＨ", FindScope::AllPages, &opts).len(),
        1
    );
}

// -- replace (phase 3) -------------------------------------------------

use dc_core::find::{decode_replacement, replace_all_command, replace_command};
use dc_core::Editor;

fn ed_with_shapes(shapes: Vec<Shape>) -> Editor {
    Editor::new(doc(vec![page(0, shapes)]))
}

#[test]
fn decode_replacement_expands_caret_codes() {
    assert_eq!(decode_replacement("a^tb^pc^-d^^e"), "a\tb\nc\u{AD}d^e");
    // `^?` is a search wildcard and stays literal; stray carets too.
    assert_eq!(decode_replacement("^?x^"), "^?x^");
}

#[test]
fn replace_command_splices_text_and_preserves_field_markers() {
    // Field safety (#115 acceptance): replacing on a field-bearing
    // shape goes through ReplaceTextRange — the `<fld>` marker
    // survives byte-identical, never SetText's flattening store.
    let raw = "Draft <fld IX='0'>1.5m</fld> tail";
    let mut ed = ed_with_shapes(vec![text_shape(1, None, raw)]);
    let m = find_matches(
        &ed.doc,
        "tail",
        FindScope::AllPages,
        &FindOptions::default(),
    )
    .pop()
    .unwrap();
    let cmd = replace_command(&ed.doc, &m, "end").expect("text command");
    ed.history.apply(cmd, &mut ed.doc).unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("Draft <fld IX='0'>1.5m</fld> end")
    );
    // Replacing INSIDE the field's cached display splices the display
    // with the markers intact.
    let m = find_matches(
        &ed.doc,
        "1.5m",
        FindScope::AllPages,
        &FindOptions::default(),
    )
    .pop()
    .unwrap();
    let cmd = replace_command(&ed.doc, &m, "2.0m").expect("text command");
    ed.history.apply(cmd, &mut ed.doc).unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.text_raw.as_deref(),
        Some("Draft <fld IX='0'>2.0m</fld> end")
    );
    // Undo restores the exact prior state each step.
    ed.history.undo(&mut ed.doc).unwrap();
    ed.history.undo(&mut ed.doc).unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(sh.sheet.text_raw.as_deref(), Some(raw));
}

#[test]
fn replace_command_writes_cell_values_through_set_row_cell() {
    let mut ed = ed_with_shapes(vec![rich_shape(
        1,
        None,
        None,
        vec![value_row("Owner", "Ada hit")],
        vec![value_row("Status", "hit ready")],
    )]);
    let opts = FindOptions {
        targets: FindTargets::all(),
        ..FindOptions::default()
    };
    let m = find_matches(&ed.doc, "hit", FindScope::AllPages, &opts)
        .first()
        .cloned()
        .unwrap();
    assert_eq!(m.target.class(), FindTargetClass::ShapeData);
    let cmd = replace_command(&ed.doc, &m, "miss").expect("cell command");
    ed.history.apply(cmd, &mut ed.doc).unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.sections_named("Prop")[0]
            .named_row("Owner")
            .unwrap()
            .cell("Value")
            .unwrap()
            .v,
        Some("Ada miss".to_string())
    );
    ed.history.undo(&mut ed.doc).unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.sections_named("Prop")[0]
            .named_row("Owner")
            .unwrap()
            .cell("Value")
            .unwrap()
            .v,
        Some("Ada hit".to_string())
    );
}

#[test]
fn replace_all_is_one_undo_across_shapes_and_spans() {
    let mut ed = ed_with_shapes(vec![
        text_shape(1, None, "hit one hit two"),
        text_shape(2, None, "no match"),
        text_shape(3, None, "hit three"),
    ]);
    let opts = FindOptions::default();
    let matches = find_matches(&ed.doc, "hit", FindScope::AllPages, &opts);
    assert_eq!(matches.len(), 3);
    let (mac, count) = replace_all_command(&ed.doc, &matches, "HIT");
    assert_eq!(count, 3);
    ed.history
        .apply(Box::new(mac.unwrap()), &mut ed.doc)
        .unwrap();
    let text = |ed: &Editor, id: i64| {
        vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, id)
            .unwrap()
            .sheet
            .text
            .clone()
            .unwrap()
    };
    assert_eq!(text(&ed, 1), "HIT one HIT two");
    assert_eq!(text(&ed, 2), "no match");
    assert_eq!(text(&ed, 3), "HIT three");
    // ONE undo restores the exact prior state of every shape.
    ed.history.undo(&mut ed.doc).unwrap();
    assert_eq!(text(&ed, 1), "hit one hit two");
    assert_eq!(text(&ed, 3), "hit three");
    assert!(ed.history.undo(&mut ed.doc).unwrap().is_none());
}

#[test]
fn replace_all_skips_name_matches_and_splices_cell_rows_once() {
    // A row with several matches becomes ONE full-value write; name
    // matches are not replaceable and are skipped.
    let mut ed = ed_with_shapes(vec![rich_shape(
        1,
        Some("hit name"),
        None,
        vec![value_row("Note", "hit and hit again")],
        vec![],
    )]);
    let opts = FindOptions {
        targets: FindTargets::all(),
        ..FindOptions::default()
    };
    let matches = find_matches(&ed.doc, "hit", FindScope::AllPages, &opts);
    assert_eq!(matches.len(), 3, "name + two cell spans: {matches:?}");
    let (mac, count) = replace_all_command(&ed.doc, &matches, "HIT");
    assert_eq!(count, 2, "only the cell spans replace");
    ed.history
        .apply(Box::new(mac.unwrap()), &mut ed.doc)
        .unwrap();
    let sh = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 1).unwrap();
    assert_eq!(
        sh.sheet.sections_named("Prop")[0]
            .named_row("Note")
            .unwrap()
            .cell("Value")
            .unwrap()
            .v,
        Some("HIT and HIT again".to_string())
    );
    assert_eq!(sh.name_u.as_deref(), Some("hit name"), "NameU untouched");
}
