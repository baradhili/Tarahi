//! AutoRecover (GitHub #213) — the timed autosave of unsaved changes
//! and the startup recovery flow. Visio 2016 semantics: with "Save
//! AutoRecover information every N minutes" on (the Save page's
//! pre-existing settings, #268), unsaved changes are written to a
//! recovery location; after a crash the next start offers a Document
//! Recovery pane — per-file entries with the recovered timestamp,
//! Restore / Discard each — and saving or closing cleanly removes the
//! recovery copy.
//!
//! Decomposition (the owner's domain-module rule, as `options.rs`):
//! this module owns the file naming, the startup scan, the snapshot
//! write, the clears, and the recovery pane; the app shell keeps only
//! the state fields, the per-frame tick, and the outcome forwarding
//! (`app/files.rs`). Nothing here writes into a `.vsdx` the user owns.
//!
//! Naming: one document autosaves to `<stem>.autorecover.vsdx` beside
//! a `<stem>.autorecover.json` sidecar that carries the original path
//! (the scan compares the snapshot against it). Documents without a
//! path on disk never autosave (owner decision 2026-11-28 — no stem
//! to key the recovery file on). A snapshot written without its
//! sidecar still recovers (degraded: restored untitled); a sidecar
//! without its snapshot is skipped.
//!
//! Concurrency (the #104/#105 stencil-extractor discipline): the tick
//! runs at a frame boundary — never mid-command — clones the document
//! there, and the write runs on a short-lived worker thread so the
//! UI never blocks on disk. `Editor::save_as`'s craft-atomic stage +
//! rename makes every snapshot crash-safe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::Context;

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// The snapshot file's suffix (the issue's `<doc-stem>.autorecover.vsdx`).
const SNAPSHOT_SUFFIX: &str = ".autorecover.vsdx";
/// The sidecar's suffix (the original-path metadata beside it).
const SIDECAR_SUFFIX: &str = ".autorecover.json";

/// The document's recovery stem: its file name minus the extension
/// (`report.vsdx` → `report`). Untitled documents never reach this
/// (they never autosave).
pub(crate) fn stem_of(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

/// The snapshot + sidecar paths for one stem inside `dir`.
fn paths_for_stem(dir: &Path, stem: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{stem}{SNAPSHOT_SUFFIX}")),
        dir.join(format!("{stem}{SIDECAR_SUFFIX}")),
    )
}

/// The AutoRecover directories, in priority order: the configured
/// location when set (the Save page's field), then the app default
/// under the user config dir. Empty when no config base is
/// discoverable AND no location is configured — autosave then has
/// nowhere to write, and the scan has nothing to read. Pure: no
/// directory is created here (the my_shapes_dir rule — kittest must
/// not touch the disk; the write path creates its directory).
pub(crate) fn recovery_dirs(config_base: Option<&Path>, custom: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if !custom.trim().is_empty() {
        dirs.push(PathBuf::from(custom.trim()));
    }
    if let Some(base) = config_base {
        dirs.push(base.join("diagramcraft").join("autorecover"));
    }
    dirs
}

// ---------------------------------------------------------------------------
// Interval (the settings live in Options; the knob is the kittest seam)
// ---------------------------------------------------------------------------

/// The autosave interval: the Save page's choice (minutes, already
/// clamped 1..=120), overridden by `DC_UI_AUTORECOVER_SECS` — the
/// kittest knob: seconds precision (minutes cannot express a test
/// interval), `0` = off. `None` when AutoRecover is disabled or
/// switched off by the knob.
pub(crate) fn interval(minutes: u32, enabled: bool) -> Option<Duration> {
    if let Some(v) = std::env::var_os("DC_UI_AUTORECOVER_SECS") {
        let secs = v.to_str().and_then(|s| s.trim().parse::<u64>().ok());
        if let Some(secs) = secs {
            return if secs == 0 {
                None
            } else {
                Some(Duration::from_secs(secs))
            };
        }
        // Unparseable knob: fall through to the configured setting.
    }
    if !enabled {
        return None;
    }
    Some(Duration::from_secs(u64::from(minutes) * 60))
}

// ---------------------------------------------------------------------------
// Snapshot write
// ---------------------------------------------------------------------------

/// Serialize + write one recovery snapshot on a worker thread (the
/// stencil-extractor pattern: the caller cloned the document at a
/// frame boundary — a consistent read — and this thread owns it; the
/// UI never waits on the disk). Best-effort: a failed snapshot is
/// retried by the next tick, and nothing here touches the original.
pub(crate) fn spawn_snapshot_write(doc: vsdx::Document, original: PathBuf, dir: PathBuf) {
    std::thread::spawn(move || {
        let _ = write_snapshot(doc, &original, &dir);
    });
}

