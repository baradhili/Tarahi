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
//! Identity: one document is keyed by its full original path (the
//! file name alone collides — two `report.vsdx` from different
//! directories are different documents, PR #419 review). The
//! snapshot's file name carries that key's stable hash:
//! `<stem>-<hash>.autorecover.vsdx`, so the same document replaces
//! its own snapshot and no two documents can share one. A sidecar
//! `<stem>-<hash>.autorecover.json` carries the original path (the
//! scan compares the snapshot against it). Documents without a path
//! on disk never autosave (owner decision 2026-11-28 — no original,
//! no identity). A snapshot written without its sidecar still
//! recovers (degraded: restored untitled); a sidecar without its
//! snapshot is skipped.
//!
//! Concurrency (the #104/#105 stencil-extractor discipline): the tick
//! runs at a frame boundary — never mid-command — clones the document
//! there, and the write runs on a short-lived worker thread so the
//! UI never blocks on disk. Publication is serialized under a write
//! gate and generation-checked: a worker dispatched before a clear
//! (save / close / new) publishes nothing — its generation no longer
//! matches, so a cleared pair cannot be resurrected by a straggler
//! (PR #419 review). The staged package is renamed only after that
//! check, so the publish window is airtight. Restore keeps the
//! recovery files until the restored document is saved or explicitly
//! closed/dropped — a crash between Restore and the next save must
//! not lose the recovered changes (PR #419 review); the existing
//! clears are exactly that decision.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use egui::Context;

// ---------------------------------------------------------------------------
// Naming + identity
// ---------------------------------------------------------------------------

/// The snapshot file's suffix.
const SNAPSHOT_SUFFIX: &str = ".autorecover.vsdx";
/// The sidecar's suffix.
const SIDECAR_SUFFIX: &str = ".autorecover.json";
/// Hash width in the file name (FNV-1a 64, hex).
const HASH_LEN: usize = 16;

/// A document's recovery identity: its full original path. The file
/// name alone is NOT an identity — two `report.vsdx` from different
/// directories are different documents (PR #419 review), and each
/// must keep its own snapshot.
pub(crate) fn key_of(original: &Path) -> String {
    original.to_string_lossy().into_owned()
}

