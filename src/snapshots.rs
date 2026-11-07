//! Local Time Machine snapshots: listing them, the command that deletes one,
//! an estimate of what deleting one frees, and the "settling" state in which
//! free space is watched while APFS releases it.
//!
//! macOS prints no per-snapshot size without root (see the design doc), so
//! [`estimate_held`] compares the mounted snapshot with the live volume: a file
//! that is gone, or that changed, is held by the snapshot. Checked against
//! Disk Utility's "Private Size" (138.1 MB against 137.9 MB). The private
//! size of a file *inside* a snapshot is never used: it reads 0 for all of them.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use crate::catalog::{CommandRunner, CommandSpec, RunControl};
use crate::scan::clone_facts;

const TMUTIL: &str = "/usr/bin/tmutil";
const DISKUTIL: &str = "/usr/sbin/diskutil";
const MOUNT_APFS: &str = "/sbin/mount_apfs";
/// The Data volume's live files; snapshots of it are compared against this.
pub const LIVE_ROOT: &str = "/System/Volumes/Data";
/// Where the system mounts local snapshots, in Time Machine's backup layout.
pub const BACKUPDB: &str = "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb";
pub const TM_PREFIX: &str = "com.apple.TimeMachine.";

/// Free space that has not moved for this many ticks (30 s) ends settling.
pub const STABLE_TICKS: u64 = 600;
/// Settling never lasts longer than this (5 minutes).
pub const SETTLE_MAX_TICKS: u64 = 6000;

/// One APFS snapshot of the system volumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    /// `YYYY-MM-DD-HHMMSS` for a Time Machine snapshot, `None` for any other
    /// kind (those are listed but never touched).
    pub date: Option<String>,
}

impl Snapshot {
    /// The key in the shared selection, for a Time Machine snapshot.
    pub fn id(&self) -> Option<String> {
        self.date.as_ref().map(|d| format!("snapshot:{d}"))
    }
}

/// Is this the date shape `tmutil` prints and accepts (`2026-11-07-032842`)?
pub fn valid_date(s: &str) -> bool {
    const SHAPE: [usize; 4] = [4, 2, 2, 6];
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 4
        && parts
            .iter()
            .zip(SHAPE)
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The valid dates in `tmutil listlocalsnapshotdates` output, oldest first.
pub fn parse_dates(out: &str) -> Vec<String> {
    let mut dates: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| valid_date(l))
        .map(str::to_string)
        .collect();
    dates.sort();
    dates.dedup();
    dates
}

/// Names of snapshots in `diskutil apfs listSnapshots` output that are not
/// Time Machine's (macOS update and boot snapshots).
pub fn parse_system_names(out: &str) -> Vec<String> {
    out.lines()
        // diskutil draws a tree: "|   Name:  ..." and "    Name:  ..."
        .filter_map(|l| {
            l.trim_start_matches(|c: char| c == '|' || c == '+' || c == '-' || c.is_whitespace())
                .strip_prefix("Name:")
        })
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && !n.starts_with(TM_PREFIX))
        .collect()
}

/// `tmutil deletelocalsnapshots <date>`, or `None` if `date` is not a date.
pub fn delete_spec(date: &str) -> Option<CommandSpec> {
    if !valid_date(date) {
        return None;
    }
    let mut spec = CommandSpec::new(TMUTIL, &["deletelocalsnapshots", date]);
    spec.timeout = Some(Duration::from_secs(600));
    Some(spec)
}

/// Every snapshot: Time Machine's (oldest first) then the system's.
/// Fails only when `tmutil` itself does not answer; the system ones are
/// extra information.
pub fn load(runner: &dyn CommandRunner) -> Result<Vec<Snapshot>, String> {
    let dates = runner
        .query(&CommandSpec::new(TMUTIL, &["listlocalsnapshotdates", "/"]))
        .ok_or_else(|| "tmutil did not list the snapshots".to_string())?;
    let mut out: Vec<Snapshot> = parse_dates(&dates)
        .into_iter()
        .map(|d| Snapshot {
            name: format!("{TM_PREFIX}{d}.local"),
            date: Some(d),
        })
        .collect();
    if let Some(text) = runner.query(&CommandSpec::new(DISKUTIL, &["apfs", "listSnapshots", "/"])) {
        out.extend(
            parse_system_names(&text)
                .into_iter()
                .map(|name| Snapshot { name, date: None }),
        );
    }
    Ok(out)
}