/// The synchronous half of [`spawn_snapshot_write`] (the unit tests'
/// seam): create the directory, write the snapshot through dc-core's
/// atomic save path, then the sidecar. The sidecar is written AFTER
/// the snapshot so a crash between the two leaves a recoverable
/// snapshot (restored untitled), never a dangling sidecar — the scan
/// skips sidecars whose snapshot is missing.
fn write_snapshot(doc: vsdx::Document, original: &Path, dir: &Path) -> std::io::Result<()> {
    let Some(stem) = stem_of(original) else {
        return Err(std::io::Error::other("untitled documents never autosave"));
    };
    std::fs::create_dir_all(dir)?;
    let (snapshot, sidecar) = paths_for_stem(dir, &stem);
    // dc-core's save path: craft-atomic stage + rename — the snapshot
    // is never a half-written package, even under a crash.
    dc_core::Editor::new(doc)
        .save_as(&snapshot)
        .map_err(|e| std::io::Error::other(format!("{e:#}")))?;
    let meta = serde_json::json!({
        "version": 1,
        "original": original.display().to_string(),
    });
    std::fs::write(&sidecar, meta.to_string())
}

// ---------------------------------------------------------------------------
// Startup scan
// ---------------------------------------------------------------------------

/// One recoverable document: the recovery snapshot, its sidecar, the
/// original file (when the sidecar named one), the snapshot's mtime
/// (Unix seconds), and the recovery stem.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryEntry {
    pub stem: String,
    pub recovery_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub original: Option<PathBuf>,
    /// The snapshot's mtime as Unix seconds (0 when the clock could
    /// not read it).
    pub recovered: i64,
}

impl RecoveryEntry {
    /// The pane's per-entry display name: the original file's name
    /// when known, else the recovery stem.
    pub fn name(&self) -> String {
        self.original
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.stem.clone())
    }
}

/// A restore's pending landing (the app shell's glue field): the
/// entry restored, the original path to present (None = untitled),
/// and the recovery files to delete once the load lands. A FAILED
/// load re-offers the entry (the snapshot is the user's to Discard).
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreLanding {
    pub entry: RecoveryEntry,
    pub original: Option<PathBuf>,
}

/// The startup scan (GitHub #213): every `*.autorecover.vsdx` in the
/// given directories whose original is GONE or OLDER than the
/// snapshot (the issue's rule — "recovery files newer than their
/// originals, or originals gone"). A stale snapshot (the original
/// moved past it) is left alone, never deleted. Entries dedupe by
/// stem, newest first — two documents may share a file name from
/// different directories, and one stem holds one snapshot.
pub(crate) fn scan(dirs: &[PathBuf]) -> Vec<RecoveryEntry> {
    let mut out: Vec<RecoveryEntry> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            let recovery_path = e.path();
            if !recovery_path.is_file() {
                continue;
            }
            let Some(name) = recovery_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(SNAPSHOT_SUFFIX) else {
                continue;
            };
            let stem = stem.to_string();
            let recovered = mtime_secs(&recovery_path);
            let sidecar_path = dir.join(format!("{stem}{SIDECAR_SUFFIX}"));
            let original = read_sidecar(&sidecar_path);
            // The issue's offer rule. No sidecar counts as "original
            // gone" (degraded recoverability — see the module doc).
            let offered = match &original {
                Some(orig) => match std::fs::metadata(orig) {
                    Err(_) => true,
                    Ok(m) => match m.modified() {
                        Ok(orig_time) => orig_time
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| recovered > d.as_secs() as i64)
                            .unwrap_or(true),
                        Err(_) => true,
                    },
                },
                None => true,
            };
            if !offered {
                continue;
            }
            out.push(RecoveryEntry {
                stem,
                recovery_path,
                sidecar_path,
                original,
                recovered,
            });
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.recovered));
    out.dedup_by_key(|e| e.stem.clone());
    out
}

/// Read the sidecar's original path; `None` on a missing, corrupt, or
/// unknown-version sidecar (degraded recoverability).
fn read_sidecar(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if v.get("version")?.as_i64()? != 1 {
        return None;
    }
    let orig = v.get("original")?.as_str()?;
    if orig.is_empty() {
        return None;
    }
    Some(PathBuf::from(orig))
}

