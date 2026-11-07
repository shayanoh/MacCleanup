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

unsafe extern "C" {
    // sys/fsgetpath.h (macOS 10.13+): the path of an inode on a volume.
    fn fsgetpath(
        buf: *mut libc::c_char,
        bufsize: usize,
        fsid: *mut libc::fsid_t,
        obj_id: u64,
    ) -> isize;
}

/// A mounted volume, to find where a file that was renamed or moved lives now.
struct Volume {
    fsid: libc::fsid_t,
    mount: PathBuf,
}

fn volume_of(path: &Path) -> Option<Volume> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statfs` is plain data; the path is a valid C string.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // SAFETY: the kernel NUL-terminates `f_mntonname`.
    let mount = unsafe { std::ffi::CStr::from_ptr(st.f_mntonname.as_ptr()) };
    Some(Volume {
        fsid: st.f_fsid,
        mount: PathBuf::from(std::ffi::OsStr::from_bytes(mount.to_bytes())),
    })
}

/// Where inode `ino` is now on `vol`, or `None` if no file has it any more.
/// A rename keeps the inode, so this tells "moved" from "deleted".
fn relocate(vol: &Volume, ino: u64) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut fsid = vol.fsid;
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: `buf` is writable for its length; `fsid` is a valid fsid_t.
    let n = unsafe { fsgetpath(buf.as_mut_ptr(), buf.len(), &mut fsid, ino) };
    if n <= 0 {
        return None;
    }
    // SAFETY: on success the buffer holds a NUL-terminated path.
    let rel = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    let rel = Path::new(std::ffi::OsStr::from_bytes(rel.to_bytes()));
    Some(vol.mount.join(rel.strip_prefix("/").unwrap_or(rel)))
}

/// What deleting a snapshot would free, as a range in bytes. `low` counts
/// only files that are really gone or rewritten; `high` also counts files
/// that were renamed or moved as if they were deleted. File data only: the
/// file system's own records (directories, extents) are not in either number,
/// so Disk Utility can read a little more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub low: u64,
    pub high: u64,
}

/// What one estimate needs, shared by the walking threads.
struct Estimate<'a> {
    live: &'a Path,
    vol: Option<Volume>,
    /// The other snapshots' folders: what they also hold is not private.
    others: &'a [PathBuf],
    cancel: &'a AtomicBool,
    low: AtomicU64,
    high: AtomicU64,
}

/// What the snapshot at `snap` holds that `live` no longer does, and no
/// other snapshot in `others` holds either (what Disk Utility calls Private
/// Size). A file gone from `live` counts in full; one rewritten counts what
/// the live copy does not still share with it; one that was renamed or moved
/// (same inode elsewhere) counts only in `high`; an unchanged one counts
/// nothing. `None` if `cancel` was set.
pub fn estimate_held(
    snap: &Path,
    live: &Path,
    others: &[PathBuf],
    cancel: &AtomicBool,
) -> Option<Held> {
    let est = Estimate {
        live,
        vol: volume_of(live),
        others,
        cancel,
        low: AtomicU64::new(0),
        high: AtomicU64::new(0),
    };
    rayon::scope(|s| walk(snap, snap.to_path_buf(), &est, s));
    (!cancel.load(Relaxed)).then(|| Held {
        low: est.low.load(Relaxed),
        high: est.high.load(Relaxed),
    })
}

fn walk<'s>(root: &'s Path, dir: PathBuf, est: &'s Estimate<'s>, s: &rayon::Scope<'s>) {
    if est.cancel.load(Relaxed) {
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
            s.spawn(move |s| walk(root, path, est, s));
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let (low, high) = held_by_file(&meta, rel, est);
            est.low.fetch_add(low, Relaxed);
            est.high.fetch_add(high, Relaxed);
        }
    }
}

/// The same file in the same state: inode, size and modification time.
fn same_state(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
}

/// `(low, high)` bytes of one snapshot file that only this snapshot holds.
fn held_by_file(snap: &fs::Metadata, rel: &Path, est: &Estimate) -> (u64, u64) {
    let snap_bytes = snap.blocks() * 512;
    let live_path = est.live.join(rel);
    let (low, high) = match fs::symlink_metadata(&live_path) {
        Ok(live) => {
            let held = rewritten(snap, &live, &live_path);
            (held, held)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Not at the same path: renamed or moved (same inode), or deleted.
            let moved = est
                .vol
                .as_ref()
                .and_then(|v| relocate(v, snap.ino()))
                // A mounted snapshot is its own device: a hit on the same
                // device is the snapshot's own file, not a live one.
                .and_then(|p| {
                    let m = fs::symlink_metadata(&p).ok()?;
                    (m.dev() != snap.dev()).then(|| rewritten(snap, &m, &p))
                });
            match moved {
                Some(held) => (held, snap_bytes),
                None => (snap_bytes, snap_bytes),
            }
        }
        // Any other failure says nothing about the file.
        Err(_) => return (0, 0),
    };
    if (low > 0 || high > 0)
        && est
            .others
            .iter()
            .any(|o| fs::symlink_metadata(o.join(rel)).is_ok_and(|m| same_state(snap, &m)))
    {
        // Another snapshot holds the same data: it is private to neither.
        return (0, 0);
    }
    (low, high)
}

/// What the snapshot holds of a file that still exists live as `live`: 0 if
/// it is unchanged, else its blocks minus those the live copy still shares.
fn rewritten(snap: &fs::Metadata, live: &fs::Metadata, live_path: &Path) -> u64 {
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
    (snap.blocks() * 512).saturating_sub(shared)
}

/// A snapshot mounted for reading: by the system, or by us (`owned`).
struct Mounted {
    path: PathBuf,
    owned: bool,
}

fn mount(runner: &dyn CommandRunner, date: &str) -> Result<Mounted, String> {
    if let Some(path) = find_mount(Path::new(BACKUPDB), date) {
        return Ok(Mounted { path, owned: false });
    }
    let dir = std::env::temp_dir().join(format!("maccleanup-snap-{date}"));
    fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let name = format!("{TM_PREFIX}{date}.local");
    let spec = CommandSpec::new(
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
    if runner.run(&spec, &RunControl::new()).ok {
        Ok(Mounted {
            path: dir,
            owned: true,
        })
    } else {
        let _ = fs::remove_dir(&dir);
        Err("could not mount the snapshot read-only".to_string())
    }
}

fn unmount(runner: &dyn CommandRunner, m: Mounted) {
    if m.owned {
        let spec = CommandSpec::new(DISKUTIL, &["unmount", &m.path.to_string_lossy()]);
        runner.run(&spec, &RunControl::new());
        let _ = fs::remove_dir(&m.path);
    }
}

/// How much deleting the Time Machine snapshot of `date` would free. `all` is
/// every snapshot's date: what another one also holds is not counted. Uses
/// the mounts the system already made, or mounts snapshots read-only itself
/// (no password) and unmounts those afterwards.
pub fn measure(
    runner: &dyn CommandRunner,
    date: &str,
    all: &[String],
    cancel: &AtomicBool,
) -> Result<Held, String> {
    if !valid_date(date) {
        return Err(format!("{date} is not a snapshot date"));
    }
    let target = mount(runner, date)?;
    // The others are best effort: one that cannot be mounted is not compared.
    let others: Vec<Mounted> = all
        .iter()
        .filter(|d| d.as_str() != date && valid_date(d))
        .filter_map(|d| mount(runner, d).ok())
        .collect();
    let paths: Vec<PathBuf> = others.iter().map(|m| m.path.clone()).collect();
    let result = estimate_held(&target.path, Path::new(LIVE_ROOT), &paths, cancel)
        .ok_or_else(|| "cancelled".to_string());
    for m in others {
        unmount(runner, m);
    }
    unmount(runner, target);
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
        let held = estimate_held(&snap, &live, &[], &AtomicBool::new(false))
            .unwrap()
            .low;
        assert!(held >= 70_000, "{held}");
        assert!(
            held < 70_000 + 3 * 4096 * 2,
            "same file must add nothing: {held}"
        );
    }

    #[test]
    fn relocate_finds_a_file_by_inode_and_none_for_a_missing_one() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("kept");
        write(&f, 10);
        let vol = volume_of(t.path()).unwrap();
        let ino = fs::symlink_metadata(&f).unwrap().ino();
        let found = relocate(&vol, ino).unwrap();
        assert_eq!(fs::symlink_metadata(found).unwrap().ino(), ino);
        assert_eq!(relocate(&vol, u64::MAX / 2), None);
    }

    #[test]
    fn what_another_snapshot_also_holds_is_not_private() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, live) = (
            t.path().join("a"),
            t.path().join("b"),
            t.path().join("live"),
        );
        // in both snapshots (the same file), gone live: held by both, private to neither
        write(&a.join("shared"), 40_000);
        fs::create_dir_all(&b).unwrap();
        fs::hard_link(a.join("shared"), b.join("shared")).unwrap();
        // only in a
        write(&a.join("only_a"), 40_000);
        fs::create_dir_all(&live).unwrap();
        let stop = AtomicBool::new(false);
        let alone = estimate_held(&a, &live, &[], &stop).unwrap().low;
        let with_b = estimate_held(&a, &live, std::slice::from_ref(&b), &stop)
            .unwrap()
            .low;
        assert!(alone >= 80_000, "{alone}");
        assert!((40_000..40_000 + 2 * 4096).contains(&with_b), "{with_b}");
    }

    #[test]
    fn estimate_stops_when_cancelled() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("snap/a"), 10);
        let stop = AtomicBool::new(true);
        assert_eq!(
            estimate_held(&t.path().join("snap"), &t.path().join("live"), &[], &stop),
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
