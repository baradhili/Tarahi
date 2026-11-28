//! kittest coverage for AutoRecover (GitHub #213): a dirty PATHED
//! document autosaves its recovery snapshot within the interval (the
//! `DC_UI_AUTORECOVER_SECS` knob; untitled documents never autosave),
//! a clean save removes the copy, and the startup Document Recovery
//! pane restores (through the normal open path — the document lands
//! as the original, dirty, and the recovery files STAY until that
//! decision saves or drops it) or discards a planted snapshot.

use std::sync::Mutex;
use std::time::Duration;

use dc_ui::app::App;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

/// The env knob is process-global: the tests in this binary
/// serialize their env writes on this lock (held per test).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn harness_with(app: App) -> Harness<'static, App> {
    dc_ui::svgimg::pin_determinism_for_tests();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            app,
        )
}

/// Plant a recovery snapshot + sidecar pair for `original` through
/// the REAL write path (production naming: the identity hash rides
/// the file name); `original` stays GONE, forcing the scan's
/// "originals gone" offer rule. Returns the pair's paths.
fn plant(dir: &std::path::Path, original: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let doc = dc_core::Editor::new_blank().doc;
    assert!(dc_ui::autorecover::write_snapshot_for_test(
        doc,
        original.to_path_buf(),
        dir.to_path_buf()
    ));
    dc_ui::autorecover::recovery_paths_for_test(dir, original)
}

/// Step frames (with real sleeps — the autosave deadline and the
/// async-open scrim run on wall-clock time) until `probe` turns true.
fn wait_until(h: &mut Harness<'_, App>, probe: impl Fn(&App) -> bool) {
    for _ in 0..120 {
        h.run();
        if probe(h.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "the awaited condition never arrived (status: {:?})",
        h.state().debug_status()
    );
}

#[test]
fn dirty_doc_autosaves_within_the_interval_and_a_clean_save_clears_it() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("DC_UI_AUTORECOVER_SECS", "1");
    let dir = tempfile::tempdir().unwrap();
    let doc_path = dir.path().join("report.vsdx");
    dc_core::Editor::new_blank().save_as(&doc_path).unwrap();
    let rec_dir = dir.path().join("rec");
    let (snap, side) = dc_ui::autorecover::recovery_paths_for_test(&rec_dir, &doc_path);
    // A persisting store with the recovery location aimed at the
    // tempdir (an ephemeral store keeps the tick inert — kittest must
    // never write into the user's real config).
    let mut app = App::with_doc(&doc_path);
    let mut opts = dc_ui::options::Options::persisting_for_test(dir.path().join("options.json"));
    opts.autorecover_dir = rec_dir.display().to_string();
    app.set_options_store_for_test(opts);
    let mut h = harness_with(app);
    h.run();
    h.state_mut().simulate_edit_for_test();
    wait_until(&mut h, |_| snap.is_file() && side.is_file());
    // The snapshot is a real package, and the original is untouched
    // (autosave never corrupts it — the writes go to the copy only).
    assert!(vsdx::parse::parse(&snap).is_ok(), "the snapshot parses");
    assert!(
        vsdx::parse::parse(&doc_path).is_ok(),
        "the original still parses — autosave never touched it"
    );

    // A clean save removes the recovery copy (saving cleanly clears).
    h.state_mut().debug_save_for_test();
    h.run();
    assert!(!snap.exists() && !side.exists(), "the clean save cleared it");
    std::env::remove_var("DC_UI_AUTORECOVER_SECS");
}

#[test]
fn untitled_documents_never_autosave() {
    let _g = ENV_LOCK.lock().unwrap();
    std::env::set_var("DC_UI_AUTORECOVER_SECS", "1");
    let dir = tempfile::tempdir().unwrap();
    let rec_dir = dir.path().join("rec");
    let mut app = App::with_blank(); // no path on disk
    let mut opts = dc_ui::options::Options::persisting_for_test(dir.path().join("options.json"));
    opts.autorecover_dir = rec_dir.display().to_string();
    app.set_options_store_for_test(opts);
    let mut h = harness_with(app);
    h.run();
    h.state_mut().simulate_edit_for_test();
    // Well past the interval: nothing may land (owner decision
    // 2026-11-28 — no original, no identity).
    for _ in 0..30 {
        h.run();
        std::thread::sleep(Duration::from_millis(50));
    }
    let any = std::fs::read_dir(&rec_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(any, 0, "an untitled document writes nothing");
    std::env::remove_var("DC_UI_AUTORECOVER_SECS");
}

#[test]
fn planted_recovery_file_prompts_and_restore_opens_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("report.vsdx"); // never written: gone
    let (snap, side) = plant(dir.path(), &original);
    let mut app = App::with_blank();
    app.scan_recoveries_for_test(dir.path());
    let mut h = harness_with(app);
    h.run();
    // The startup pane is up with the entry.
    assert!(h.query_by_label_contains("Document Recovery").is_some());
    assert!(h.query_by_label_contains("report.vsdx").is_some());

    // Restore: the snapshot opens through the normal (async, scrimmed)
    // load path; the landing presents the ORIGINAL, dirty — and the
    // recovery files STAY until that decision is saved or dropped
    // (a crash before the next save must not lose the recovered
    // changes, PR #419 review).
    h.get_by_label("Restore").click();
    wait_until(&mut h, |a| a.debug_dirty());
    assert!(h.state().debug_doc_open());
    assert_eq!(h.state().debug_doc_path(), original.display().to_string());
    assert!(
        snap.exists() && side.exists(),
        "the recovery files outlive the restore until a save or drop"
    );

    // The decision: saving the restored document clears them.
    h.state_mut().debug_save_for_test();
    h.run();
    wait_until(&mut h, |_| !snap.exists());
    assert!(!side.exists(), "the sidecar goes with the snapshot");
}

#[test]
fn planted_recovery_file_discard_deletes_the_copy() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("notes.vsdx");
    let (snap, side) = plant(dir.path(), &original);
    let mut app = App::with_blank();
    app.scan_recoveries_for_test(dir.path());
    let mut h = harness_with(app);
    h.run();
    assert!(h.query_by_label_contains("Document Recovery").is_some());
    h.get_by_label("Discard").click();
    h.run();
    assert!(!snap.exists() && !side.exists(), "Discard removes the files");
}

#[test]
fn escape_dismisses_the_pane_and_keeps_the_files() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("kept.vsdx");
    let (snap, side) = plant(dir.path(), &original);
    let mut app = App::with_blank();
    app.scan_recoveries_for_test(dir.path());
    let mut h = harness_with(app);
    h.run();
    assert!(h.query_by_label_contains("Document Recovery").is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(
        h.query_by_label_contains("Document Recovery").is_none(),
        "Esc dismisses the pane for this session"
    );
    assert!(snap.exists() && side.exists(), "the files stay");
}