/// The identity's stable hash for the file name (FNV-1a 64 — a
/// hand-rolled constant is deliberate: `DefaultHasher` is not stable
/// across runs, and the file name must survive restarts).
fn hash64(key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The document's recovery stem: its file name minus the extension
/// (`report.vsdx` → `report`). Untitled documents never reach this
/// (they never autosave).
pub(crate) fn stem_of(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

/// The snapshot + sidecar paths for one document inside `dir`.
fn paths_for_key(dir: &Path, stem: &str, key: &str) -> (PathBuf, PathBuf) {
    let core = format!("{stem}-{}", hash64(key));
    (
        dir.join(format!("{core}{SNAPSHOT_SUFFIX}")),
        dir.join(format!("{core}{SIDECAR_SUFFIX}")),
    )
}

/// The file-name core's display stem: the part before the identity
/// hash (`report-85e7…` → `report`). A core that does not carry a
/// 16-hex hash tail displays as-is.
fn display_stem(file_core: &str) -> String {
    match file_core.rsplit_once('-') {
        Some((stem, hash))
            if hash.len() == HASH_LEN && hash.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            stem.to_string()
        }
        _ => file_core.to_string(),
    }
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
// Snapshot write: dispatch → stage → generation-checked publish
// ---------------------------------------------------------------------------

/// The per-identity generation counter. Every dispatch AND every
/// clear bumps it; a worker publishes only while its dispatch
/// generation is still current. A clear therefore invalidates any
/// worker it was racing: a deleted pair is never resurrected by a
/// straggler (PR #419 review).
struct PublishState {
    gens: HashMap<String, u64>,
}

static PUBLISH: OnceLock<Mutex<PublishState>> = OnceLock::new();

fn publish_state() -> &'static Mutex<PublishState> {
    PUBLISH.get_or_init(|| Mutex::new(PublishState { gens: HashMap::new() }))
}

/// The write gate: snapshot publishes are serialized, so a slow write
/// can never rename its OLDER clone over a later dispatch's newer one
/// — publication order equals dispatch order (PR #419 review).
static WRITE_GATE: Mutex<()> = Mutex::new(());

/// Bump (or start) an identity's generation and return it.
fn next_gen(key: &str) -> u64 {
    let mut st = publish_state().lock().unwrap();
    let g = st.gens.entry(key.to_string()).or_insert(0);
    *g += 1;
    *g
}

/// Invalidate any in-flight worker for `key` without dispatching one.
fn invalidate(key: &str) {
    let _ = next_gen(key);
}

/// Dispatch one recovery snapshot write on a worker thread (the
/// stencil-extractor pattern: the caller cloned the document at a
/// frame boundary — a consistent read — and this thread owns it; the
/// UI never waits on the disk). Best-effort: a failed snapshot is
/// retried by the next tick, and nothing here touches the original.
pub(crate) fn dispatch_snapshot(doc: vsdx::Document, original: PathBuf, dir: PathBuf) {
    let key = key_of(&original);
    let gen = next_gen(&key);
    std::thread::spawn(move || {
        // Stage the package (the slow half — no locks held).
        let Some(stem) = stem_of(&original) else {
            return; // untitled documents never autosave
        };
        let (snapshot, sidecar) = paths_for_key(&dir, &stem, &key);
        let staged = dir.join(format!(
            "{}-{}.autorecover.{}.staging",
            stem,
            hash64(&key),
            gen
        ));
        let _ = std::fs::create_dir_all(&dir);
        if vsdx::write::save(&doc, &staged).is_err() {
            return; // retried by the next tick
        }
        // Publish under the gate (serialized), and only while this
        // dispatch's generation is still current. The check and the
        // rename hold the generations lock together — a clear (which
        // bumps the generation under the same lock) cannot interleave.
        let _gate = WRITE_GATE.lock().unwrap();
        {
            let st = publish_state().lock().unwrap();
            if st.gens.get(&key) != Some(&gen) {
                drop(st);
                let _ = std::fs::remove_file(&staged);
                return;
            }
        }
        let renamed = std::fs::rename(&staged, &snapshot).is_ok();
        if renamed {
            let meta = serde_json::json!({
                "version": 1,
                "original": original.display().to_string(),
            });
            let _ = std::fs::write(&sidecar, meta.to_string());
        }
    });
}

/// The synchronous write used by the tests: dispatch + run the whole
/// stage/publish body inline (same naming, same generation bump).
#[doc(hidden)]
pub fn write_snapshot_for_test(doc: vsdx::Document, original: PathBuf, dir: PathBuf) -> bool {
    let key = key_of(&original);
    let gen = next_gen(&key);
    let Some(stem) = stem_of(&original) else {
        return false;
    };
    let (snapshot, sidecar) = paths_for_key(&dir, &stem, &key);
    let _ = std::fs::create_dir_all(&dir);
    let staged = dir.join(format!("{}-{}.autorecover.{}.staging", stem, hash64(&key), gen));
    if vsdx::write::save(&doc, &staged).is_err() {
        return false;
    }
    let ok = std::fs::rename(&staged, &snapshot).is_ok();
    if ok {
        let meta = serde_json::json!({
            "version": 1,
            "original": original.display().to_string(),
        });
        let _ = std::fs::write(&sidecar, meta.to_string());
    }
    ok
}

/// The recovery pair's paths for one document inside `dir` (the
/// kittest's seam for asserting on exactly the files production
/// writes).
#[doc(hidden)]
pub fn recovery_paths_for_test(dir: &Path, original: &Path) -> (PathBuf, PathBuf) {
    let key = key_of(original);
    let stem = stem_of(original).unwrap_or_default();
    paths_for_key(dir, &stem, &key)
}

// ---------------------------------------------------------------------------
// Startup scan
// ---------------------------------------------------------------------------

/// One recoverable document: the recovery snapshot, its sidecar, the
/// original file (when the sidecar named one), the snapshot's mtime
/// (Unix seconds — DISPLAY only; the offer comparison keeps the full
/// precision), and the display stem.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryEntry {
    /// The display stem (the file name's core without the identity
    /// hash) — the fallback name when no sidecar names an original.
    pub stem: String,
    /// The dedupe identity: the original path when the sidecar named
    /// one, else this snapshot file's own core (an unknown-original
    /// snapshot is its own entry).
    identity: String,
    pub recovery_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub original: Option<PathBuf>,
    /// The snapshot's mtime as Unix seconds (0 when the clock could
    /// not read it) — the pane's timestamp. The newest-first ORDER
    /// sorts the full-precision [`Self::mtime`], never this
    /// truncated value.
    pub recovered: i64,
    /// The snapshot's full-precision mtime (the dedupe order).
    pub(crate) mtime: SystemTime,
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
/// entry restored and the original path to present (None = untitled).
/// The recovery files STAY on disk until the restored document is
/// saved or explicitly closed/dropped (the existing clears) — a
/// crash between Restore and that decision must not lose the
/// recovered changes (PR #419 review). A FAILED load re-offers the
/// entry (the snapshot is the user's to Discard).
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreLanding {
    pub entry: RecoveryEntry,
    pub original: Option<PathBuf>,
}

/// The recovery files a RESTORE left on disk, remembered by the app
/// shell so the decision sites (save / close / new / exit) can delete
/// them EXACTLY — the identity-stem clear only reaches the currently
/// configured recovery dirs, and a snapshot restored from a
/// since-unconfigured location would otherwise ghost-reoffer after a
/// save (PR #419 review round 2). `key` is the restored document's
/// identity when known: clearing invalidates its in-flight autosave
/// worker, the same contract as [`clear_original`].
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredCopy {
    pub key: Option<String>,
    pub files: Vec<PathBuf>,
}

/// Delete a restored copy's exact files (and invalidate its
/// identity's in-flight worker when known).
pub(crate) fn clear_restored(r: &RestoredCopy) {
    if let Some(key) = &r.key {
        invalidate(key);
    }
    clear_files(&r.files);
}

/// The startup scan (GitHub #213): every `*.autorecover.vsdx` in the
/// given directories whose original is GONE or OLDER than the
/// snapshot (the issue's rule — "recovery files newer than their
/// originals, or originals gone"; compared at full mtime precision —
/// a second-truncated comparison would exclude snapshots written
/// within the same second as their original, PR #419 review). A stale
/// snapshot (the original moved past it) is left alone, never
/// deleted. Entries dedupe by identity across the WHOLE newest-first
/// list — `dedup_by_key` only removes consecutive duplicates, and
/// same-identity entries with different mtimes interleave (PR #419
/// review).
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
            let Some(file_core) = name.strip_suffix(SNAPSHOT_SUFFIX) else {
                continue;
            };
            let recovered = mtime_secs(&recovery_path);
            let mtime = std::fs::metadata(&recovery_path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let sidecar_path = dir.join(format!("{file_core}{SIDECAR_SUFFIX}"));
            let original = read_sidecar(&sidecar_path);
            // The issue's offer rule. No sidecar counts as "original
            // gone" (degraded recoverability — see the module doc).
            let offered = match &original {
                Some(orig) => match std::fs::metadata(orig) {
                    Err(_) => true,
                    Ok(m) => match m.modified() {
                        // Full-precision instants, never truncated to
                        // whole seconds.
                        Ok(orig_time) => match std::fs::metadata(&recovery_path) {
                            Ok(sm) => sm.modified().map(|t| orig_time < t).unwrap_or(true),
                            Err(_) => true,
                        },
                        Err(_) => true,
                    },
                },
                None => true,
            };
            if !offered {
                continue;
            }
            // Identity: the original path when the sidecar named one,
            // else this file alone (unknown originals never merge).
            let identity = original
                .as_ref()
                .map(|p| key_of(p))
                .unwrap_or_else(|| file_core.to_string());
            out.push(RecoveryEntry {
                stem: display_stem(file_core),
                identity,
                recovery_path,
                sidecar_path,
                original,
                recovered,
                mtime,
            });
        }
    }
    // Newest first (FULL precision — same-second snapshots must
    // still order deterministically), then keep the FIRST entry per
    // identity across the complete list (the retained one is the
    // newest).
    out.sort_by_key(|e| std::cmp::Reverse(e.mtime));
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.identity.clone()));
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