/// The folder of the Data volume inside the mounted snapshot of `date` under
/// `backupdb` (`<backupdb>/<host>/<date>/<volume>`), if the system mounted it.
pub fn find_mount(backupdb: &Path, date: &str) -> Option<PathBuf> {
    if !valid_date(date) {
        return None;
    }
    for host in fs::read_dir(backupdb).ok()?.flatten() {
        let day = host.path().join(date);
        if let Some(volume) = fs::read_dir(&day)
            .ok()
            .and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.is_dir()))
        {
            return Some(volume);
        }
    }
    None
}

/// What the snapshot at `snap` holds that `live` no longer does, in bytes.
/// A file gone from `live` counts in full; one whose size or modification
/// time differs counts what the live copy does not still share with it; the
/// same file counts nothing. `None` if `cancel` was set.
pub fn estimate_held(snap: &Path, live: &Path, cancel: &AtomicBool) -> Option<u64> {
    let total = AtomicU64::new(0);
    rayon::scope(|s| walk(snap, snap.to_path_buf(), live, cancel, &total, s));
    (!cancel.load(Relaxed)).then(|| total.load(Relaxed))
}

fn walk<'s>(
    root: &'s Path,
    dir: PathBuf,
    live: &'s Path,
    cancel: &'s AtomicBool,
    total: &'s AtomicU64,
    s: &rayon::Scope<'s>,
) {
    if cancel.load(Relaxed) {
        return;
    }
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            s.spawn(move |s| walk(root, path, live, cancel, total, s));
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            total.fetch_add(held_by_file(&meta, &live.join(rel)), Relaxed);
        }
    }
}

fn held_by_file(snap: &fs::Metadata, live_path: &Path) -> u64 {
    let snap_bytes = snap.blocks() * 512;
    let live = match fs::symlink_metadata(live_path) {
        Ok(l) => l,
        // Only "not there" means gone; any other failure says nothing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return snap_bytes,
        Err(_) => return 0,
    };
    if live.len() == snap.len()
        && live.mtime() == snap.mtime()
        && live.mtime_nsec() == snap.mtime_nsec()
    {
        return 0;
    }
    // The live copy's own private bytes are not shared with the snapshot; the
    // rest of its blocks are. (A cloned live file says nothing: assume none.)
    let shared = match clone_facts(live_path) {
        Some(c) if c.refcnt <= 1 => (live.blocks() * 512).saturating_sub(c.private),
        _ => 0,
    };
    snap_bytes.saturating_sub(shared)
}

/// How much deleting the Time Machine snapshot of `date` would free, about.
/// Uses the mount the system already made, or mounts the snapshot read-only
/// itself (no password) and unmounts it afterwards.
pub fn measure(runner: &dyn CommandRunner, date: &str, cancel: &AtomicBool) -> Result<u64, String> {
    if !valid_date(date) {
        return Err(format!("{date} is not a snapshot date"));
    }
    let live = Path::new(LIVE_ROOT);
    if let Some(mount) = find_mount(Path::new(BACKUPDB), date) {
        return estimate_held(&mount, live, cancel).ok_or_else(|| "cancelled".to_string());
    }
    let dir = std::env::temp_dir().join(format!("maccleanup-snap-{date}"));
    fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let name = format!("{TM_PREFIX}{date}.local");
    let mount = CommandSpec::new(
        MOUNT_APFS,
        &[
            "-o",
            "rdonly,nobrowse",
            "-s",
            &name,
            LIVE_ROOT,
            &dir.to_string_lossy(),
        ],
    );
    let result = if runner.run(&mount, &RunControl::new()).ok {
        let r = estimate_held(&dir, live, cancel).ok_or_else(|| "cancelled".to_string());
        let unmount = CommandSpec::new(DISKUTIL, &["unmount", &dir.to_string_lossy()]);
        runner.run(&unmount, &RunControl::new());
        r
    } else {
        Err("could not mount the snapshot read-only".to_string())
    };
    let _ = fs::remove_dir(&dir);
    result
}

/// Watches free space after a snapshot delete: APFS gives the space back
/// gradually, so the app looks often until the number has stopped moving.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settle {
    on: bool,
    until: u64,
    last_free: u64,
    changed_at: u64,
}

impl Settle {
    pub fn start(&mut self, tick: u64, free: u64) {
        *self = Settle {
            on: true,
            until: tick + SETTLE_MAX_TICKS,
            last_free: free,
            changed_at: tick,
        };
    }

    pub fn observe(&mut self, tick: u64, free: u64) {
        if free != self.last_free {
            self.last_free = free;
            self.changed_at = tick;
        }
    }