/// A file's mtime as Unix seconds (0 when unreadable or pre-epoch).
fn mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Clears
// ---------------------------------------------------------------------------

/// Remove one document's recovery snapshot + sidecar (its stem's
/// pair inside `dir`). Best-effort: missing files are the expected
/// "nothing to clear" case.
pub(crate) fn clear_stem(dir: &Path, stem: &str) {
    let (snapshot, sidecar) = paths_for_stem(dir, stem);
    let _ = std::fs::remove_file(snapshot);
    let _ = std::fs::remove_file(sidecar);
}

/// Remove the exact recovery files an entry owns (Discard, and the
/// delete after a successful Restore).
pub(crate) fn clear_files(files: &[PathBuf]) {
    for f in files {
        let _ = std::fs::remove_file(f);
    }
}

/// The files one entry owns, for the landing/Discard deletes.
pub(crate) fn entry_files(entry: &RecoveryEntry) -> Vec<PathBuf> {
    vec![
        entry.recovery_path.clone(),
        entry.sidecar_path.clone(),
    ]
}

// ---------------------------------------------------------------------------
// Timestamps (UTC, zero-dep — shared with the comment dates)
// ---------------------------------------------------------------------------

/// Unix seconds now (0 before the epoch: a pre-1970 clock shows the
/// epoch date, never a panic).
pub(crate) fn epoch_secs_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` (UTC) — the zero-dep civil-from-days conversion
/// (Hinnant's algorithm) over the Unix epoch seconds. The comment
/// dates (`app::format_now_utc`) share this.
pub(crate) fn utc_date_string(secs: i64) -> String {
    let (y, m, d, _, _) = civil_from_epoch(secs);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM` (UTC) — the recovery pane's per-entry timestamp
/// (the issue's "recovered timestamp").
pub(crate) fn utc_timestamp_string(secs: i64) -> String {
    let (y, m, d, hh, mm) = civil_from_epoch(secs);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// Civil date + time of day from Unix seconds, zero-dep.
fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        y,
        m,
        d,
        (secs_of_day / 3600) as u32,
        ((secs_of_day % 3600) / 60) as u32,
    )
}

/// Hinnant's civil-from-days: days since 1970-01-01 → (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Recovery pane
// ---------------------------------------------------------------------------

/// One pane action for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryAction {
    Restore,
    Discard,
}

/// What the pane did this frame: per-entry actions (index into the
/// entries the caller passed) and whether Esc dismissed it.
#[derive(Debug, Default)]
pub(crate) struct PaneResult {
    pub actions: Vec<(usize, EntryAction)>,
    pub dismissed: bool,
}