/// Remove one document's recovery snapshot + sidecar from EVERY
/// recognized recovery directory — the custom Save-page location and
/// the app default can each hold a pair written while it was the
/// write dir, and a clear must reach all of them (PR #419 review) —
/// and invalidate any in-flight snapshot worker for the document, so
/// a pair deleted by a save/close/new cannot be recreated by a
/// straggler (PR #419 review). Best-effort: missing files are the
/// expected "nothing to clear" case.
pub(crate) fn clear_original(original: &Path, dirs: &[PathBuf]) {
    let key = key_of(original);
    invalidate(&key);
    let Some(stem) = stem_of(original) else {
        return; // untitled documents never had a snapshot
    };
    for dir in dirs {
        let (snapshot, sidecar) = paths_for_key(dir, &stem, &key);
        let _ = std::fs::remove_file(snapshot);
        let _ = std::fs::remove_file(sidecar);
    }
}

/// Remove the exact recovery files an entry owns (Discard — the
/// startup pane, where no autosave worker for that document can be in
/// flight: nothing is open yet).
pub(crate) fn clear_files(files: &[PathBuf]) {
    for f in files {
        let _ = std::fs::remove_file(f);
    }
}

/// The files one entry owns, for the Discard delete.
pub(crate) fn entry_files(entry: &RecoveryEntry) -> Vec<PathBuf> {
    vec![entry.recovery_path.clone(), entry.sidecar_path.clone()]
}