    pub fn active(&self, tick: u64) -> bool {
        self.on && tick < self.until && tick.saturating_sub(self.changed_at) < STABLE_TICKS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use std::time::{Duration, SystemTime};

    #[test]
    fn valid_date_accepts_only_the_tmutil_shape() {
        assert!(valid_date("2026-11-07-032842"));
        for bad in [
            "",
            "2026-11-07",
            "2026-11-07-03284",
            "2026-11-07-0328420",
            "2026-11-07-03284x",
            "2026/11/07-032842",
            "2026-11-07-032842; rm -rf /",
            "../2026-11-07-032842",
            "/",
        ] {
            assert!(!valid_date(bad), "{bad:?}");
        }
    }

    #[test]
    fn parse_dates_keeps_valid_sorted() {
        let out = "Snapshot dates for disk /:\n2026-11-07-032842\nnonsense\n2026-11-06-020608\n2026-11-07-032842\n";
        assert_eq!(parse_dates(out), ["2026-11-06-020608", "2026-11-07-032842"]);
        assert!(parse_dates("").is_empty());
    }

    #[test]
    fn parse_system_names_skip_time_machine() {
        let out = "|   Name:        com.apple.os.update-2A1B\n|   XID:         1\n+-- X\n    Name:        com.apple.TimeMachine.2026-11-07-032842.local\n|   Name:        com.apple.bless.3D4C\n";
        assert_eq!(
            parse_system_names(out),
            ["com.apple.os.update-2A1B", "com.apple.bless.3D4C"]
        );
    }

    #[test]
    fn delete_spec_refuses_bad_dates() {
        let s = delete_spec("2026-11-07-032842").unwrap();
        assert_eq!(s.display(), "tmutil deletelocalsnapshots 2026-11-07-032842");
        assert!(delete_spec("/").is_none());
        assert!(delete_spec("2026-11-07-032842 /").is_none());
    }

    fn write(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; n]).unwrap();
    }

    #[test]
    fn estimate_counts_gone_and_changed_but_not_same_files() {
        let t = tempfile::tempdir().unwrap();
        let (snap, live) = (t.path().join("snap"), t.path().join("live"));
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let stamp = |p: &Path| {
            fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };
        // same in both: counts nothing
        for root in [&snap, &live] {
            write(&root.join("d/same"), 40_000);
            stamp(&root.join("d/same"));
        }
        // only in the snapshot: counts in full
        write(&snap.join("d/gone"), 40_000);
        // rewritten since: its old bytes are held
        write(&snap.join("changed"), 30_000);
        write(&live.join("changed"), 50_000);
        let held = estimate_held(&snap, &live, &AtomicBool::new(false)).unwrap();
        assert!(held >= 70_000, "{held}");
        assert!(
            held < 70_000 + 3 * 4096 * 2,
            "same file must add nothing: {held}"
        );
    }

    #[test]
    fn estimate_stops_when_cancelled() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("snap/a"), 10);
        let stop = AtomicBool::new(true);
        assert_eq!(
            estimate_held(&t.path().join("snap"), &t.path().join("live"), &stop),
            None
        );
    }

    #[test]
    fn find_mount_finds_the_backupdb_layout() {
        let t = tempfile::tempdir().unwrap();
        let vol = t
            .path()
            .join("Kratos/2026-11-07-032842/Macintosh HD - Data");
        fs::create_dir_all(&vol).unwrap();
        assert_eq!(find_mount(t.path(), "2026-11-07-032842"), Some(vol));
        assert_eq!(find_mount(t.path(), "2026-11-06-020608"), None);
        assert_eq!(find_mount(t.path(), "../x"), None);
    }

    #[test]
    fn load_lists_time_machine_then_system_and_reports_failure() {
        let runner = FakeRunner::new(&[]).answering(&[
            (
                "listlocalsnapshotdates",
                "Snapshot dates for disk /:\n2026-11-07-032842",
            ),
            ("listSnapshots", "Name:        com.apple.bless.3D4C"),
        ]);
        let list = load(&runner).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id().as_deref(), Some("snapshot:2026-11-07-032842"));
        assert_eq!(list[1].date, None);
        assert!(list[1].id().is_none());
        assert!(load(&FakeRunner::new(&[])).is_err(), "tmutil silent");
    }

    #[test]
    fn settle_ends_after_stable_free_space() {
        let mut s = Settle::default();
        assert!(!s.active(0), "idle until started");
        s.start(100, 1_000);
        assert!(s.active(100 + STABLE_TICKS - 1));
        s.observe(300, 2_000);
        assert!(
            s.active(300 + STABLE_TICKS - 1),
            "a change restarts the wait"
        );
        assert!(!s.active(300 + STABLE_TICKS));
        let mut t = Settle::default();
        t.start(0, 1);
        for tick in (0..SETTLE_MAX_TICKS).step_by(100) {
            t.observe(tick, tick + 2);
        }
        assert!(!t.active(SETTLE_MAX_TICKS), "never longer than the maximum");
    }
}