/// The startup Document Recovery pane (GitHub #213): a MODAL (the
/// #166 `egui::Modal` pattern, as the close prompt and Options) over
/// whatever the first frames draw — the splash and the loading scrim
/// sit above it, so it reveals itself as the boot overlays clear.
/// One row per entry: the document's name, the recovered timestamp,
/// and Restore / Discard. The pane clears when every entry is
/// handled; Esc dismisses it for this session (the files stay and
/// the next start offers them again). A backdrop click is ignored —
/// Windows-dialog semantics, like Options.
pub(crate) fn recovery_window(ctx: &Context, entries: &[RecoveryEntry], esc_blocked: bool) -> PaneResult {
    let mut result = PaneResult::default();
    let strs = crate::i18n::strs();
    egui::Modal::new(egui::Id::unique("dc-recovery")).show(ctx, |ui| {
        ui.with_layout(crate::app::dialog_dir(), |ui| {
            ui.strong(strs.msg_document_recovery());
            ui.add_space(4.0);
            ui.label(strs.msg_recovery_description());
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("dc-recovery-entries")
                .show(ui, |ui| {
                    for (ix, entry) in entries.iter().enumerate() {
                        crate::app::row(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(entry.name());
                                ui.weak(strs.msg_recovery_recovered_at(
                                    utc_timestamp_string(entry.recovered),
                                ));
                            });
                            if ui.button(strs.msg_recovery_restore()).clicked() {
                                result.actions.push((ix, EntryAction::Restore));
                            }
                            if ui.button(strs.msg_recovery_discard()).clicked() {
                                result.actions.push((ix, EntryAction::Discard));
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
        });
    });
    // Esc dismisses for this session — but only when no modal above
    // (a confirm prompt) owns the key this frame (the esc_blocked
    // contract, as Options' dialog).
    if !esc_blocked && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        result.dismissed = true;
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The kittest env knob is process-global: tests in this binary
    /// serialize their env writes on this lock.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_knob(v: Option<&str>) {
        let _g = ENV_LOCK.lock().unwrap();
        match v {
            Some(s) => std::env::set_var("DC_UI_AUTORECOVER_SECS", s),
            None => std::env::remove_var("DC_UI_AUTORECOVER_SECS"),
        }
    }

    #[test]
    fn stem_and_names_follow_the_issue_convention() {
        let stem = stem_of(Path::new("/tmp/report.vsdx")).unwrap();
        assert_eq!(stem, "report");
        let dir = Path::new("/rec");
        let (snap, side) = paths_for_stem(dir, &stem);
        assert_eq!(snap, Path::new("/rec/report.autorecover.vsdx"));
        assert_eq!(side, Path::new("/rec/report.autorecover.json"));
    }

    #[test]
    fn untitled_documents_have_no_stem() {
        assert_eq!(stem_of(Path::new("")), None);
    }

    #[test]
    fn recovery_dirs_prefer_the_custom_location_and_need_no_disk() {
        let base = Path::new("/cfg");
        // Custom location set: it leads, the default follows.
        assert_eq!(
            recovery_dirs(Some(base), "/custom/rec"),
            vec![
                PathBuf::from("/custom/rec"),
                PathBuf::from("/cfg/diagramcraft/autorecover")
            ]
        );
        // No custom: the default only.
        assert_eq!(
            recovery_dirs(Some(base), ""),
            vec![PathBuf::from("/cfg/diagramcraft/autorecover")]
        );
        // No config base and no custom: nowhere to write, nothing to scan.
        assert!(recovery_dirs(None, "").is_empty());
    }

    #[test]
    fn interval_follows_the_save_page_setting() {
        set_knob(None);
        assert_eq!(interval(10, true), Some(Duration::from_secs(600)));
        assert_eq!(interval(10, false), None);
    }

    #[test]
    fn the_env_knob_overrides_in_seconds_and_zero_turns_it_off() {
        set_knob(Some("1"));
        assert_eq!(interval(10, true), Some(Duration::from_secs(1)));
        set_knob(Some("0"));
        assert_eq!(interval(10, true), None, "0 = off");
        set_knob(Some("0"));
        assert_eq!(interval(10, false), None);
        // An unparseable knob falls back to the configured setting.
        set_knob(Some("soon"));
        assert_eq!(interval(10, true), Some(Duration::from_secs(600)));
        set_knob(None);
    }

    #[test]
    fn write_snapshot_writes_a_parseable_package_and_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("report.vsdx");
        let dir = tmp.path().join("rec");
        let doc = dc_core::Editor::new_blank().doc;
        write_snapshot(doc, &original, &dir).unwrap();
        let (snap, side) = paths_for_stem(&dir, "report");
        assert!(snap.is_file(), "snapshot written");
        assert!(side.is_file(), "sidecar written");
        // The snapshot is a real package (the Restore open path's
        // parse).
        let parsed = vsdx::parse::parse(&snap);
        assert!(parsed.is_ok(), "the snapshot parses: {parsed:?}");
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&side).unwrap()).unwrap();
        assert_eq!(meta["version"], 1);
        assert_eq!(meta["original"], original.display().to_string());
    }

    #[test]
    fn scan_missing_and_empty_dirs_offer_nothing() {
        assert!(scan(&[PathBuf::from("/nonexistent/rec")]).is_empty());
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan(&[tmp.path().to_path_buf()]).is_empty());
    }

    /// Plant a snapshot + sidecar pair; returns the paths.
    fn plant(dir: &Path, stem: &str, original: Option<&Path>) -> (PathBuf, PathBuf) {
        let (snap, side) = paths_for_stem(dir, stem);
        let doc = dc_core::Editor::new_blank().doc;
        dc_core::Editor::new(doc)
            .save_as(&snap)
            .expect("snapshot write");
        if let Some(orig) = original {
            let meta = serde_json::json!({"version": 1, "original": orig.display().to_string()});
            std::fs::write(&side, meta.to_string()).unwrap();
        }
        (snap, side)
    }

    #[test]
    fn a_snapshot_with_the_original_gone_is_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = tmp.path().join("gone.vsdx"); // never written
        let (snap, side) = plant(dir, "gone", Some(&original));
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1, "original missing → offered");
        assert_eq!(found[0].recovery_path, snap);
        assert_eq!(found[0].sidecar_path, side);
        assert_eq!(found[0].original.as_deref(), Some(original.as_path()));
        assert_eq!(found[0].name(), "gone.vsdx");
    }

    #[test]
    fn a_snapshot_without_a_sidecar_is_offered_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        plant(dir, "bare", None);
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, None, "no sidecar: original unknown");
        assert_eq!(found[0].name(), "bare", "the stem is the display name");
    }

    #[test]
    fn a_stale_snapshot_older_than_its_original_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The snapshot lands FIRST, the original a moment later (the
        // sleep keeps the order real — same-second writes would tie).
        let (snap, side) = paths_for_stem(dir, "saved");
        dc_core::Editor::new_blank().save_as(&snap).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let original = tmp.path().join("saved.vsdx");
        dc_core::Editor::new_blank().save_as(&original).unwrap();
        let meta = serde_json::json!({"version": 1, "original": original.display().to_string()});
        std::fs::write(&side, meta.to_string()).unwrap();
        let newer = original.metadata().unwrap().modified().unwrap();
        let older = snap.metadata().unwrap().modified().unwrap();
        assert!(
            newer > older,
            "test setup: the original must be newer ({} vs {})",
            unix_of(newer),
            unix_of(older)
        );
        assert!(
            scan(&[dir.to_path_buf()]).is_empty(),
            "the original moved past the snapshot → not offered"
        );
    }

    #[test]
    fn a_snapshot_newer_than_its_original_is_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = tmp.path().join("live.vsdx");
        dc_core::Editor::new_blank().save_as(&original).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        plant(dir, "live", Some(&original));
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1, "unsaved changes beyond the original");
    }

    #[test]
    fn a_sidecar_without_its_snapshot_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (_, side) = paths_for_stem(dir, "orphan");
        std::fs::write(&side, r#"{"version":1,"original":"/x/y.vsdx"}"#).unwrap();
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    #[test]
    fn scan_dedupes_by_stem_newest_first() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        plant(a.path(), "dup", None);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        plant(b.path(), "dup", None);
        let found = scan(&[a.path().to_path_buf(), b.path().to_path_buf()]);
        assert_eq!(found.len(), 1, "one stem holds one snapshot");
        assert_eq!(
            found[0].recovery_path.parent(),
            Some(b.path()),
            "the newest snapshot wins"
        );
    }

    #[test]
    fn clear_stem_removes_both_files_and_tolerates_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        plant(dir, "doc", None);
        clear_stem(dir, "doc");
        let (snap, side) = paths_for_stem(dir, "doc");
        assert!(!snap.exists() && !side.exists(), "both removed");
        // Idempotent.
        clear_stem(dir, "doc");
    }

    #[test]
    fn non_snapshot_files_are_ignored_by_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("notes.txt"), "not a snapshot").unwrap();
        std::fs::write(dir.join("other.autorecover.json"), "{}").unwrap();
        std::fs::write(dir.join("drawing.vsdx"), "not ours").unwrap();
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    #[test]
    fn corrupt_sidecars_degrade_to_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (snap, side) = plant(dir, "broken", None);
        std::fs::write(&side, "{not json").unwrap();
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, None);
        assert_eq!(found[0].recovery_path, snap);
    }

    #[test]
    fn future_version_sidecars_degrade_to_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (_, side) = paths_for_stem(dir, "future");
        std::fs::write(&side, r#"{"version":9,"original":"/x.vsdx"}"#).unwrap();
        // A sidecar alone is still skipped (no snapshot beside it).
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    #[test]
    fn timestamps_render_as_utc_date_and_minute() {
        assert_eq!(utc_date_string(0), "1970-01-01");
        assert_eq!(utc_timestamp_string(0), "1970-01-01 00:00");
        assert_eq!(utc_timestamp_string(86_399), "1970-01-01 23:59");
        // 2023-11-14 22:13:20 UTC.
        assert_eq!(utc_timestamp_string(1_700_000_000), "2023-11-14 22:13");
        // A leap day: 2024-02-29 12:00 UTC.
        assert_eq!(utc_timestamp_string(1_709_208_000), "2024-02-29 12:00");
        // Pre-epoch seconds convert correctly (euclid division — no
        // panic, no clamp needed).
        assert_eq!(utc_date_string(-1), "1969-12-31");
        assert_eq!(utc_timestamp_string(-1), "1969-12-31 23:59");
    }

    fn unix_of(t: std::time::SystemTime) -> i64 {
        t.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}