// ---------------------------------------------------------------------------
// Timestamps (UTC, zero-dep — shared with the comment dates)
// ---------------------------------------------------------------------------

/// Unix seconds now (0 before the epoch: a pre-1970 clock shows the
/// epoch date, never a panic).
pub(crate) fn epoch_secs_now() -> i64 {
    SystemTime::now()
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
pub(crate) fn recovery_window(
    ctx: &Context,
    entries: &[RecoveryEntry],
    esc_blocked: bool,
) -> PaneResult {
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
    /// serialize their env writes on this lock — held across the WHOLE
    /// test (set → assertions → restore), never released between the
    /// set and the assertion (PR #419 review).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run `f` with the knob forced to `v`, holding the env lock the
    /// whole time and restoring the previous value afterwards.
    fn with_knob(v: Option<&str>, f: impl FnOnce()) {
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("DC_UI_AUTORECOVER_SECS").ok();
        match v {
            Some(s) => std::env::set_var("DC_UI_AUTORECOVER_SECS", s),
            None => std::env::remove_var("DC_UI_AUTORECOVER_SECS"),
        }
        f();
        match prev {
            Some(p) => std::env::set_var("DC_UI_AUTORECOVER_SECS", p),
            None => std::env::remove_var("DC_UI_AUTORECOVER_SECS"),
        }
    }

    #[test]
    fn identity_is_the_full_path_and_the_names_carry_its_hash() {
        let a = Path::new("/work/a/report.vsdx");
        let b = Path::new("/work/b/report.vsdx");
        // Different directories, same file name: different identities.
        assert_ne!(key_of(a), key_of(b));
        let dir = Path::new("/rec");
        let (a_snap, a_side) = paths_for_key(dir, "report", &key_of(a));
        let (b_snap, b_side) = paths_for_key(dir, "report", &key_of(b));
        assert_ne!(a_snap, b_snap, "same-stem documents never share a pair");
        assert_ne!(a_side, b_side);
        // The same document maps to the same pair (replacement works).
        assert_eq!(
            paths_for_key(dir, "report", &key_of(a)),
            paths_for_key(dir, "report", &key_of(a))
        );
        // The name keeps the human stem before the hash tail.
        let core = a_snap
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_suffix(SNAPSHOT_SUFFIX)
            .unwrap()
            .to_string();
        assert_eq!(display_stem(&core), "report");
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
        with_knob(None, || {
            assert_eq!(interval(10, true), Some(Duration::from_secs(600)));
            assert_eq!(interval(10, false), None);
        });
    }

    #[test]
    fn the_env_knob_overrides_in_seconds_and_zero_turns_it_off() {
        with_knob(Some("1"), || {
            assert_eq!(interval(10, true), Some(Duration::from_secs(1)));
        });
        with_knob(Some("0"), || {
            assert_eq!(interval(10, true), None, "0 = off");
            assert_eq!(interval(10, false), None);
        });
        // An unparseable knob falls back to the configured setting.
        with_knob(Some("soon"), || {
            assert_eq!(interval(10, true), Some(Duration::from_secs(600)));
        });
    }

    #[test]
    fn a_snapshot_writes_a_parseable_package_and_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("report.vsdx");
        let dir = tmp.path().join("rec");
        let doc = dc_core::Editor::new_blank().doc;
        assert!(write_snapshot_for_test(doc, original.clone(), dir.clone()));
        let (snap, side) = recovery_paths_for_test(&dir, &original);
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
        // Re-writing the SAME document replaces its own pair (the
        // staging generation does not litter).
        let doc = dc_core::Editor::new_blank().doc;
        assert!(write_snapshot_for_test(doc, original.clone(), dir.clone()));
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".staging"))
            .collect();
        assert!(leftovers.is_empty(), "staging files are renamed away");
    }

    #[test]
    fn scan_missing_and_empty_dirs_offer_nothing() {
        assert!(scan(&[PathBuf::from("/nonexistent/rec")]).is_empty());
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan(&[tmp.path().to_path_buf()]).is_empty());
    }

    /// Plant a snapshot + sidecar pair for `original` via the real
    /// write path; returns the pair's paths.
    fn plant(dir: &Path, original: &Path, write_original_too: bool) -> (PathBuf, PathBuf) {
        let doc = dc_core::Editor::new_blank().doc;
        assert!(write_snapshot_for_test(doc, original.to_path_buf(), dir.to_path_buf()));
        if write_original_too {
            dc_core::Editor::new_blank().save_as(original).unwrap();
        }
        recovery_paths_for_test(dir, original)
    }

    #[test]
    fn a_snapshot_with_the_original_gone_is_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = tmp.path().join("gone.vsdx"); // never written
        let (snap, side) = plant(dir, &original, false);
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1, "original missing → offered");
        assert_eq!(found[0].recovery_path, snap);
        assert_eq!(found[0].sidecar_path, side);
        assert_eq!(found[0].original.as_deref(), Some(original.as_path()));
        assert_eq!(found[0].name(), "gone.vsdx");
        assert_eq!(found[0].stem, "gone", "the display stem drops the hash");
    }

    #[test]
    fn a_snapshot_without_a_sidecar_is_offered_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (snap, _) = plant(dir, Path::new("/x/bare.vsdx"), false);
        std::fs::remove_file(recovery_paths_for_test(dir, Path::new("/x/bare.vsdx")).1).unwrap();
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, None, "no sidecar: original unknown");
        assert_eq!(found[0].name(), "bare", "the stem is the display name");
        assert_eq!(found[0].recovery_path, snap);
    }

    #[test]
    fn a_stale_snapshot_older_than_its_original_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The snapshot lands FIRST, the original a moment later (a
        // short sleep keeps the order real; the comparison is
        // full-precision, so sub-second spacing is enough).
        let original = tmp.path().join("saved.vsdx");
        let (snap, side) = plant(dir, &original, false);
        std::thread::sleep(Duration::from_millis(250));
        dc_core::Editor::new_blank().save_as(&original).unwrap();
        let newer = original.metadata().unwrap().modified().unwrap();
        let older = snap.metadata().unwrap().modified().unwrap();
        assert!(
            newer > older,
            "test setup: the original must be newer ({} vs {})",
            unix_of(newer),
            unix_of(older)
        );
        let _ = side;
        assert!(
            scan(&[dir.to_path_buf()]).is_empty(),
            "the original moved past the snapshot → not offered"
        );
    }

    #[test]
    fn a_snapshot_newer_than_its_original_is_offered_subsecond() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The original first, the snapshot a QUARTER SECOND later —
        // the second-truncated comparison the review flagged would
        // read both as the same second and skip it (PR #419 review).
        let original = tmp.path().join("live.vsdx");
        dc_core::Editor::new_blank().save_as(&original).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        plant(dir, &original, false);
        let found = scan(&[dir.to_path_buf()]);
        assert_eq!(
            found.len(),
            1,
            "unsaved changes beyond the original, sub-second spacing"
        );
    }

    #[test]
    fn a_sidecar_without_its_snapshot_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (_, side) = recovery_paths_for_test(dir, Path::new("/x/y.vsdx"));
        std::fs::write(&side, r#"{"version":1,"original":"/x/y.vsdx"}"#).unwrap();
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    #[test]
    fn scan_dedupes_by_identity_across_the_whole_list() {
        // The regression the review flagged: same-identity entries
        // with different mtimes INTERLEAVE in the newest-first sort,
        // so consecutive-only dedup keeps both. A middle document
        // forces the interleaving.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let original = Path::new("/docs/dup.vsdx");
        plant(a.path(), original, false); // old
        plant(b.path(), Path::new("/docs/mid.vsdx"), false); // middle
        std::thread::sleep(Duration::from_millis(250));
        plant(c.path(), original, false); // newest, same identity
        let found = scan(&[
            a.path().to_path_buf(),
            b.path().to_path_buf(),
            c.path().to_path_buf(),
        ]);
        let dups: Vec<_> = found.iter().filter(|e| e.name() == "dup.vsdx").collect();
        assert_eq!(dups.len(), 1, "one identity, one entry");
        assert_eq!(
            dups[0].recovery_path.parent(),
            Some(c.path()),
            "the newest snapshot wins"
        );
        assert_eq!(found.len(), 2, "dup + mid");
    }

    #[test]
    fn clear_original_reaches_every_dir_and_is_exact() {
        let custom = tempfile::tempdir().unwrap();
        let def = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let original = Path::new("/docs/doc.vsdx");
        let (snap, side) = plant(custom.path(), original, false);
        // A second document in the same dir keeps its pair.
        let other_original = Path::new("/docs/other.vsdx");
        let (o_snap, o_side) = plant(custom.path(), other_original, false);
        // And a stale pair in the DEFAULT dir (the user moved the
        // location after the snapshot was written).
        plant(def.path(), original, false);
        let dirs = vec![
            custom.path().to_path_buf(),
            def.path().to_path_buf(),
            other.path().to_path_buf(),
        ];
        clear_original(original, &dirs);
        assert!(!snap.exists() && !side.exists(), "custom dir cleared");
        let (d_snap, d_side) = recovery_paths_for_test(def.path(), original);
        assert!(!d_snap.exists() && !d_side.exists(), "default dir cleared");
        assert!(
            o_snap.exists() && o_side.exists(),
            "other documents keep their pairs"
        );
        // Idempotent.
        clear_original(original, &dirs);
    }

    #[test]
    fn non_snapshot_files_are_ignored_by_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("notes.txt"), "not a snapshot").unwrap();
        std::fs::write(dir.join("other.autorecover.json"), "{}").unwrap();
        std::fs::write(dir.join("drawing.vsdx"), "not ours").unwrap();
        std::fs::write(dir.join("report-abc.autorecover.vsdx.staging.1"), "staging").unwrap();
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    #[test]
    fn corrupt_sidecars_degrade_to_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = Path::new("/x/broken.vsdx");
        let (snap, side) = plant(dir, original, false);
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
        let (_, side) = recovery_paths_for_test(dir, Path::new("/x/future.vsdx"));
        std::fs::write(&side, r#"{"version":9,"original":"/x/future.vsdx"}"#).unwrap();
        // A sidecar alone is still skipped (no snapshot beside it).
        assert!(scan(&[dir.to_path_buf()]).is_empty());
    }

    /// Poll until `probe` turns true (the dispatch write runs on a
    /// worker thread).
    fn wait_for(probe: impl Fn() -> bool) {
        for _ in 0..100 {
            if probe() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the awaited condition never arrived");
    }

    #[test]
    fn dispatch_writes_the_pair_asynchronously() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = tmp.path().join("async.vsdx");
        let (snap, side) = recovery_paths_for_test(dir, &original);
        dispatch_snapshot(dc_core::Editor::new_blank().doc, original.clone(), dir.to_path_buf());
        wait_for(|| snap.is_file() && side.is_file());
        assert!(vsdx::parse::parse(&snap).is_ok());
    }

    #[test]
    fn a_clear_invalidates_the_in_flight_dispatch() {
        // The review's race: dispatch → clear → the straggler must
        // publish NOTHING (the deleted pair is never resurrected).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let original = tmp.path().join("raced.vsdx");
        let (snap, side) = recovery_paths_for_test(dir, &original);
        // Dispatch FIRST (its worker stages, then publishes), then
        // clear on the same identity: the generation bumps and the
        // worker's publish is rejected.
        dispatch_snapshot(dc_core::Editor::new_blank().doc, original.clone(), dir.to_path_buf());
        clear_original(&original, &[dir.to_path_buf()]);
        // Well past any staging write: nothing may land.
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            !snap.exists() && !side.exists(),
            "the invalidated worker published nothing"
        );
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

    fn unix_of(t: SystemTime) -> i64 {
        t.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}
