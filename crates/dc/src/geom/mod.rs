//! The geometry placement frame, hit walk, and connector-crossing
//! math (#187): the pieces the EXPORT path needs on the dc side —
//! moved verbatim from dc-ui's `connectors.rs` (which re-exports them,
//! so every call site stayed put). The line-jump overlay the canvas
//! paints and the one `dc::svg` now appends share THIS code, so the
//! two can't drift.
//!
//! The pieces live along their internal boundaries:
//! `connector` (identity checks), `hitframe` (the placement frame),
//! `rows` (row-cell resolution + the curve seats), `segments` (the
//! hit walk), `jumps` (crossing math) — re-exported here so every
//! `dc::geom::…` path stays put.

mod connector;
mod hitframe;
mod jumps;
mod rows;
mod segments;

pub use connector::{is_connector, is_connector_in};
pub use hitframe::{hit_frame, HitFrame};
pub use jumps::{
    is_jump_candidate, is_stored_jump_arc, is_stored_jump_gap, is_stored_jump_polyline,
    jump_radii_in, jump_radius_in, jump_radius_y_in, line_jumps, line_jumps_with, page_jump_rules,
    page_jump_style, page_jumps, page_jumps_stripping, parse_polyline_controls, seg_cross, Jump,
    JumpLine, JumpRules, JumpStyle,
};
pub use rows::{arc_row, cubic_row, row_cell_value, ArcRow, CubicRow};
pub use segments::{geometry_hit_segments, geometry_hit_segments_straight, MAX_WALK_ROWS};

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
            style: JumpStyle::Arc,
            pts: pts.to_vec(),
        }
    }

    /// The stylesheet default `BegTrigger V='0' F='No Formula'` is no
    /// glue trigger: a plain 1D line that resolves it stays a line,
    /// one with the glue engine's `_XFTRIGGER` write is a connector.
    #[test]
    fn style_default_triggers_do_not_make_a_connector() {
        let mut no_style = vsdx::model::StyleSheet::default();
        no_style.sheet.set_cell(
            "BegTrigger",
            vsdx::Cell {
                v: Some("0".into()),
                f: Some("No Formula".into()),
                ..vsdx::Cell::default()
            },
        );
        let doc = vsdx::Document {
            stylesheets: vec![no_style],
            ..vsdx::Document::default()
        };
        let mut line = vsdx::Shape {
            id: 1,
            ..vsdx::Shape::default()
        };
        for (n, v) in [
            ("BeginX", "0"),
            ("BeginY", "0"),
            ("EndX", "1"),
            ("EndY", "0"),
        ] {
            line.sheet.set_cell(n, vsdx::Cell::value(v));
        }
        assert!(!super::is_connector_in(&doc, &[&line], &line));
        line.sheet.set_cell(
            "BegTrigger",
            vsdx::Cell {
                v: Some("2".into()),
                f: Some("_XFTRIGGER(Sheet.9!EventXFMod)".into()),
                ..vsdx::Cell::default()
            },
        );
        assert!(super::is_connector_in(&doc, &[&line], &line));
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
            style: JumpStyle::Arc,
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
            &[l(1, 0, &arc), l(2, 1, &[(0.8, 2.0), (1.2, 0.0)])],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "the converging pair merges: {js:?}");
        // Two crossings far apart stay two hops.
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (1.0, 1.0), (1.0, 3.0), (4.0, 3.0)]),
                l(2, 1, &[(2.0, 0.0), (2.0, 4.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "one real crossing");
        let js = line_jumps(
            &[
                l(1, 0, &[(0.0, 1.0), (3.0, 1.0)]),
                l(2, 1, &[(1.0, 0.0), (1.0, 2.0), (2.5, 2.0), (2.5, 0.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
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
            0.041_666_7,
            0.041_666_7,
        ));
        // A corner arc cuts across the run — never a jump.
        assert!(!is_stored_jump_arc(
            0.09,
            (0.5, 0.5),
            Some((1.0, 0.0)),
            Some((0.0, 1.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        // An authored shallow bump (the #294 review's control) is a
        // curve: |A| ≪ the semicircle band.
        assert!(!is_stored_jump_arc(
            0.25,
            (4.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.25,
            0.25,
        ));
        // A large SHALLOW arc stays a curve — size alone never makes
        // a jump (the semicircle band does the work; the old 0.5 in
        // cap wrongly rejected documented factor-10 jumps, #294
        // review).
        assert!(!is_stored_jump_arc(
            1.0,
            (0.0, 4.0),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            1.0,
            1.0,
        ));
        // A large SEMICIRCLE with full route context is a jump at any
        // size (LineJumpFactorX=10 → 0.625 in at the defaults).
        assert!(is_stored_jump_arc(
            2.0,
            (0.0, 4.0),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            2.0,
            2.0,
        ));
        // Degenerate: zero chord and a zero-sagitta straight row
        // (which the ink path already emits as a line).
        assert!(!is_stored_jump_arc(
            0.0,
            (0.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        assert!(!is_stored_jump_arc(
            0.0,
            (0.0, 1.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.041_666_7,
        ));
        // An authored whole-route semicircle (#294 review) has no
        // continuing runs — it stays a curve.
        assert!(!is_stored_jump_arc(
            0.25,
            (0.5, 0.0),
            None,
            None,
            0.25,
            0.25
        ));
        // Mid-run geometry but a run that stops at the arc (an
        // authored end-arc) is not a jump either.
        assert!(!is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            None,
            0.041_666_7,
            0.041_666_7,
        ));
        // Radius evidence (CodeRabbit rounds 1–2 on #121): the shape
        // checks alone cannot tell an AUTHORED semicircle between two
        // straight runs from a stored jump — a 1-inch arc with A=−0.5
        // matches every geometric test. The page radius decides.
        assert!(!is_stored_jump_arc(
            -0.5,
            (1.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7, // the page's radius is far smaller: authored ink
            0.5,
        ));
        assert!(is_stored_jump_arc(
            -0.5,
            (1.0, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.5, // the page's radius matches: a stored jump
            0.5,
        ));
        // Writer rounding rides inside the 5% band.
        assert!(is_stored_jump_arc(
            -0.041_666_67,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_691, // 0.125 × 0.66667 / 2, the No Style values
            0.041_666_691,
        ));
        // No radius evidence (0) never strips — preserve the arc.
        assert!(!is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.0,
            0.0,
        ));
        // Round 3 (PR #299 review): only the CHOSEN axis's radius must
        // be positive — a FactorY=0 page still recognizes horizontal
        // stored jumps, and a zero on the arc's own axis never strips.
        assert!(is_stored_jump_arc(
            -0.041_666_7,
            (0.083_333_3, 0.0),
            Some((1.0, 0.0)),
            Some((1.0, 0.0)),
            0.041_666_7,
            0.0,
        ));
        assert!(!is_stored_jump_arc(
            -0.041_666_7,
            (0.0, 0.083_333_3),
            Some((0.0, 1.0)),
            Some((0.0, 1.0)),
            0.041_666_7,
            0.0,
        ));
    }

    /// Round 3 (PR #299 review): the cluster reach is the crossing's
    /// OWN axis radius — the unrelated axis's radius must not merge
    /// two crossings the drawn arcs never overlap. ONE pair must
    /// produce BOTH crossings (clustering merges within a pair only),
    /// and the weights stay tiny so the stroke-width reach cannot
    /// mask the radius difference (round-4 review).
    #[test]
    fn line_jumps_clusters_on_the_crossing_axis_radius() {
        let lines = [
            // The jumper: one horizontal run.
            JumpLine {
                id: 1,
                z: 1,
                weight: 0.01,
                color: (0, 0, 0),
                style: JumpStyle::Arc,
                pts: vec![(0.0, 1.0), (2.0, 1.0)],
            },
            // The crossed line: a U crossing the run TWICE — at x=1
            // and x=1.5. One pair, two crossings 0.5 in apart.
            JumpLine {
                id: 2,
                z: 2,
                weight: 0.01,
                color: (0, 0, 0),
                style: JumpStyle::Arc,
                pts: vec![(1.0, 0.0), (1.0, 2.0), (1.5, 2.0), (1.5, 0.0)],
            },
        ];
        // X radius tiny, Y radius huge: the horizontal hops draw at
        // the X radius, so 0.5 in apart they stay TWO hops — the old
        // max-axis reach (1.0 in) merged them into one hop at their
        // midpoint, x=1.25, where no crossing exists.
        let js = line_jumps(&lines, (0.041_666_7, 0.5));
        assert_eq!(js.len(), 2, "no unrelated-axis clustering: {js:?}");
        assert!(
            js.iter().all(|j| j.dir.1.abs() < 1e-9),
            "the horizontal line hops: {js:?}"
        );
        // And the hops sit ON the crossings, not at a centroid.
        assert!(
            js.iter().any(|j| (j.at.0 - 1.0).abs() < 1e-9)
                && js.iter().any(|j| (j.at.0 - 1.5).abs() < 1e-9),
            "the hops stay at their crossings: {js:?}"
        );
    }

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
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
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
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a glue end never hops: {js:?}");
        // A V that only TOUCHES the horizontal (both runs on the same
        // side) is a tangency, not a crossing — no hop (#294 review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 0.0), (3.0, 0.0)]),
                jline(2, 1, &[(0.0, 1.0), (1.0, 0.0), (2.0, 1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a tangency never hops: {js:?}");
        // BOTH routes turning at the shared contact: the four runs
        // must alternate by route around the vertex. An L meeting a V
        // groups two L-runs and two V-runs — a touch, no hop (#294
        // review).
        let js = line_jumps(
            &[
                jline(1, 0, &[(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)]),
                jline(2, 1, &[(-1.0, 1.0), (0.0, 0.0), (-1.0, -1.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 0, "a two-corner touch never hops: {js:?}");
        // Two routes cornering THROUGH each other interleave — a real
        // crossing hops.
        let js = line_jumps(
            &[
                jline(1, 0, &[(-1.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
                jline(2, 1, &[(1.0, 1.0), (0.0, 0.0), (-1.0, 0.0)]),
            ],
            (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS),
        );
        assert_eq!(js.len(), 1, "interleaved corner runs cross: {js:?}");
    }

    /// #334: a connector MERGING onto another's run — a ray the two
    /// routes share — is not a crossing, in both contact branches: the
    /// both-turn `alternating` sort (where the coincident pair's tie
    /// read as alternation by sort luck — the join heading east
    /// hopped, its mirror heading west did not) and the one-route-turn
    /// touch test (where the run along the other segment sidestepped
    /// the same-side sign test). The offset gate keeps the PIERCE that
    /// runs 5e-10 in beside the line a crossing; a line genuinely
    /// leaving keeps its touch/cross verdicts.
    #[test]
    fn shared_runs_merge_without_a_hop() {
        let r = (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS);
        let both_turn = |b: &[(f64, f64)]| {
            line_jumps(
                &[
                    jline(1, 0, &[(-1.0, 0.0), (0.0, 0.0), (1.0, 0.0)]),
                    jline(2, 1, b),
                ],
                r,
            )
        };
        // B reaches A's interior vertex and leaves along A's own run —
        // east (the reported false hop), west (the mirror that passed
        // by tie-order luck), and floating-point skew on both sides of
        // the shared line.
        assert_eq!(
            both_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, 0.0)]).len(),
            0,
            "a join heading east merges, not crosses"
        );
        assert_eq!(
            both_turn(&[(0.0, 1.0), (0.0, 0.0), (-1.0, 0.0)]).len(),
            0,
            "a join heading west merges too — no east/west asymmetry"
        );
        assert_eq!(
            both_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, 1e-13)]).len(),
            0,
            "skew above the shared line still merges"
        );
        assert_eq!(
            both_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, -1e-13)]).len(),
            0,
            "skew below the shared line still merges"
        );
        // The same merge rotated onto a vertical run.
        assert_eq!(
            line_jumps(
                &[
                    jline(1, 0, &[(0.0, -1.0), (0.0, 0.0), (0.0, 1.0)]),
                    jline(2, 1, &[(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)]),
                ],
                r
            )
            .len(),
            0,
            "a join heading north on a vertical run merges"
        );
        // One-turn merges: the other route runs straight through (a
        // single segment), B merges at an interior vertex of its own.
        let one_turn = |b: &[(f64, f64)]| {
            line_jumps(
                &[jline(1, 0, &[(-1.0, 0.0), (1.0, 0.0)]), jline(2, 1, b)],
                r,
            )
        };
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, 0.0)]).len(),
            0,
            "a one-turn join heading east merges"
        );
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (-1.0, 0.0)]).len(),
            0,
            "a one-turn join heading west merges"
        );
        // Near-misses stay crossings: the route PIERCES the line and
        // runs 5e-10 in beside it — beyond the 1e-11 shared-run offset.
        assert_eq!(
            both_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, -5e-10)]).len(),
            1,
            "a both-turn pierce beside the line still hops"
        );
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, -5e-10)]).len(),
            1,
            "a one-turn pierce beside the line still hops"
        );
        // The offset is measured against the other route's OWN line,
        // never a ray fabricated from the contact vertex: a vertex
        // 5e-10 ABOVE the line whose departure merely AIMS along it
        // still crosses (CodeRabbit, on the re-land) — in both
        // contact branches.
        let aim_along = &[(0.0, -1.0), (0.0, 5e-10), (0.5, 2.5e-10)];
        assert_eq!(
            one_turn(aim_along).len(),
            1,
            "a vertex off the line aiming along it still crosses"
        );
        assert_eq!(
            both_turn(aim_along).len(),
            1,
            "the both-turn aim-along contact still crosses"
        );
        // The shared-run rejection is RECIPROCAL: both runs must
        // agree they lie on one line. A short run otherwise hides a
        // long one's divergence — A's 0.001-in run stays within
        // 5e-13 of B's piercing line while B's own endpoint is 5e-10
        // out, and that pair still crosses (CodeRabbit round 2).
        assert_eq!(
            line_jumps(
                &[
                    jline(1, 0, &[(-0.001, 0.0), (0.0, 0.0), (0.001, 0.0)]),
                    jline(2, 1, &[(0.0, 1.0), (0.0, 0.0), (1.0, -5e-10)]),
                ],
                r
            )
            .len(),
            1,
            "a short run does not hide the pierce"
        );
        // …and the reciprocal check holds in the ONE-TURN branch too:
        // a SHORT departure (0.001 in, ending 5e-13 below) must not
        // hide the other side's divergence — the same geometry hops
        // whether the other route carries a redundant vertex or not
        // (CodeRabbit round 3).
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (0.001, -5e-13)]).len(),
            1,
            "a short departure does not hide the pierce either"
        );
        // Controls: a line genuinely leaving keeps its verdicts —
        // upward is a touch, downward a crossing.
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, 0.01)]).len(),
            0,
            "leaving upward: a touch"
        );
        assert_eq!(
            one_turn(&[(0.0, 1.0), (0.0, 0.0), (1.0, -0.01)]).len(),
            1,
            "leaving downward: a crossing"
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

    /// A PolylineTo row's POLYLINE() vertices are part of the hit
    /// path, so crossings see the drawn bends (Greptile on PR #329).
    #[test]
    fn polyline_to_vertices_join_the_hit_path() {
        let mut poly = mrow(2, "PolylineTo", &[("X", 2.0), ("Y", 0.0)]);
        poly.cells.push((
            "A".into(),
            Cell {
                v: Some("POLYLINE(1, 1, 0,1, 2,1)".into()),
                ..Cell::default()
            },
        ));
        let s = conn_shape(
            7,
            2.0,
            1.0,
            vec![mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), poly],
        );
        let doc = vsdx::Document::default();
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let segs = geometry_hit_segments(&s, None, &frame, None);
        assert_eq!(segs.len(), 3, "M → (0,1) → (2,1) → (2,0): {segs:?}");
        let ys: Vec<f64> = segs.iter().map(|(a, _)| a.1).collect();
        assert!(
            (ys[1] - ys[0] - 1.0).abs() < 1e-9 && (ys[2] - ys[1]).abs() < 1e-9,
            "the walk climbs to the polyline's top run: {segs:?}"
        );
    }

    /// #324 step 1 (the live-measured rules, mk-git 2026-11-25): the
    /// walk's straight-run eligibility. Line rows, an L route's
    /// bends, and a recognized stored-hop arc all keep the connector
    /// eligible — the arc straightens, exactly as the crossing set
    /// walks it.
    #[test]
    fn straight_walk_lines_bends_and_stored_hops_stay_eligible() {
        let doc = vsdx::Document::default();
        // The straight route with a mid-run stored hop.
        let s = conn_shape(7, 3.0, 0.25, jump_rows());
        let chain = vec![&s];
        let frame = hit_frame(&doc, &chain, &s, (1.0, 1.0));
        let (segs, straight) =
            geometry_hit_segments_straight(&s, None, &frame, Some((0.041_666_7, 0.041_666_7)));
        assert!(straight, "a stored hop is not a curve: it straightens");
        assert!(
            !segs.is_empty() && flat(&segs),
            "the route stays one run: {segs:?}"
        );
        // An L route: both legs are straight runs.
        let bent = conn_shape(
            8,
            1.0,
            1.0,
            vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                mrow(2, "LineTo", &[("X", 1.0), ("Y", 0.0)]),
                mrow(3, "LineTo", &[("X", 1.0), ("Y", 1.0)]),
            ],
        );
        let chain = vec![&bent];
        let frame = hit_frame(&doc, &chain, &bent, (1.0, 1.0));
        let (_, straight) = geometry_hit_segments_straight(&bent, None, &frame, None);
        assert!(straight, "an L route is entirely straight runs");
    }

    /// #324 step 1: curve rows drop the connector from the crossing
    /// set's eligibility whole — an authored arc, a polyline row, a
    /// spline — while the same rows still walk (the hit path is
    /// unchanged).
    #[test]
    fn curve_rows_leave_the_straight_run_eligibility() {
        let doc = vsdx::Document::default();
        // An authored arc: sagitta far off the page radius and no
        // continuing run after it, so it classifies as a curve.
        let arc = conn_shape(
            7,
            1.0,
            1.0,
            vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                mrow(2, "LineTo", &[("X", 0.0), ("Y", 0.5)]),
                mrow(3, "ArcTo", &[("X", 0.0), ("Y", 1.0), ("A", -0.5)]),
            ],
        );
        let chain = vec![&arc];
        let frame = hit_frame(&doc, &chain, &arc, (1.0, 1.0));
        let (segs, straight) =
            geometry_hit_segments_straight(&arc, None, &frame, Some((0.041_666_7, 0.041_666_7)));
        assert!(!straight, "an authored arc is curve geometry");
        assert!(
            segs.iter()
                .any(|(a, b)| (a.0 - b.0).abs() > 1e-9 || (a.1 - b.1).abs() > 1e-9),
            "the walk still draws the arc: {segs:?}"
        );
        // A polyline row.
        let mut poly = mrow(2, "PolylineTo", &[("X", 2.0), ("Y", 0.0)]);
        poly.cells.push((
            "A".into(),
            Cell {
                v: Some("POLYLINE(1, 1, 0,1, 2,1)".into()),
                ..Cell::default()
            },
        ));
        let poly_s = conn_shape(
            8,
            2.0,
            1.0,
            vec![mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]), poly],
        );
        let chain = vec![&poly_s];
        let frame = hit_frame(&doc, &chain, &poly_s, (1.0, 1.0));
        let (_, straight) = geometry_hit_segments_straight(&poly_s, None, &frame, None);
        assert!(
            !straight,
            "a POLYLINE row is authored geometry, not a routed run"
        );
        // Spline rows.
        let spline = conn_shape(
            9,
            1.0,
            1.0,
            vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                mrow(
                    2,
                    "SplineStart",
                    &[
                        ("X", 0.3),
                        ("Y", 0.5),
                        ("A", 0.2),
                        ("B", 0.1),
                        ("C", 1.0),
                        ("D", 3.0),
                    ],
                ),
                mrow(3, "SplineKnot", &[("X", 0.7), ("Y", 0.5), ("A", 0.5)]),
                mrow(4, "SplineKnot", &[("X", 1.0), ("Y", 0.0), ("A", 1.0)]),
            ],
        );
        let chain = vec![&spline];
        let frame = hit_frame(&doc, &chain, &spline, (1.0, 1.0));
        let (_, straight) = geometry_hit_segments_straight(&spline, None, &frame, None);
        assert!(!straight, "spline rows are curve geometry");
    }

    #[test]
    fn ellipse_excludes_connector_without_adding_hit_segments() {
        let shape = conn_shape(
            7,
            2.0,
            1.0,
            vec![
                mrow(1, "MoveTo", &[("X", 0.0), ("Y", 0.0)]),
                mrow(2, "Ellipse", &[("X", 1.0), ("Y", 1.0)]),
                mrow(3, "LineTo", &[("X", 2.0), ("Y", 0.0)]),
            ],
        );
        let doc = vsdx::Document::default();
        let frame = hit_frame(&doc, &[&shape], &shape, (1.0, 1.0));
        let (segments, straight) = geometry_hit_segments_straight(&shape, None, &frame, None);
        assert!(!straight, "an ellipse is curve geometry");
        assert_eq!(
            segments,
            [(frame.to_page((0.0, 0.0)), frame.to_page((2.0, 0.0)))]
        );
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
    use vsdx::model::{Cell, Row, Section, Shape, SheetChild};

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

    /// A straight two-row `ObjType=2` connector from `from` to `to`
    /// (page inches, y-up), with an optional `LineColor` cell value.
    fn straight_connector(id: i64, from: (f64, f64), to: (f64, f64), color: Option<&str>) -> Shape {
        let (w, h) = (to.0 - from.0, to.1 - from.1);
        let mut s = Shape {
            id,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", (from.0 + to.0) / 2.0),
            ("PinY", (from.1 + to.1) / 2.0),
            ("Width", w),
            ("Height", h),
            ("LocPinX", w / 2.0),
            ("LocPinY", h / 2.0),
            ("ObjType", 2.0),
        ] {
            s.sheet.set_cell(n, Cell::value(format!("{v}")));
        }
        if let Some(c) = color {
            s.sheet.set_cell("LineColor", Cell::value(c));
        }
        let row = |ix: i64, t: &str, x: f64, y: f64| Row {
            t: Some(t.into()),
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
        sec.rows = vec![row(1, "MoveTo", 0.0, 0.0), row(2, "LineTo", w, h)];
        s.sheet.children.push(SheetChild::Section(sec));
        s
    }

    /// #324 phase 2 (live-measured in Visio 2021, mk-git 2026-11-25):
    /// page `LineJumpCode` 1 hops the flatter segment — steepness,
    /// not class, so the more horizontal of two STEEP lines hops too
    /// (29° over 61°, 75° over 85°, measured live) — and equally
    /// steep segments never hop; 2 mirrors it; 3 hops the later
    /// line, 5 the earlier. `ConLineJumpCode` 1 never, 2 always, 3
    /// the other line, 4 neither.
    #[test]
    fn jump_rules_follow_visio() {
        let h = jline(1, 0, &[(0.0, 1.0), (2.0, 1.0)]);
        let v = jline(2, 1, &[(1.0, 0.0), (1.0, 2.0)]);
        // Two STEEP lines that cross (60° through the origin, an 84°
        // one beside it) — both vertical-class under a |dx| ≥ |dy|
        // reading, so this pair pins steepness, not class.
        let s60 = jline(3, 2, &[(-1.0, -1.7320508), (1.0, 1.7320508)]);
        let s84 = jline(4, 3, &[(0.3, -1.0), (0.6, 1.8542526)]);
        // The mirrored X: equal steepness, opposite signs.
        let x1 = jline(5, 4, &[(0.0, 0.0), (2.0, 1.6)]);
        let x2 = jline(6, 5, &[(0.0, 1.6), (2.0, 0.0)]);
        let r = (TEST_JUMP_RADIUS, TEST_JUMP_RADIUS);
        let rules = |page: i64, codes: &[(i64, i64)]| JumpRules {
            page_code: page,
            con_codes: codes.iter().copied().collect(),
        };
        let hops = |ls: &[JumpLine], ru: &JumpRules| line_jumps_with(ls, r, ru);
        // Page 1: the horizontal hops over the vertical…
        let js = hops(&[h.clone(), v.clone()], &rules(1, &[]));
        assert!(js.len() == 1 && js[0].dir.1.abs() < 1e-9, "{js:?}");
        // …the FLATTER (60°) of two steep lines hops — the live
        // witness, where a class reading would draw no hop…
        let js = hops(&[s60.clone(), s84.clone()], &rules(1, &[]));
        assert!(
            js.len() == 1 && js[0].jumper == s60.id,
            "the 60° link hops over the 84° one: {js:?}"
        );
        // …and equally steep segments (the mirrored X) never hop.
        assert!(hops(&[x1.clone(), x2.clone()], &rules(1, &[])).is_empty());
        // Page 2: the vertical hops; the steeper (84°) hops; the X
        // still never hops.
        let js = hops(&[h.clone(), v.clone()], &rules(2, &[]));
        assert!(js.len() == 1 && js[0].dir.0.abs() < 1e-9, "{js:?}");
        let js = hops(&[s60.clone(), s84.clone()], &rules(2, &[]));
        assert!(
            js.len() == 1 && js[0].jumper == s84.id,
            "the 84° link hops over the 60° one: {js:?}"
        );
        assert!(hops(&[x1.clone(), x2.clone()], &rules(2, &[])).is_empty());
        // Page 3 hops the X too — the LATER line; page 5 the earlier.
        let js = hops(&[x1.clone(), x2.clone()], &rules(3, &[]));
        assert_eq!((js.len(), js[0].jumper), (1, x2.id), "{js:?}");
        let js = hops(&[x1.clone(), x2.clone()], &rules(5, &[]));
        assert_eq!((js.len(), js[0].jumper), (1, x1.id), "{js:?}");
        // Connector codes on the horizontal line under page 1.
        let on_h = |code: i64| hops(&[h.clone(), v.clone()], &rules(1, &[(1, code)]));
        assert_eq!(on_h(1).len(), 0, "never");
        assert!(on_h(2)[0].dir.1.abs() < 1e-9, "always: the horizontal hops");
        assert!(
            on_h(3)[0].dir.0.abs() < 1e-9,
            "other line: the vertical hops"
        );
        assert_eq!(on_h(4).len(), 0, "neither");
    }

    /// #336: the shared builder resolves jump colors like the
    /// renderer — a legacy palette-index `LineColor` (value `2` = red,
    /// the [MS-VSDX] color table; the #218 review's common case) paints
    /// its hop red, not the hex-fallback gray — and a hidden-layer
    /// member leaves the crossing set entirely. The canvas overlay and
    /// the stored-jump reconciliation consume this same set, so this is
    /// their contract too.
    #[test]
    fn page_jumps_resolves_palette_colors_and_skips_hidden_layers() {
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        // The horizontal jumper carries the palette-index LineColor.
        let a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), Some("2"));
        let mut b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(a);
        page.shapes.push(b.clone());
        let doc = vsdx::Document::default();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1, "the crossing hops once: {js:?}");
        assert_eq!(js[0].jumper, 1, "the flatter line hops: {js:?}");
        assert_eq!(
            js[0].color,
            (255, 0, 0),
            "palette index 2 is red, not the #424242 fallback: {js:?}"
        );
        // A hidden-layer member leaves the crossing set: the visible
        // connector no longer hops over it.
        b.sheet.set_cell("LayerMember", Cell::value("2"));
        page.shapes[1] = b;
        let js = page_jumps(&doc, &page, &[2]);
        assert!(js.is_empty(), "a hidden under-line crosses nothing: {js:?}");
        // Re-showing the layer restores the hop.
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1, "visible again: {js:?}");
    }

    /// #333: a malformed non-ASCII `LineColor` (`#aé000` — six BYTES,
    /// a multi-byte character inside the fixed byte slices) degrades
    /// to the gray fallback instead of panicking the crossing walk —
    /// every consumer of the shared set (export overlay, canvas,
    /// `ReconcileLineJumps` on every edit) runs through here. The
    /// healthy side of the hop keeps its own color.
    #[test]
    fn page_jumps_degrades_malformed_line_color() {
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            ..vsdx::Page::default()
        };
        let a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), Some("#00ff00"));
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), Some("#aé000"));
        page.shapes.push(a);
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1, "the crossing still hops: {js:?}");
        assert_eq!(js[0].jumper, 1, "the flatter line hops: {js:?}");
        assert_eq!(
            js[0].color,
            (0, 255, 0),
            "the healthy jumper keeps its green: {js:?}"
        );
        assert_eq!(
            js[0].under_color,
            (0x42, 0x42, 0x42),
            "the malformed under line takes the gray fallback: {js:?}"
        );
    }

    /// `page_jump_style` reads the page's `LineJumpStyle` through the
    /// style chain, and a connector's nonzero `ConLineJumpStyle`
    /// overrides it per jumper (#324 phase 4).
    #[test]
    fn page_jump_style_resolves_chain_and_connector() {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.set_cell("LineJumpStyle", Cell::value("3"));
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            children: vec![vsdx::model::IndexChild::Sheet {
                tag: "PageSheet".into(),
                sheet,
            }],
            ..vsdx::Page::default()
        };
        let mut a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(a.clone());
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        // The page style (3 = Square) flows to the hop…
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js.len(), 1);
        assert_eq!(js[0].style, JumpStyle::Square);
        // …and the connector's own nonzero code overrides it
        // (4 = 2 sides).
        a.sheet.set_cell("ConLineJumpStyle", Cell::value("4"));
        page.shapes[0] = a.clone();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js[0].style, JumpStyle::Sides(2));
        // Zero means "as the page specifies" — back to Square.
        a.sheet.set_cell("ConLineJumpStyle", Cell::value("0"));
        page.shapes[0] = a.clone();
        let js = page_jumps(&doc, &page, &[]);
        assert_eq!(js[0].style, JumpStyle::Square);
    }

    /// `page_jump_rules` reads what Visio reads — the PageSheet's
    /// `LineJumpCode` through the style chain, the connectors'
    /// resolved `ConLineJumpCode`s — and jump candidacy is
    /// `ObjType = 2`: a plain glued 1D line (static `PAR(PNT` glue,
    /// no ObjType) never joins the crossing set (#322's witness),
    /// while the master chain still supplies the marker for real
    /// connectors (#174, covered end to end by the b01/c02 fixtures).
    #[test]
    fn page_jump_rules_read_visio_and_gate_on_objtype() {
        let mut sheet = vsdx::model::Sheet::default();
        sheet.set_cell("LineJumpCode", Cell::value("2"));
        let mut page = vsdx::Page {
            id: 0,
            name: "T".into(),
            children: vec![vsdx::model::IndexChild::Sheet {
                tag: "PageSheet".into(),
                sheet,
            }],
            ..vsdx::Page::default()
        };
        let mut a = straight_connector(1, (1.0, 1.0), (5.0, 1.0), None);
        let b = straight_connector(2, (3.0, 0.0), (3.0, 2.0), None);
        page.shapes.push(a.clone());
        page.shapes.push(b);
        let doc = vsdx::Document::default();
        let rules = page_jump_rules(&doc, &page);
        assert_eq!(rules.page_code, 2, "the PageSheet cell is read");
        assert!(rules.con_codes.is_empty(), "absent codes stay absent");
        // Page 2 hops the vertical.
        let js = page_jumps(&doc, &page, &[]);
        assert!(js.len() == 1 && js[0].jumper == 2, "{js:?}");
        // The horizontal's own ConLineJumpCode overrides the page
        // rule: 2 forces it to hop, 3 hands the hop to the vertical,
        // 1 is its OWN veto only (the page-2 jumper B still hops over
        // it — c02's witness is the veto on the WOULD-BE jumper), and
        // 4 kills the hop at its crossings outright.
        for (code, expect) in [(2, vec![1]), (3, vec![2]), (1, vec![2])] {
            a.sheet
                .set_cell("ConLineJumpCode", Cell::value(format!("{code}")));
            page.shapes[0] = a.clone();
            let js = page_jumps(&doc, &page, &[]);
            let mut got: Vec<i64> = js.iter().map(|j| j.jumper).collect();
            got.sort_unstable();
            assert_eq!(got, expect, "code {code}: {js:?}");
            assert_eq!(
                page_jump_rules(&doc, &page).con_codes.get(&1),
                Some(&code),
                "the connector code resolves through the chain"
            );
        }
        a.sheet.set_cell("ConLineJumpCode", Cell::value("4"));
        page.shapes[0] = a.clone();
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "code 4: no hop at its crossings"
        );
        // A plain glued 1D line — static PAR(PNT glue, ObjType absent
        // — is NOT a jump candidate: nothing hops over it and it hops
        // over nothing, where the old glue-ownership gate drew one.
        let mut g = straight_connector(7, (1.0, 0.5), (5.0, 0.5), None);
        g.sheet
            .children
            .retain(|c| !matches!(c, vsdx::model::SheetChild::Cell(n, _) if n == "ObjType"));
        let mut glue = Cell::value("1.0");
        glue.f = Some("PAR(PNT(Sheet.99!Connections.X1,Sheet.99!Connections.Y1))".into());
        g.sheet.set_cell("BeginX", glue);
        page.shapes[0] = g;
        assert!(
            page_jumps(&doc, &page, &[]).is_empty(),
            "an ObjType-0 glued line never joins the crossing set"
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
