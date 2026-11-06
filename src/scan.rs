use std::cell::RefCell;
use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io::{self, ErrorKind};
use std::iter::Sum;
use std::ops::Add;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::{self, ThreadId};
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};

/// Publish partial totals after this many files so counters keep moving
/// even while scanning one huge folder.
const FLUSH_EVERY: u64 = 4096;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the filesystem says about how a file's data is shared (APFS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneFacts {
    /// Bytes that deleting this file alone would free right now: not shared
    /// with a clone and not held by a Time Machine local snapshot.
    pub private: u64,
    /// Identifies a set of files that are clones of each other.
    pub clone_id: u64,
    /// How many files are in that set (on the whole volume).
    pub refcnt: u32,
}

// getattrlist(2) extended attributes (sys/attr.h). They are requested in
// `forkattr` together with FSOPT_ATTR_CMN_EXTENDED.
const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
const ATTR_CMNEXT_CLONEID: u32 = 0x0000_0100;
const ATTR_CMNEXT_CLONE_REFCNT: u32 = 0x0000_1000;
const FSOPT_NOFOLLOW: libc::c_uint = 0x0000_0001;
const FSOPT_ATTR_CMN_EXTENDED: libc::c_uint = 0x0000_0020;

/// Ask macOS for a file's private size, clone id and clone count. `None` if
/// the call fails (not APFS, file gone, permission): callers then treat the
/// file as plain and unshared.
pub fn clone_facts(path: &Path) -> Option<CloneFacts> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `attrlist` is plain-old-data; zero means "nothing requested".
    let mut request: libc::attrlist = unsafe { std::mem::zeroed() };
    request.bitmapcount = 5; // ATTR_BIT_MAP_COUNT
    request.forkattr = ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID | ATTR_CMNEXT_CLONE_REFCNT;
    let mut buf = [0u8; 64];
    // SAFETY: valid NUL-terminated path, request struct and output buffer.
    let rc = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            (&mut request as *mut libc::attrlist).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            FSOPT_NOFOLLOW | FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    if rc != 0 {
        return None;
    }
    // Packed result: u32 length, u64 private size, u64 clone id, u32 refcount.
    let length = u32::from_ne_bytes(buf[0..4].try_into().ok()?) as usize;
    if length < 24 {
        return None;
    }
    Some(CloneFacts {
        private: u64::from_ne_bytes(buf[4..12].try_into().ok()?),
        clone_id: u64::from_ne_bytes(buf[12..20].try_into().ok()?),
        refcnt: u32::from_ne_bytes(buf[20..24].try_into().ok()?),
    })
}

/// When the newest local snapshot of the startup volume was taken, in Unix
/// seconds: `NO_SNAPSHOT` if there is none, `UNKNOWN_SNAPSHOT` if it could not
/// be found out. Refreshed whenever a scan starts.
static NEWEST_SNAPSHOT: AtomicI64 = AtomicI64::new(UNKNOWN_SNAPSHOT);
const NO_SNAPSHOT: i64 = i64::MIN;
const UNKNOWN_SNAPSHOT: i64 = i64::MAX;

/// `com.apple.TimeMachine.2026-11-06-170157.local` -> Unix seconds (the name
/// is in local time). `None` for any other kind of snapshot name.
fn snapshot_time(name: &str) -> Option<i64> {
    let stamp = name.strip_prefix("com.apple.TimeMachine.")?.get(..17)?;
    let n = |r: std::ops::Range<usize>| stamp.get(r)?.parse::<i32>().ok();
    // SAFETY: `tm` is plain-old-data; mktime only reads and normalizes it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = n(0..4)? - 1900;
    tm.tm_mon = n(5..7)? - 1;
    tm.tm_mday = n(8..10)?;
    tm.tm_hour = n(11..13)?;
    tm.tm_min = n(13..15)?;
    tm.tm_sec = n(15..17)?;
    tm.tm_isdst = -1;
    // SAFETY: as above.
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t)
}

/// Ask `tmutil` (no admin rights needed) when the newest local snapshot was
/// taken, and remember it.
pub fn refresh_local_snapshots() {
    let newest = match std::process::Command::new("/usr/bin/tmutil")
        .args(["listlocalsnapshots", "/"])
        .output()
        .ok()
        .filter(|o| o.status.success())
    {
        None => UNKNOWN_SNAPSHOT,
        Some(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            let names: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("com.apple."))
                .collect();
            if names.is_empty() {
                NO_SNAPSHOT
            } else {
                // A name we cannot read could be newer than all we can.
                let unreadable = names.iter().any(|n| snapshot_time(n).is_none());
                let newest = names.iter().filter_map(|n| snapshot_time(n)).max();
                match (unreadable, newest) {
                    (false, Some(t)) => t,
                    _ => UNKNOWN_SNAPSHOT,
                }
            }
        }
    };
    NEWEST_SNAPSHOT.store(newest, Ordering::Relaxed);
}

/// Could a local snapshot be holding blocks of this unshared file? Only if
/// one exists, and the file already existed when the newest one was taken
/// (a file created later has no blocks in any snapshot). A file with no data
/// fork extents at all (`refcnt == 0`: macOS-compressed files keep their data
/// elsewhere, so their private size reads 0) is never a snapshot's.
fn snapshot_may_hold(created: Option<i64>, refcnt: u32) -> bool {
    may_hold(NEWEST_SNAPSHOT.load(Ordering::Relaxed), created, refcnt)
}

fn may_hold(newest_snapshot: i64, created: Option<i64>, refcnt: u32) -> bool {
    if refcnt == 0 {
        return false;
    }
    match newest_snapshot {
        NO_SNAPSHOT => false,
        UNKNOWN_SNAPSHOT => true,
        newest => created.is_none_or(|c| c <= newest),
    }
}

/// Bytes of an unshared file that a snapshot could be holding: the part of
/// its real data (the logical size, rounded up to blocks) that its private
/// size does not cover. Blocks allocated past the end of the file
/// (preallocation) are never snapshot data; they free when the file goes.
fn held_bytes(alloc: u64, len: u64, private: u64) -> u64 {
    let data = alloc.min(len.div_ceil(4096) * 4096);
    data.saturating_sub(private)
}

/// Birth time of a file as Unix seconds.
fn created_secs(md: &fs::Metadata) -> Option<i64> {
    let d = md.created().ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// Running totals added to a folder and all its ancestors.
#[derive(Clone, Copy, Debug, Default)]
struct Tally {
    size: u64,
    disk: u64,
    files: u64,
    /// Bytes freed right now by deleting these files (their private size).
    private: u64,
    /// Bytes of unshared files that only a local snapshot keeps alive.
    snap: u64,
}

/// A scanned directory. Totals cover its whole subtree and grow live while
/// scanning is in progress. Only directories are kept in memory; files are
/// read on demand by [`Scanner::list_children`].
pub struct Node {
    pub path: PathBuf,
    pub name: OsString,
    dev: u64,
    parent: Weak<Node>,
    size: AtomicU64,
    disk: AtomicU64,
    files: AtomicU64,
    private: AtomicU64,
    snap: AtomicU64,
    /// 1 for this directory's own listing plus one per unfinished child.
    pending: AtomicUsize,
    done: AtomicBool,
    /// Set when a worker begins listing this folder (before that it is queued).
    started: AtomicBool,
    subdirs: Mutex<Vec<Arc<Node>>>,
}

impl Node {
    fn new(path: PathBuf, name: OsString, dev: u64, parent: Weak<Node>) -> Arc<Node> {
        Arc::new(Node {
            path,
            name,
            dev,
            parent,
            size: AtomicU64::new(0),
            disk: AtomicU64::new(0),
            files: AtomicU64::new(0),
            private: AtomicU64::new(0),
            snap: AtomicU64::new(0),
            pending: AtomicUsize::new(1),
            done: AtomicBool::new(false),
            started: AtomicBool::new(false),
            subdirs: Mutex::new(Vec::new()),
        })
    }

    /// Logical bytes in the subtree.
    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// Allocated bytes on disk in the subtree (`blocks * 512`, each file
    /// counted in full, hard links once).
    pub fn disk(&self) -> u64 {
        self.disk.load(Ordering::Relaxed)
    }

    pub fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
    }

    /// Bytes deleting the unshared files here would free right now.
    pub fn private(&self) -> u64 {
        self.private.load(Ordering::Relaxed)
    }

    /// Bytes of unshared files kept alive only by local snapshots.
    pub fn snapshot_held(&self) -> u64 {
        self.snap.load(Ordering::Relaxed)
    }

    /// True once a worker has begun listing this folder.
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    /// True once the whole subtree has been scanned.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Snapshot of the sub-directories discovered so far.
    pub fn children(&self) -> Vec<Arc<Node>> {
        lock(&self.subdirs).clone()
    }

    pub fn subdir(&self, path: &Path) -> Option<Arc<Node>> {
        lock(&self.subdirs).iter().find(|n| n.path == path).cloned()
    }

    fn tally(&self) -> Tally {
        Tally {
            size: self.size(),
            disk: self.disk(),
            files: self.files(),
            private: self.private(),
            snap: self.snapshot_held(),
        }
    }
}

/// Add to a node and every ancestor.
fn add_up(node: &Arc<Node>, t: &Tally) {
    let mut cur = Some(Arc::clone(node));
    while let Some(n) = cur {
        n.size.fetch_add(t.size, Ordering::Relaxed);
        n.disk.fetch_add(t.disk, Ordering::Relaxed);
        n.files.fetch_add(t.files, Ordering::Relaxed);
        n.private.fetch_add(t.private, Ordering::Relaxed);
        n.snap.fetch_add(t.snap, Ordering::Relaxed);
        cur = n.parent.upgrade();
    }
}

/// Subtract from a node and every ancestor (never below zero).
fn sub_up(node: &Arc<Node>, t: &Tally) {
    let sub = |counter: &AtomicU64, by: u64| {
        let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(by))
        });
    };
    let mut cur = Some(Arc::clone(node));
    while let Some(n) = cur {
        sub(&n.size, t.size);
        sub(&n.disk, t.disk);
        sub(&n.files, t.files);
        sub(&n.private, t.private);
        sub(&n.snap, t.snap);
        cur = n.parent.upgrade();
    }
}

/// One unit of pending work finished; mark finished subtrees done upwards.
fn finish_one(node: &Arc<Node>) {
    let mut cur = Some(Arc::clone(node));
    while let Some(n) = cur {
        if n.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            n.done.store(true, Ordering::Release);
            cur = n.parent.upgrade();
        } else {
            break;
        }
    }
}

/// Totals seen so far by the scanner (for live progress counters).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub files: u64,
    /// Logical bytes.
    pub bytes: u64,
    /// Allocated bytes on disk.
    pub disk: u64,
}

/// Where a folder's disk space would go. All numbers are bytes on disk, and
/// `footprint == frees_now + snapshots + shared_inside + shared_outside`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Space {
    /// What the folder's files really occupy: clone data and hard links
    /// counted once.
    pub footprint: u64,
    /// Freed right now if everything here were deleted.
    pub frees_now: u64,
    /// Kept alive only by Time Machine local snapshots; freed when they expire.
    pub snapshots: u64,
    /// Also referenced from outside the folder (a clone or hard link); stays.
    pub shared_outside: u64,
    /// Shared only among files inside the folder; goes when the whole folder
    /// does (unless a snapshot also holds it).
    pub shared_inside: u64,
    /// What clones and hard links inside the folder already save.
    pub saved: u64,
}

impl Space {
    /// What deleting everything in a *set* frees: also the data shared only
    /// among the set's own files (its holders are all being deleted).
    pub fn frees_all(&self) -> u64 {
        self.frees_now.saturating_add(self.shared_inside)
    }
}

impl Add for Space {
    type Output = Space;
    fn add(self, o: Space) -> Space {
        Space {
            footprint: self.footprint.saturating_add(o.footprint),
            frees_now: self.frees_now.saturating_add(o.frees_now),
            snapshots: self.snapshots.saturating_add(o.snapshots),
            shared_outside: self.shared_outside.saturating_add(o.shared_outside),
            shared_inside: self.shared_inside.saturating_add(o.shared_inside),
            saved: self.saved.saturating_add(o.saved),
        }
    }
}

impl Sum for Space {
    fn sum<I: Iterator<Item = Space>>(iter: I) -> Space {
        iter.fold(Space::default(), |a, b| a + b)
    }
}

/// One entry shown in the explorer list.
#[derive(Clone)]
pub struct Row {
    pub name: OsString,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// For a folder with a known [`Space`] this is its footprint; otherwise
    /// the allocation of its files summed.
    pub disk: u64,
    pub files: u64,
    pub scanning: bool,
    pub node: Option<Arc<Node>>,
    /// Clone- and snapshot-aware numbers, known once the scan has finished.
    pub space: Option<Space>,
    /// What deleting this entry frees right now. `None` while a scan runs.
    pub frees: Option<u64>,
}

impl Row {
    /// A folder that is waiting for its turn to be scanned.
    pub fn queued(&self) -> bool {
        self.is_dir && self.scanning && self.node.as_ref().is_none_or(|n| !n.is_started())
    }

    /// Re-read live totals from the node (if the row has one).
    pub fn live(&self) -> Row {
        match &self.node {
            Some(n) => Row {
                size: n.size(),
                // Keep the footprint once it is known.
                disk: if self.space.is_some() {
                    self.disk
                } else {
                    n.disk()
                },
                files: n.files(),
                scanning: !n.is_done(),
                // `frees` is kept as it was (`..self.clone()`).
                ..self.clone()
            },
            None => self.clone(),
        }
    }
}

/// All the links to one hard-linked file that the scanner has seen.
struct LinkGroup {
    /// Total link count reported by the filesystem (may include links
    /// outside the scanned tree).
    nlink: u64,
    size: u64,
    disk: u64,
    /// Bytes of the file that deleting it (all links) would free right now.
    private: u64,
    /// The folder whose totals include this file (the first one scanned).
    counted_by: Option<PathBuf>,
    /// Every link path seen.
    paths: Vec<PathBuf>,
}

/// A set of APFS clones of each other, as far as the scan has seen them.
struct CloneGroup {
    /// Files in the set on the whole volume.
    refcnt: u32,
    /// Bytes of data the members share.
    shared: u64,
    /// The folder holding each member that was scanned.
    members: Vec<Arc<Node>>,
}

const CLONE_SHARDS: u64 = 64;

/// Clone sets, sharded by clone id so scan threads rarely contend.
struct CloneTable {
    shards: Vec<Mutex<HashMap<u64, CloneGroup>>>,
}

impl CloneTable {
    fn new() -> Self {
        Self {
            shards: (0..CLONE_SHARDS)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
        }
    }

    fn shard(&self, id: u64) -> MutexGuard<'_, HashMap<u64, CloneGroup>> {
        lock(&self.shards[(id % CLONE_SHARDS) as usize])
    }

    fn clear(&self) {
        for shard in &self.shards {
            lock(shard).clear();
        }
    }

    /// Forget members inside `prefix` (a folder about to be rescanned).
    fn forget_under(&self, prefix: &Path) {
        for shard in &self.shards {
            lock(shard).retain(|_, group| {
                group.members.retain(|dir| !dir.path.starts_with(prefix));
                !group.members.is_empty()
            });
        }
    }
}

/// Find the scanned folder node for `dir`, walking down from `root`.
pub fn locate(root: &Arc<Node>, dir: &Path) -> Option<Arc<Node>> {
    let rel = dir.strip_prefix(&root.path).ok()?;
    let mut cur = Arc::clone(root);
    for component in rel.components() {
        let next = cur.subdir(&cur.path.join(component))?;
        cur = next;
    }
    Some(cur)
}

/// Space numbers for a single file, asked of the filesystem directly. A
/// hard-linked file frees nothing when one link is removed; a clone's shared
/// data stays while its partners exist.
pub fn file_space(path: &Path) -> io::Result<Space> {
    let md = fs::symlink_metadata(path)?;
    let alloc = md.blocks() * 512;
    let mut space = Space {
        footprint: alloc,
        ..Space::default()
    };
    if md.is_file() && md.nlink() > 1 {
        space.shared_outside = alloc;
    } else if md.is_file() && alloc > 0 {
        match clone_facts(path) {
            Some(f) => {
                let private = f.private.min(alloc);
                space.frees_now = private;
                if f.refcnt > 1 {
                    space.shared_outside = alloc - private;
                } else if snapshot_may_hold(created_secs(&md), f.refcnt) {
                    space.snapshots = held_bytes(alloc, md.len(), private);
                    space.frees_now = alloc - space.snapshots;
                } else {
                    space.frees_now = alloc; // nothing can be holding it
                }
            }
            None => space.frees_now = alloc,
        }
    } else {
        space.frees_now = alloc;
    }
    Ok(space)
}

/// Accumulates one folder's [`Space`] while the clone and link tables are
/// read once for many folders.
#[derive(Default)]
struct Acc {
    disk: u64,
    private: u64,
    snap: u64,
    frees_links: u64,
    snap_links: u64,
    inside: u64,
    outside: u64,
    saved_clones: u64,
    saved_links: u64,
    extra_links: u64,
}

impl Acc {
    fn finish(self) -> Space {
        Space {
            // Hard-linked data credited to another folder is not in `disk`.
            footprint: (self.disk + self.extra_links).saturating_sub(self.saved_clones),
            frees_now: self.private + self.frees_links,
            snapshots: self.snap + self.snap_links,
            shared_outside: self.outside,
            shared_inside: self.inside,
            saved: self.saved_clones + self.saved_links,
        }
    }
}

struct Shared {
    /// Every hard-linked file seen, by (device, inode).
    links: Mutex<HashMap<(u64, u64), LinkGroup>>,
    /// Every APFS clone set with more than one member.
    clones: CloneTable,
    denied: Mutex<Vec<PathBuf>>,
    threads: Mutex<HashSet<ThreadId>>,
    files_seen: AtomicU64,
    bytes_seen: AtomicU64,
    disk_seen: AtomicU64,
    active: AtomicUsize,
}

/// Multi-threaded directory sizing on a dedicated worker pool.
pub struct Scanner {
    pool: rayon::ThreadPool,
    shared: Arc<Shared>,
}

impl Scanner {
    /// Directory walking is I/O-bound, so use twice the core count.
    pub fn default_threads() -> usize {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        (cores * 2).max(2)
    }

    pub fn new(threads: usize) -> Scanner {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .thread_name(|i| format!("scan-{i}"))
            .build()
            .expect("failed to start scanner threads");
        Scanner {
            pool,
            shared: Arc::new(Shared {
                links: Mutex::new(HashMap::new()),
                clones: CloneTable::new(),
                denied: Mutex::new(Vec::new()),
                threads: Mutex::new(HashSet::new()),
                files_seen: AtomicU64::new(0),
                bytes_seen: AtomicU64::new(0),
                disk_seen: AtomicU64::new(0),
                active: AtomicUsize::new(0),
            }),
        }
    }

    /// Start scanning `path` in the background and return its node at once.
    pub fn scan_root(&self, path: &Path) -> Arc<Node> {
        refresh_local_snapshots();
        let dev = fs::metadata(path).map_or(0, |m| m.dev());
        let name = path
            .file_name()
            .map_or_else(|| path.as_os_str().to_os_string(), |n| n.to_os_string());
        let node = Node::new(path.to_path_buf(), name, dev, Weak::new());
        self.shared.active.fetch_add(1, Ordering::AcqRel);
        let shared = Arc::clone(&self.shared);
        let job = Arc::clone(&node);
        self.pool.spawn(move || walk(shared, job));
        node
    }

    /// List one directory's entries. Sub-directories carry their live node
    /// (or `node: None, scanning: true` if not yet discovered); files are
    /// stat'ed directly. Once the whole scan is done, folder rows also carry
    /// their clone- and snapshot-aware [`Space`], worked out for all of them
    /// in one pass.
    pub fn list_children(&self, dir: &Arc<Node>) -> Vec<Row> {
        let Ok(rd) = fs::read_dir(&dir.path) else {
            return Vec::new();
        };
        let known: HashMap<PathBuf, Arc<Node>> = lock(&dir.subdirs)
            .iter()
            .map(|n| (n.path.clone(), Arc::clone(n)))
            .collect();
        let mut rows = Vec::new();
        for entry in rd.flatten() {
            let Ok(md) = entry.metadata() else { continue };
            let path = entry.path();
            let name = entry.file_name();
            if md.is_dir() {
                match known.get(&path) {
                    Some(n) => rows.push(Row {
                        name,
                        path,
                        is_dir: true,
                        size: n.size(),
                        disk: n.disk(),
                        files: n.files(),
                        scanning: !n.is_done(),
                        node: Some(Arc::clone(n)),
                        space: None,
                        frees: None,
                    }),
                    None => rows.push(Row {
                        name,
                        path,
                        is_dir: true,
                        size: 0,
                        disk: 0,
                        files: 0,
                        // A different device (mount point) is never scanned.
                        scanning: md.dev() == dir.dev,
                        node: None,
                        space: None,
                        frees: None,
                    }),
                }
            } else {
                rows.push(Row {
                    name,
                    path,
                    is_dir: false,
                    size: md.len(),
                    disk: md.blocks() * 512,
                    files: 1,
                    scanning: false,
                    node: None,
                    space: None,
                    frees: None,
                });
            }
        }
        if self.is_idle() {
            self.fill_spaces(dir, &mut rows);
            self.fill_frees(&mut rows);
        }
        rows
    }

    /// Give every folder row its [`Space`] and make its `disk` the footprint.
    fn fill_spaces(&self, dir: &Arc<Node>, rows: &mut [Row]) {
        let nodes: Vec<Arc<Node>> = rows.iter().filter_map(|r| r.node.clone()).collect();
        if nodes.is_empty() {
            return;
        }
        let index: HashMap<&OsStr, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.as_os_str(), i))
            .collect();
        // A path belongs to the child folder named by its first component
        // below `dir`; files directly in `dir` belong to no child.
        let locate = |p: &Path| -> Option<usize> {
            let rel = p.strip_prefix(&dir.path).ok()?;
            let first = rel.components().next()?;
            index.get(first.as_os_str()).copied()
        };
        let mut spaces = self.compute(&nodes, &locate).into_iter();
        for row in rows.iter_mut().filter(|r| r.node.is_some()) {
            if let Some(space) = spaces.next() {
                row.disk = space.footprint;
                row.space = Some(space);
            }
        }
    }

    /// Work out what deleting each row frees now (files are asked in parallel).
    fn fill_frees(&self, rows: &mut [Row]) {
        use rayon::prelude::*;
        rows.par_iter_mut().for_each(|r| {
            r.frees = Some(match (&r.space, r.is_dir) {
                (Some(sp), _) => sp.frees_now,
                (None, true) => 0,
                (None, false) => file_space(&r.path).map_or(r.disk, |sp| sp.frees_now),
            });
        });
    }

    /// Where one folder's disk space would go: what deleting it frees now,
    /// what only snapshots hold, and what is shared by clone or hard link.
    /// Only complete once the whole scan has finished.
    pub fn space(&self, node: &Arc<Node>) -> Space {
        let locate = |p: &Path| p.starts_with(&node.path).then_some(0);
        self.compute(std::slice::from_ref(node), &locate)
            .pop()
            .unwrap_or_default()
    }

    /// Read the clone and hard-link tables once and work out the [`Space`]
    /// of every node. `locate` says which node (by index) a path belongs to.
    fn compute(&self, nodes: &[Arc<Node>], locate: &dyn Fn(&Path) -> Option<usize>) -> Vec<Space> {
        compute_with(&self.shared, seeds(nodes), locate)
    }

    /// A cheap handle that can work out set-wise space on another thread.
    pub fn view(&self) -> SpaceView {
        SpaceView {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Where the space of a whole set of (disjoint) folders would go if they
    /// were all deleted together: a clone set frees only when every holder
    /// is inside the set.
    pub fn space_of_set(&self, nodes: &[Arc<Node>]) -> Space {
        self.view().space_of_set(nodes)
    }

    /// [`Scanner::space`] for many disjoint folders in one pass.
    pub fn spaces_of(&self, nodes: &[Arc<Node>]) -> Vec<Space> {
        self.view().spaces_of(nodes)
    }

    /// Drop the hard-link entries inside a rescanned folder. A file that this
    /// folder was credited with but that also has a link elsewhere is handed
    /// to the folder holding that other link, so it is neither lost from the
    /// totals nor counted twice.
    fn forget_links_under(&self, node: &Arc<Node>, parent: &Arc<Node>) {
        let mut handoffs: Vec<(PathBuf, u64, u64)> = Vec::new();
        lock(&self.shared.links).retain(|_, group| {
            let credited_inside = group
                .counted_by
                .as_deref()
                .is_some_and(|dir| dir.starts_with(&node.path));
            group.paths.retain(|p| !p.starts_with(&node.path));
            if credited_inside {
                group.counted_by = None;
                if let Some(dir) = group.paths.first().and_then(|p| p.parent()) {
                    group.counted_by = Some(dir.to_path_buf());
                    handoffs.push((dir.to_path_buf(), group.size, group.disk));
                }
            }
            !group.paths.is_empty()
        });
        if handoffs.is_empty() {
            return;
        }
        let mut top = Arc::clone(parent);
        while let Some(up) = top.parent.upgrade() {
            top = up;
        }
        for (dir, size, disk) in handoffs {
            if let Some(owner) = locate(&top, &dir) {
                let t = Tally {
                    size,
                    disk,
                    files: 1,
                    ..Tally::default()
                };
                add_up(&owner, &t);
                self.shared.files_seen.fetch_add(1, Ordering::Relaxed);
                self.shared.bytes_seen.fetch_add(size, Ordering::Relaxed);
                self.shared.disk_seen.fetch_add(disk, Ordering::Relaxed);
            }
        }
    }

    /// Take a subtree's bytes out of every total: the ancestors' counters,
    /// the scan counters, the link and clone tables and the denied list.
    fn forget_totals(&self, node: &Arc<Node>, parent: &Arc<Node>, old: &Tally) {
        sub_up(parent, old);
        for (counter, by) in [
            (&self.shared.files_seen, old.files),
            (&self.shared.bytes_seen, old.size),
            (&self.shared.disk_seen, old.disk),
        ] {
            let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(by))
            });
        }
        self.forget_links_under(node, parent);
        self.shared.clones.forget_under(&node.path);
        lock(&self.shared.denied).retain(|p| !p.starts_with(&node.path));
    }

    /// A folder that no longer exists (it was moved away or deleted): drop it
    /// from the tree and from every total, without scanning anything. Returns
    /// false while a scan runs, and for the root (it has no parent to leave).
    pub fn forget_dir(&self, node: &Arc<Node>) -> bool {
        if !self.is_idle() {
            return false;
        }
        let Some(parent) = node.parent.upgrade() else {
            return false;
        };
        self.forget_totals(node, &parent, &node.tally());
        lock(&parent.subdirs).retain(|n| n.path != node.path);
        true
    }

    /// Start over: scan `path` again from scratch with fresh counters.
    /// Returns `None` while a scan is still running.
    pub fn rescan_all(&self, path: &Path) -> Option<Arc<Node>> {
        if !self.is_idle() {
            return None;
        }
        lock(&self.shared.links).clear();
        self.shared.clones.clear();
        lock(&self.shared.denied).clear();
        for counter in [
            &self.shared.files_seen,
            &self.shared.bytes_seen,
            &self.shared.disk_seen,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
        Some(self.scan_root(path))
    }

    /// Scan one folder again and swap the new subtree into the tree, fixing
    /// the totals above it. Returns the new node, or `None` while a scan is
    /// still running. Rescanning the root is a full rescan.
    pub fn rescan_dir(&self, node: &Arc<Node>) -> Option<Arc<Node>> {
        if !self.is_idle() {
            return None;
        }
        let Some(parent) = node.parent.upgrade() else {
            return self.rescan_all(&node.path);
        };
        refresh_local_snapshots();
        let old = node.tally();
        self.forget_totals(node, &parent, &old);

        // Swap in a fresh node and mark every ancestor unfinished until the
        // new subtree completes.
        let fresh = Node::new(
            node.path.clone(),
            node.name.clone(),
            node.dev,
            Arc::downgrade(&parent),
        );
        {
            let mut subdirs = lock(&parent.subdirs);
            subdirs.retain(|n| n.path != node.path);
            subdirs.push(Arc::clone(&fresh));
        }
        let mut cur = Some(parent);
        while let Some(a) = cur {
            a.pending.fetch_add(1, Ordering::AcqRel);
            a.done.store(false, Ordering::Release);
            cur = a.parent.upgrade();
        }
        self.shared.active.fetch_add(1, Ordering::AcqRel);
        let shared = Arc::clone(&self.shared);
        let job = Arc::clone(&fresh);
        self.pool.spawn(move || walk(shared, job));
        Some(fresh)
    }

    /// Test hook: pretend a scan is (or is no longer) running.
    #[cfg(test)]
    pub(crate) fn set_busy(&self, busy: bool) {
        if busy {
            self.shared.active.fetch_add(1, Ordering::AcqRel);
        } else {
            self.shared.active.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.shared.active.load(Ordering::Acquire) == 0
    }

    /// Paths that could not be read because of missing permission.
    pub fn denied(&self) -> Vec<PathBuf> {
        lock(&self.shared.denied).clone()
    }

    /// Files, bytes and on-disk bytes seen so far across all scans.
    pub fn stats(&self) -> ScanStats {
        ScanStats {
            files: self.shared.files_seen.load(Ordering::Relaxed),
            bytes: self.shared.bytes_seen.load(Ordering::Relaxed),
            disk: self.shared.disk_seen.load(Ordering::Relaxed),
        }
    }

    /// Number of distinct worker threads that have run a directory job.
    pub fn thread_ids_seen(&self) -> usize {
        lock(&self.shared.threads).len()
    }

    /// Block until the scanner is idle or the timeout passes (used by tests).
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while !self.is_idle() {
            if start.elapsed() >= timeout {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

/// Starting numbers for each node (what its own plain files add up to).
fn seeds(nodes: &[Arc<Node>]) -> Vec<Acc> {
    nodes
        .iter()
        .map(|n| Acc {
            disk: n.disk(),
            private: n.private(),
            snap: n.snapshot_held(),
            ..Acc::default()
        })
        .collect()
}

/// Read the clone and hard-link tables once and finish every seed.
fn compute_with(
    shared: &Shared,
    mut acc: Vec<Acc>,
    locate: &dyn Fn(&Path) -> Option<usize>,
) -> Vec<Space> {
    let mut counts: HashMap<usize, u64> = HashMap::new();
    for shard in &shared.clones.shards {
        for group in lock(shard).values() {
            counts.clear();
            for dir in &group.members {
                if let Some(i) = locate(&dir.path) {
                    *counts.entry(i).or_default() += 1;
                }
            }
            for (&i, &members) in &counts {
                let a = &mut acc[i];
                if members >= u64::from(group.refcnt) {
                    a.inside += group.shared;
                } else {
                    a.outside += group.shared;
                }
                a.saved_clones += group.shared.saturating_mul(members - 1);
            }
        }
    }
    for group in lock(&shared.links).values() {
        counts.clear();
        for path in &group.paths {
            if let Some(i) = locate(path) {
                *counts.entry(i).or_default() += 1;
            }
        }
        let credited = group.counted_by.as_deref().and_then(locate);
        for (&i, &links) in &counts {
            let a = &mut acc[i];
            if credited != Some(i) {
                a.extra_links += group.disk;
            }
            if links >= group.nlink {
                a.frees_links += group.private;
                a.snap_links += group.disk - group.private;
            } else {
                a.outside += group.disk;
            }
            a.saved_links += group.disk.saturating_mul(links - 1);
        }
    }
    acc.into_iter().map(Acc::finish).collect()
}

/// Set-wise space, usable from any thread while the scanner lives on.
#[derive(Clone)]
pub struct SpaceView {
    shared: Arc<Shared>,
}

impl SpaceView {
    /// See [`Scanner::space_of_set`].
    pub fn space_of_set(&self, nodes: &[Arc<Node>]) -> Space {
        if nodes.is_empty() {
            return Space::default();
        }
        let mut seed = Acc::default();
        for s in seeds(nodes) {
            seed.disk += s.disk;
            seed.private += s.private;
            seed.snap += s.snap;
        }
        let roots: HashSet<&Path> = nodes.iter().map(|n| n.path.as_path()).collect();
        // Clone members repeat the same few folders very often: remember
        // the answer per folder instead of walking its ancestors each time.
        let memo: RefCell<HashMap<PathBuf, Option<usize>>> = RefCell::new(HashMap::new());
        let locate = |p: &Path| {
            if let Some(known) = memo.borrow().get(p) {
                return *known;
            }
            let found = p.ancestors().any(|a| roots.contains(a)).then_some(0);
            memo.borrow_mut().insert(p.to_path_buf(), found);
            found
        };
        compute_with(&self.shared, vec![seed], &locate)
            .pop()
            .unwrap_or_default()
    }

    /// Other folders that hold a clone or a hard link of data inside `node`
    /// (the folders themselves, not the files), sorted.
    pub fn sharers(&self, node: &Arc<Node>) -> Vec<PathBuf> {
        let inside = |p: &Path| p.starts_with(&node.path);
        let mut out: HashSet<PathBuf> = HashSet::new();
        for shard in &self.shared.clones.shards {
            for group in lock(shard).values() {
                if group.members.iter().any(|m| inside(&m.path)) {
                    out.extend(
                        group
                            .members
                            .iter()
                            .filter(|m| !inside(&m.path))
                            .map(|m| m.path.clone()),
                    );
                }
            }
        }
        for group in lock(&self.shared.links).values() {
            if group.paths.iter().any(|p| inside(p)) {
                out.extend(
                    group
                        .paths
                        .iter()
                        .filter(|p| !inside(p))
                        .filter_map(|p| p.parent().map(Path::to_path_buf)),
                );
            }
        }
        let mut v: Vec<PathBuf> = out.into_iter().collect();
        v.sort();
        v
    }

    /// See [`Scanner::spaces_of`].
    pub fn spaces_of(&self, nodes: &[Arc<Node>]) -> Vec<Space> {
        let index: HashMap<&Path, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.path.as_path(), i))
            .collect();
        let memo: RefCell<HashMap<PathBuf, Option<usize>>> = RefCell::new(HashMap::new());
        let locate = |p: &Path| {
            if let Some(known) = memo.borrow().get(p) {
                return *known;
            }
            let found = p.ancestors().find_map(|a| index.get(a).copied());
            memo.borrow_mut().insert(p.to_path_buf(), found);
            found
        };
        compute_with(&self.shared, seeds(nodes), &locate)
    }
}

fn walk(shared: Arc<Shared>, node: Arc<Node>) {
    lock(&shared.threads).insert(thread::current().id());
    node.started.store(true, Ordering::Release);
    scan_dir(&shared, &node);
    finish_one(&node);
    shared.active.fetch_sub(1, Ordering::AcqRel);
}

fn scan_dir(shared: &Arc<Shared>, node: &Arc<Node>) {
    let rd = match fs::read_dir(&node.path) {
        Ok(rd) => rd,
        Err(e) => {
            if e.kind() == ErrorKind::PermissionDenied {
                lock(&shared.denied).push(node.path.clone());
            }
            return;
        }
    };
    let mut tally = Tally::default();
    let flush = |t: &mut Tally| {
        if t.files == 0 && t.size == 0 {
            return;
        }
        add_up(node, t);
        shared.files_seen.fetch_add(t.files, Ordering::Relaxed);
        shared.bytes_seen.fetch_add(t.size, Ordering::Relaxed);
        shared.disk_seen.fetch_add(t.disk, Ordering::Relaxed);
        *t = Tally::default();
    };
    for entry in rd {
        let Ok(entry) = entry else { continue };
        let Ok(md) = entry.metadata() else { continue };
        if md.is_dir() {
            if md.dev() != node.dev {
                continue; // never cross onto another volume
            }
            let child = Node::new(
                entry.path(),
                entry.file_name(),
                node.dev,
                Arc::downgrade(node),
            );
            node.pending.fetch_add(1, Ordering::AcqRel);
            lock(&node.subdirs).push(Arc::clone(&child));
            shared.active.fetch_add(1, Ordering::AcqRel);
            let sh = Arc::clone(shared);
            rayon::spawn(move || walk(sh, child));
        } else {
            let alloc = md.blocks() * 512;
            let is_file = md.is_file();
            let (mut private, mut snap) = (0u64, 0u64);
            if is_file && md.nlink() > 1 {
                // Hard-linked: count the data once, and keep every link so
                // folders can tell shared from unique. Whether it frees
                // anything depends on the folder, so no private/snapshot
                // numbers go into the running totals here.
                let key = (md.dev(), md.ino());
                let path = entry.path();
                let first_sight = match lock(&shared.links).get_mut(&key) {
                    Some(group) => {
                        group.paths.push(path.clone());
                        false
                    }
                    None => true,
                };
                if !first_sight {
                    continue; // hard link already counted
                }
                let private_part = if alloc > 0 {
                    clone_facts(&path).map_or(alloc, |f| f.private.min(alloc))
                } else {
                    0
                };
                match lock(&shared.links).entry(key) {
                    Entry::Occupied(mut e) => {
                        e.get_mut().paths.push(path);
                        continue; // another thread got there first
                    }
                    Entry::Vacant(v) => {
                        v.insert(LinkGroup {
                            nlink: md.nlink(),
                            size: md.len(),
                            disk: alloc,
                            private: private_part,
                            counted_by: Some(node.path.clone()),
                            paths: vec![path],
                        });
                    }
                }
            } else if is_file && alloc > 0 {
                match clone_facts(&entry.path()) {
                    Some(f) => {
                        private = f.private.min(alloc);
                        let shared_part = alloc - private;
                        if f.refcnt > 1 && shared_part > 0 {
                            let mut shard = shared.clones.shard(f.clone_id);
                            let group = shard.entry(f.clone_id).or_insert_with(|| CloneGroup {
                                refcnt: f.refcnt,
                                shared: 0,
                                members: Vec::new(),
                            });
                            group.refcnt = group.refcnt.max(f.refcnt);
                            group.shared = group.shared.max(shared_part);
                            group.members.push(Arc::clone(node));
                        } else {
                            // Not cloned, yet not fully private: a local
                            // snapshot holds these blocks (if there is none,
                            // the shortfall is not snapshot data: it frees).
                            if snapshot_may_hold(created_secs(&md), f.refcnt) {
                                snap = held_bytes(alloc, md.len(), private);
                            }
                            private = alloc - snap;
                        }
                    }
                    None => private = alloc,
                }
            } else {
                private = alloc; // empty files, symlinks and the like
            }
            tally.size += md.len();
            tally.disk += alloc;
            tally.files += 1;
            tally.private += private;
            tally.snap += snap;
            if tally.files >= FLUSH_EVERY {
                flush(&mut tally);
            }
        }
    }
    flush(&mut tally);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(20);

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn scan(root: &Path, threads: usize) -> (Scanner, Arc<Node>) {
        let scanner = Scanner::new(threads);
        let node = scanner.scan_root(root);
        assert!(scanner.wait_idle(WAIT), "scan did not finish");
        (scanner, node)
    }

    #[test]
    fn sizes_sum_up() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/f1"), 1000);
        write(&r.join("a/b/f2"), 2000);
        write(&r.join("c/f3"), 500);
        let (_s, root) = scan(r, 4);
        assert_eq!(root.size(), 3500);
        assert_eq!(root.files(), 3);
        assert!(root.is_done());
        let a = root.subdir(&r.join("a")).unwrap();
        assert_eq!(a.size(), 3000);
        assert_eq!(a.files(), 2);
    }

    #[test]
    fn disk_is_block_multiple() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f1"), 1000);
        write(&dir.path().join("a/f2"), 5000);
        let (_s, root) = scan(dir.path(), 2);
        assert_eq!(root.disk() % 512, 0);
        assert!(root.disk() >= root.size());
    }

    #[test]
    fn hardlink_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("one"), 4000);
        fs::hard_link(dir.path().join("one"), dir.path().join("two")).unwrap();
        let (_s, root) = scan(dir.path(), 2);
        assert_eq!(root.size(), 4000);
        assert_eq!(root.files(), 1);
    }

    #[test]
    fn symlink_not_followed() {
        let outer = tempfile::tempdir().unwrap();
        let root_path = outer.path().join("root");
        let outside = outer.path().join("outside");
        write(&outside.join("big"), 9000);
        fs::create_dir_all(&root_path).unwrap();
        write(&root_path.join("small"), 10);
        symlink(&outside, root_path.join("link")).unwrap();
        let (_s, root) = scan(&root_path, 2);
        assert!(root.size() < 9000, "followed the symlink: {}", root.size());
        assert!(root.size() >= 10);
    }

    #[test]
    fn list_children_rows() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("d/inner"), 700);
        write(&r.join("file.bin"), 300);
        let (s, root) = scan(r, 2);
        let mut rows = s.list_children(&root);
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(rows.len(), 2);
        let d = &rows[0];
        assert_eq!(d.name, "d");
        assert!(d.is_dir);
        let d = d.live();
        assert!(d.node.is_some());
        assert!(!d.scanning);
        assert_eq!(d.size, 700);
        assert_eq!(d.files, 1);
        let f = &rows[1];
        assert_eq!(f.name, "file.bin");
        assert!(!f.is_dir);
        assert_eq!(f.size, 300);
        assert!(f.disk >= 300);
    }

    #[test]
    fn empty_dir_ok() {
        let dir = tempfile::tempdir().unwrap();
        let (s, root) = scan(dir.path(), 2);
        assert_eq!(root.size(), 0);
        assert!(root.is_done());
        assert!(s.list_children(&root).is_empty());
    }

    struct RestorePerms(PathBuf);
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn unreadable_dir_recorded() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root can read everything
        }
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        write(&locked.join("secret"), 100);
        write(&dir.path().join("open/f"), 50);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let _guard = RestorePerms(locked.clone());
        let (s, root) = scan(dir.path(), 2);
        assert!(root.is_done());
        assert!(s.denied().contains(&locked), "{:?}", s.denied());
        assert_eq!(root.size(), 50);
    }

    #[test]
    fn non_utf8_name_ok() {
        let dir = tempfile::tempdir().unwrap();
        let name = std::ffi::OsStr::from_bytes(&[0x66, 0x80, 0x6f]);
        if fs::write(dir.path().join(name), b"hello").is_err() {
            eprintln!("skipping: filesystem rejects non-UTF-8 names");
            return;
        }
        let (s, root) = scan(dir.path(), 2);
        assert_eq!(root.size(), 5);
        let rows = s.list_children(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name.as_bytes(), name.as_bytes());
    }

    #[test]
    fn vanished_dir_ok() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f"), 10);
        let (s, root) = scan(dir.path(), 2);
        let a = root.subdir(&dir.path().join("a")).unwrap();
        fs::remove_dir_all(dir.path().join("a")).unwrap();
        assert!(s.list_children(&a).is_empty());
    }

    #[test]
    fn idle_flag() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f"), 10);
        let s = Scanner::new(2);
        let _root = s.scan_root(dir.path());
        assert!(s.wait_idle(WAIT));
        assert!(s.is_idle());
    }

    fn many_dirs(root: &Path) {
        for i in 0..8 {
            for j in 0..8 {
                for k in 0..60 {
                    write(&root.join(format!("d{i}/e{j}/f{k}")), 100 + k);
                }
            }
        }
    }

    #[test]
    fn uses_multiple_threads() {
        let dir = tempfile::tempdir().unwrap();
        many_dirs(dir.path());
        let (s4, root4) = scan(dir.path(), 4);
        assert!(s4.thread_ids_seen() > 1, "only one worker thread ran");
        let (_s1, root1) = scan(dir.path(), 1);
        assert_eq!(root4.size(), root1.size());
        assert_eq!(root4.disk(), root1.disk());
        assert_eq!(root4.files(), root1.files());
        assert_eq!(root4.files(), 8 * 8 * 60);
    }

    #[test]
    fn default_threads_is_at_least_two() {
        assert!(Scanner::default_threads() >= 2);
    }

    #[test]
    fn stats_include_files_bytes_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f1"), 1000);
        write(&dir.path().join("a/b/f2"), 5000);
        let (s, root) = scan(dir.path(), 2);
        let stats = s.stats();
        assert_eq!(stats.files, 2);
        assert_eq!(stats.bytes, 6000);
        assert_eq!(stats.disk, root.disk());
        assert!(stats.disk >= 6000);
    }

    fn wait(s: &Scanner) {
        assert!(s.wait_idle(WAIT), "scan did not finish");
    }

    #[test]
    fn rescan_all_picks_up_changes() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/f"), 1000);
        let (s, old) = scan(r, 2);
        assert_eq!(old.size(), 1000);

        write(&r.join("b/g"), 500);
        let root = s.rescan_all(r).expect("idle scanner can rescan");
        wait(&s);
        assert_eq!(root.size(), 1500);
        assert_eq!(root.files(), 2);
        assert!(root.is_done());
        let stats = s.stats();
        assert_eq!((stats.files, stats.bytes), (2, 1500), "counters restart");
        assert_eq!(old.size(), 1000, "the old tree is untouched");
    }

    #[test]
    fn rescan_dir_replaces_subtree_and_fixes_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/x"), 1000);
        write(&r.join("a/b/y"), 2000);
        write(&r.join("c/z"), 500);
        let (s, root) = scan(r, 2);
        assert_eq!(root.size(), 3500);
        let a = root.subdir(&r.join("a")).unwrap();
        assert!(a.is_done());

        write(&r.join("a/new"), 700);
        fs::remove_file(r.join("a/b/y")).unwrap();
        let fresh = s.rescan_dir(&a).expect("idle scanner can rescan a folder");
        wait(&s);

        assert_eq!(fresh.size(), 1700);
        assert_eq!(fresh.files(), 2);
        assert!(fresh.is_done());
        assert_eq!(root.size(), 2200, "ancestors follow the change");
        assert_eq!(root.files(), 3);
        assert!(root.is_done(), "ancestors are done again after the rescan");
        let again = root.subdir(&r.join("a")).unwrap();
        assert!(
            Arc::ptr_eq(&again, &fresh),
            "the tree now holds the new node"
        );
        let stats = s.stats();
        assert_eq!(
            (stats.files, stats.bytes),
            (3, 2200),
            "counters stay consistent"
        );
    }

    #[test]
    fn rescan_dir_while_ancestors_unfinished_is_refused_until_idle() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/x"), 10);
        let (s, root) = scan(dir.path(), 2);
        let a = root.subdir(&dir.path().join("a")).unwrap();
        s.set_busy(true);
        assert!(s.rescan_dir(&a).is_none());
        assert!(s.rescan_all(dir.path()).is_none());
        s.set_busy(false);
        assert!(s.rescan_dir(&a).is_some());
        wait(&s);
    }

    #[test]
    fn rescan_dir_keeps_hard_links_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/one"), 4000);
        fs::create_dir_all(r.join("c")).unwrap();
        fs::hard_link(r.join("a/one"), r.join("c/two")).unwrap();
        let (s, root) = scan(r, 2);
        assert_eq!(root.size(), 4000);
        // Whichever folder counted the shared file first, rescanning `a`
        // must neither lose it nor count it twice.
        let a = root.subdir(&r.join("a")).unwrap();
        s.rescan_dir(&a).unwrap();
        wait(&s);
        assert_eq!(root.size(), 4000);
        assert_eq!(root.files(), 1);
        let c = root.subdir(&r.join("c")).unwrap();
        s.rescan_dir(&c).unwrap();
        wait(&s);
        assert_eq!(root.size(), 4000);
        assert_eq!(root.files(), 1);
    }

    #[test]
    fn rescan_dir_of_the_root_is_a_full_rescan() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f"), 100);
        let (s, root) = scan(dir.path(), 2);
        write(&dir.path().join("b/g"), 50);
        let fresh = s.rescan_dir(&root).unwrap();
        wait(&s);
        assert_eq!(fresh.size(), 150);
        assert!(!Arc::ptr_eq(&fresh, &root));
    }

    #[test]
    fn rescan_dir_of_a_vanished_folder_removes_its_totals() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/x"), 1000);
        write(&r.join("c/z"), 500);
        let (s, root) = scan(r, 2);
        let a = root.subdir(&r.join("a")).unwrap();
        fs::remove_dir_all(r.join("a")).unwrap();
        let fresh = s.rescan_dir(&a).unwrap();
        wait(&s);
        assert_eq!(fresh.size(), 0);
        assert_eq!(root.size(), 500);
        assert!(root.is_done());
    }

    #[test]
    fn rescan_dir_forgets_denied_paths_inside_it() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let locked = r.join("a/locked");
        write(&locked.join("secret"), 10);
        write(&r.join("a/ok"), 10);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let _guard = RestorePerms(locked.clone());
        let (s, root) = scan(r, 2);
        assert!(s.denied().contains(&locked));
        // Fix the permissions, then rescan the folder: no longer denied.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let a = root.subdir(&r.join("a")).unwrap();
        s.rescan_dir(&a).unwrap();
        wait(&s);
        assert!(s.denied().is_empty(), "{:?}", s.denied());
        assert_eq!(root.size(), 20);
    }

    fn link(root: &Path, from: &str, to: &str) {
        fs::create_dir_all(root.join(to).parent().unwrap()).unwrap();
        fs::hard_link(root.join(from), root.join(to)).unwrap();
    }

    #[test]
    fn rescan_dir_hands_a_shared_file_to_the_other_folder() {
        // Delete the link inside the rescanned folder. If that folder was the
        // one credited with the file, the credit must move to the other
        // folder, or the root total would silently lose a file that is still
        // on disk. Either folder may have been credited, so try both.
        for victim in ["a", "b"] {
            let dir = tempfile::tempdir().unwrap();
            let r = dir.path();
            write(&r.join("a/f"), 8192);
            link(r, "a/f", "b/f");
            let (s, root) = scan(r, 2);
            assert_eq!(root.size(), 8192);

            let node = root.subdir(&r.join(victim)).unwrap();
            fs::remove_file(r.join(victim).join("f")).unwrap();
            s.rescan_dir(&node).unwrap();
            wait(&s);

            assert_eq!(
                root.size(),
                8192,
                "victim {victim}: the file is still on disk"
            );
            assert_eq!(root.files(), 1);
            let a = root.subdir(&r.join("a")).unwrap();
            let b = root.subdir(&r.join("b")).unwrap();
            assert_eq!(a.size() + b.size(), 8192, "counted exactly once");
            assert_eq!(s.stats().files, 1);
        }
    }

    /// `cp -c`: an APFS clone. Needs the temp dir to be on APFS (it is on macOS).
    fn clone_file(root: &Path, from: &str, to: &str) {
        fs::create_dir_all(root.join(to).parent().unwrap()).unwrap();
        let ok = std::process::Command::new("cp")
            .arg("-c")
            .arg(root.join(from))
            .arg(root.join(to))
            .status()
            .unwrap()
            .success();
        assert!(ok, "cp -c failed: these tests need an APFS volume");
    }

    fn hard_link(root: &Path, from: &str, to: &str) {
        fs::create_dir_all(root.join(to).parent().unwrap()).unwrap();
        fs::hard_link(root.join(from), root.join(to)).unwrap();
    }

    fn identity_holds(sp: &Space) -> bool {
        sp.footprint == sp.frees_now + sp.snapshots + sp.shared_inside + sp.shared_outside
    }

    /// store/f with clones in a/f and b/f: three holders of one block.
    fn three_holders(r: &Path) -> (Scanner, Arc<Node>) {
        write(&r.join("store/f"), 200_000);
        clone_file(r, "store/f", "a/f");
        clone_file(r, "store/f", "b/f");
        scan(r, 2)
    }

    #[test]
    fn set_frees_clone_data_only_when_every_holder_is_in_the_set() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let (s, root) = three_holders(r);
        let node = |n: &str| root.subdir(&r.join(n)).unwrap();
        let (store, a, b) = (node("store"), node("a"), node("b"));
        assert_eq!(s.space_of_set(std::slice::from_ref(&a)).frees_all(), 0);
        assert_eq!(s.space_of_set(&[a.clone(), b.clone()]).frees_all(), 0);
        assert_eq!(s.space_of_set(&[store.clone(), a.clone()]).frees_all(), 0);
        let all = s.space_of_set(&[store, a, b]);
        assert!(all.frees_all() >= 200_000, "{all:?}");
        assert_eq!(all.shared_outside, 0);
    }

    #[test]
    fn set_space_identity_holds() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let (s, root) = three_holders(r);
        let store = root.subdir(&r.join("store")).unwrap();
        let a = root.subdir(&r.join("a")).unwrap();
        assert!(identity_holds(&s.space_of_set(&[store, a])));
    }

    #[test]
    fn spaces_of_matches_space_for_disjoint_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let (s, root) = three_holders(r);
        let a = root.subdir(&r.join("a")).unwrap();
        let b = root.subdir(&r.join("b")).unwrap();
        let many = s.spaces_of(&[a.clone(), b.clone()]);
        assert_eq!(many, vec![s.space(&a), s.space(&b)]);
    }

    #[test]
    fn a_set_of_one_node_equals_space() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let (s, root) = three_holders(r);
        let a = root.subdir(&r.join("a")).unwrap();
        assert_eq!(s.space_of_set(std::slice::from_ref(&a)), s.space(&a));
    }

    #[test]
    fn sharers_lists_other_folders_holding_the_same_clone_or_link() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("store/f"), 200_000);
        clone_file(r, "store/f", "a/f");
        clone_file(r, "store/f", "b/f");
        write(&r.join("a/x"), 8192);
        hard_link(r, "a/x", "linked/y");
        let (s, root) = scan(r, 2);
        let node = |n: &str| root.subdir(&r.join(n)).unwrap();
        let view = s.view();
        assert_eq!(
            view.sharers(&node("a")),
            vec![r.join("b"), r.join("linked"), r.join("store")]
        );
        assert!(view.sharers(&node("b")).contains(&r.join("a")));
        write(&r.join("lonely/z"), 100);
        let (s2, root2) = scan(&r.join("lonely"), 2);
        assert!(s2.view().sharers(&root2).is_empty());
    }

    #[test]
    fn forgetting_a_folder_removes_it_and_its_bytes_from_every_total() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/f1"), 8192);
        write(&r.join("b/f2"), 4096);
        let (s, root) = scan(r, 2);
        assert_eq!((root.size(), root.files()), (12288, 2));
        let b = root.subdir(&r.join("b")).unwrap();
        assert!(s.forget_dir(&b));
        assert_eq!((root.size(), root.files()), (8192, 1));
        assert!(root.subdir(&r.join("b")).is_none());
        assert_eq!(s.stats().files, 1);
        assert_eq!(s.stats().bytes, 8192);
    }

    #[test]
    fn forgetting_is_refused_while_a_scan_runs_and_for_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/f1"), 100);
        let (s, root) = scan(r, 2);
        let a = root.subdir(&r.join("a")).unwrap();
        s.set_busy(true);
        assert!(!s.forget_dir(&a));
        s.set_busy(false);
        assert!(!s.forget_dir(&root), "the root has no parent to leave");
        assert!(root.subdir(&r.join("a")).is_some());
    }

    #[test]
    fn a_forgotten_folder_no_longer_counts_as_a_holder_of_shared_data() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let (s, root) = three_holders(r);
        let (a, b) = (
            root.subdir(&r.join("a")).unwrap(),
            root.subdir(&r.join("b")).unwrap(),
        );
        assert!(s.view().sharers(&b).contains(&r.join("a")));
        assert!(s.forget_dir(&a));
        assert!(!s.view().sharers(&b).contains(&r.join("a")));
        assert!(s.view().sharers(&b).contains(&r.join("store")));
    }

    #[test]
    fn clone_facts_tell_plain_files_from_clones() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("plain"), 8192);
        write(&r.join("orig"), 8192);
        clone_file(r, "orig", "clone");
        let plain = clone_facts(&r.join("plain")).unwrap();
        assert_eq!(plain.refcnt, 1);
        assert!(plain.private <= 8192);
        let orig = clone_facts(&r.join("orig")).unwrap();
        let clone = clone_facts(&r.join("clone")).unwrap();
        assert_eq!((orig.refcnt, clone.refcnt), (2, 2));
        assert_eq!(orig.clone_id, clone.clone_id);
        assert_eq!((orig.private, clone.private), (0, 0), "all of it is shared");
        assert!(clone_facts(&r.join("missing")).is_none());
    }

    #[test]
    fn a_folder_of_plain_files_has_no_sharing() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f1"), 8192);
        write(&dir.path().join("a/f2"), 4096);
        let (s, root) = scan(dir.path(), 2);
        let a = root.subdir(&dir.path().join("a")).unwrap();
        let sp = s.space(&a);
        assert_eq!(sp.footprint, a.disk());
        assert_eq!(sp.footprint, 8192 + 4096);
        assert_eq!((sp.shared_inside, sp.shared_outside, sp.saved), (0, 0, 0));
        // Everything is either freed now or held by a snapshot.
        assert_eq!(sp.frees_now + sp.snapshots, sp.footprint);
        assert!(identity_holds(&sp));
    }

    #[test]
    fn clones_inside_one_folder_are_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/orig"), 8192);
        clone_file(r, "a/orig", "a/clone");
        let (s, root) = scan(r, 2);
        let a = root.subdir(&r.join("a")).unwrap();
        assert_eq!(a.disk(), 16384, "per-file allocations add up");
        let sp = s.space(&a);
        assert_eq!(sp.footprint, 8192, "the shared data occupies disk once");
        assert_eq!(sp.saved, 8192);
        assert_eq!((sp.shared_inside, sp.shared_outside), (8192, 0));
        assert_eq!((sp.frees_now, sp.snapshots), (0, 0));
        assert!(identity_holds(&sp));
    }

    #[test]
    fn clones_across_folders_are_shared_with_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/orig"), 8192);
        clone_file(r, "a/orig", "b/clone");
        let (s, root) = scan(r, 2);
        let a = s.space(&root.subdir(&r.join("a")).unwrap());
        let b = s.space(&root.subdir(&r.join("b")).unwrap());
        for sp in [a, b] {
            assert_eq!(
                (sp.footprint, sp.shared_outside, sp.shared_inside),
                (8192, 8192, 0)
            );
            assert_eq!(sp.frees_now, 0, "the other clone keeps the data alive");
            assert!(identity_holds(&sp));
        }
        // Together they hold every member, so at the root it is all inside.
        let top = s.space(&root);
        assert_eq!(
            (top.footprint, top.shared_inside, top.shared_outside),
            (8192, 8192, 0)
        );
        assert_eq!(top.saved, 8192);
        assert!(identity_holds(&top));
    }

    #[test]
    fn a_clone_whose_partner_is_outside_the_scan_root_is_shared() {
        let outer = tempfile::tempdir().unwrap();
        let o = outer.path();
        write(&o.join("store/orig"), 8192);
        clone_file(o, "store/orig", "proj/clone");
        let (s, root) = scan(&o.join("proj"), 2);
        let sp = s.space(&root);
        assert_eq!(
            (sp.footprint, sp.shared_outside, sp.frees_now),
            (8192, 8192, 0)
        );
        assert!(identity_holds(&sp));
    }

    #[test]
    fn hard_links_all_inside_are_not_shared() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/x"), 8192);
        hard_link(r, "a/x", "a/y");
        let (s, root) = scan(r, 2);
        let sp = s.space(&root.subdir(&r.join("a")).unwrap());
        assert_eq!(sp.footprint, 8192, "one inode, counted once");
        assert_eq!(sp.shared_outside, 0);
        assert_eq!(sp.frees_now + sp.snapshots, 8192);
        assert_eq!(sp.saved, 8192, "the second link costs nothing");
        assert!(identity_holds(&sp));
    }

    #[test]
    fn a_hard_link_with_a_link_outside_is_shared_whichever_folder_was_credited() {
        for _ in 0..6 {
            let dir = tempfile::tempdir().unwrap();
            let r = dir.path();
            write(&r.join("a/f"), 8192);
            hard_link(r, "a/f", "b/f");
            let (s, root) = scan(r, 4);
            for name in ["a", "b"] {
                let sp = s.space(&root.subdir(&r.join(name)).unwrap());
                assert_eq!(
                    (sp.footprint, sp.shared_outside, sp.frees_now),
                    (8192, 8192, 0),
                    "{name}"
                );
                assert!(identity_holds(&sp), "{name}");
            }
            let top = s.space(&root);
            assert_eq!((top.footprint, top.shared_outside), (8192, 0));
            assert_eq!(top.frees_now + top.snapshots, 8192);
        }
    }

    #[test]
    fn a_hard_link_to_a_file_outside_the_scan_root_is_shared() {
        let outer = tempfile::tempdir().unwrap();
        let o = outer.path();
        write(&o.join("store/f"), 8192);
        hard_link(o, "store/f", "proj/f");
        let (s, root) = scan(&o.join("proj"), 2);
        let sp = s.space(&root);
        assert_eq!(
            (sp.footprint, sp.shared_outside, sp.frees_now),
            (8192, 8192, 0)
        );
    }

    fn mixed_tree(r: &Path) {
        write(&r.join("plain/a"), 8192);
        write(&r.join("plain/b"), 100);
        write(&r.join("dup/orig"), 8192);
        clone_file(r, "dup/orig", "dup/clone1");
        clone_file(r, "dup/orig", "other/clone2");
        write(&r.join("hl/x"), 4096);
        hard_link(r, "hl/x", "hl/y");
        hard_link(r, "hl/x", "other/z");
        write(&r.join("deep/one/two/f"), 12288);
        clone_file(r, "deep/one/two/f", "deep/one/g");
        fs::create_dir_all(r.join("empty")).unwrap();
        write(&r.join("zero"), 0);
    }

    #[test]
    fn the_space_identity_holds_everywhere_in_a_mixed_tree() {
        let dir = tempfile::tempdir().unwrap();
        mixed_tree(dir.path());
        let (s, root) = scan(dir.path(), 4);
        let top_footprint = s.space(&root).footprint;
        let mut stack = vec![Arc::clone(&root)];
        let mut checked = 0;
        while let Some(n) = stack.pop() {
            let sp = s.space(&n);
            assert!(identity_holds(&sp), "{:?}: {sp:?}", n.path);
            // Whichever folder the scanner credited a hard link to, no folder
            // can occupy more than the whole tree.
            assert!(
                sp.footprint <= top_footprint,
                "{:?}: {} > {top_footprint}",
                n.path,
                sp.footprint
            );
            checked += 1;
            stack.extend(n.children());
        }
        assert!(checked >= 8, "walked {checked} folders");
        let top = s.space(&root);
        assert_eq!(top.shared_outside, 0, "everything is inside the root");
    }

    #[test]
    fn batch_spaces_for_rows_match_the_single_folder_numbers() {
        let dir = tempfile::tempdir().unwrap();
        mixed_tree(dir.path());
        let (s, root) = scan(dir.path(), 4);
        let rows = s.list_children(&root);
        let mut dirs = 0;
        for row in rows.iter().filter(|r| r.is_dir) {
            let node = row.node.as_ref().expect("scanned folder");
            let expected = s.space(node);
            assert_eq!(row.space, Some(expected), "{:?}", row.name);
            assert_eq!(
                row.disk, expected.footprint,
                "disk is the footprint: {:?}",
                row.name
            );
            dirs += 1;
        }
        assert!(dirs >= 5);
        // Files keep their own allocation and carry no folder-level space.
        for row in rows.iter().filter(|r| !r.is_dir) {
            assert_eq!(row.space, None);
        }
    }

    #[test]
    fn rows_show_plain_allocation_while_a_scan_is_running() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("dup/orig"), 8192);
        clone_file(dir.path(), "dup/orig", "dup/clone");
        let (s, root) = scan(dir.path(), 2);
        s.set_busy(true);
        let rows = s.list_children(&root);
        s.set_busy(false);
        let dup = rows.iter().find(|r| r.name == "dup").unwrap();
        assert_eq!(
            dup.space, None,
            "clone data is only known once the scan is done"
        );
        assert_eq!(dup.disk, 16384);
        let done = s.list_children(&root);
        let dup = done.iter().find(|r| r.name == "dup").unwrap();
        assert_eq!(dup.disk, 8192, "the footprint counts the clone once");
    }

    #[test]
    fn live_rows_keep_the_footprint() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("dup/orig"), 8192);
        clone_file(dir.path(), "dup/orig", "dup/clone");
        let (s, root) = scan(dir.path(), 2);
        let rows = s.list_children(&root);
        let dup = rows.iter().find(|r| r.name == "dup").unwrap().live();
        assert_eq!(
            dup.disk, 8192,
            "live() must not fall back to the summed allocation"
        );
    }

    #[test]
    fn file_space_for_single_files() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("plain"), 8192);
        write(&r.join("linked"), 8192);
        hard_link(r, "linked", "linked2");
        write(&r.join("orig"), 8192);
        clone_file(r, "orig", "cloned");
        let plain = file_space(&r.join("plain")).unwrap();
        assert_eq!(plain.footprint, 8192);
        assert_eq!(plain.frees_now + plain.snapshots, 8192);
        assert_eq!(plain.shared_outside, 0);
        let linked = file_space(&r.join("linked")).unwrap();
        assert_eq!(
            (linked.footprint, linked.shared_outside, linked.frees_now),
            (8192, 8192, 0)
        );
        let cloned = file_space(&r.join("cloned")).unwrap();
        assert_eq!(
            (cloned.footprint, cloned.shared_outside, cloned.frees_now),
            (8192, 8192, 0)
        );
        assert!(file_space(&r.join("missing")).is_err());
        for sp in [plain, linked, cloned] {
            assert!(identity_holds(&sp));
        }
    }

    #[test]
    fn empty_files_and_unreadable_folders_do_not_disturb_the_space_numbers() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/empty"), 0);
        write(&dir.path().join("a/f"), 4096);
        let (s, root) = scan(dir.path(), 2);
        let sp = s.space(&root);
        assert_eq!(sp.footprint, 4096);
        assert!(identity_holds(&sp));
        assert_eq!(s.space(&root.subdir(&dir.path().join("a")).unwrap()), sp);
    }

    #[test]
    fn rescanning_a_folder_leaves_the_space_numbers_equal_to_a_fresh_scan() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/orig"), 8192);
        clone_file(r, "a/orig", "a/clone");
        write(&r.join("b/f"), 4096);
        let (s, root) = scan(r, 2);
        // Change b, then rescan only b.
        write(&r.join("b/g"), 4096);
        let b = root.subdir(&r.join("b")).unwrap();
        s.rescan_dir(&b).unwrap();
        wait(&s);
        let after = s.space(&root);
        let (fresh_scanner, fresh_root) = scan(r, 2);
        assert_eq!(after, fresh_scanner.space(&fresh_root));
        assert!(identity_holds(&after));
    }

    #[test]
    fn rescanning_forgets_clone_members_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/orig"), 8192);
        clone_file(r, "a/orig", "a/clone");
        let (s, root) = scan(r, 2);
        assert_eq!(s.space(&root).saved, 8192);
        fs::remove_file(r.join("a/clone")).unwrap();
        let a = root.subdir(&r.join("a")).unwrap();
        s.rescan_dir(&a).unwrap();
        wait(&s);
        let sp = s.space(&root);
        assert_eq!(sp.saved, 0, "the clone is gone: no stale members");
        assert_eq!(sp.footprint, 8192);
        assert!(identity_holds(&sp));
    }

    #[test]
    fn rescanning_everything_matches_a_fresh_scan_even_with_clones() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        mixed_tree(r);
        let (s, root) = scan(r, 4);
        let before = s.space(&root);
        let again = s.rescan_all(r).unwrap();
        wait(&s);
        assert_eq!(s.space(&again), before);
    }

    #[test]
    fn space_adds_up_for_review_totals() {
        let a = Space {
            footprint: 10,
            frees_now: 4,
            snapshots: 3,
            shared_outside: 2,
            shared_inside: 1,
            saved: 5,
        };
        let sum: Space = [a, a].into_iter().sum();
        assert_eq!(sum.footprint, 20);
        assert_eq!(sum.snapshots, 6);
        assert_eq!(sum.saved, 10);
        assert_eq!(a + Space::default(), a);
    }

    #[test]
    fn rows_know_what_deleting_them_frees_once_the_scan_is_done() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("plain"), 8192);
        write(&r.join("orig"), 8192);
        clone_file(r, "orig", "clone");
        write(&r.join("h1"), 4096);
        hard_link(r, "h1", "h2");
        write(&r.join("d/p"), 8192);
        write(&r.join("d/o"), 8192);
        clone_file(r, "d/o", "d/c");
        let (s, root) = scan(r, 2);
        let rows = s.list_children(&root);
        let frees = |name: &str| rows.iter().find(|x| x.name == name).unwrap().frees;
        assert_eq!(frees("orig"), Some(0), "clone data stays shared");
        assert_eq!(frees("clone"), Some(0));
        assert_eq!(frees("h1"), Some(0), "another link keeps a hard link alive");
        assert_eq!(frees("h2"), Some(0));
        assert_eq!(
            frees("plain"),
            Some(file_space(&r.join("plain")).unwrap().frees_now)
        );
        let d = rows.iter().find(|x| x.name == "d").unwrap();
        let d_node = d.node.as_ref().unwrap();
        assert_eq!(d.frees, Some(s.space(d_node).frees_now));
    }

    #[test]
    fn frees_are_unknown_while_a_scan_is_running() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("f"), 8192);
        write(&dir.path().join("d/g"), 4096);
        let (s, root) = scan(dir.path(), 2);
        s.set_busy(true);
        let busy = s.list_children(&root);
        s.set_busy(false);
        assert!(busy.iter().all(|x| x.frees.is_none()), "unknown, not zero");
        let idle = s.list_children(&root);
        assert!(idle.iter().all(|x| x.frees.is_some()));
    }

    #[test]
    fn only_a_shortfall_inside_the_data_can_be_a_snapshots() {
        // Fully covered: nothing held, even if blocks were preallocated past EOF.
        assert_eq!(held_bytes(2_600_000, 1_556_328, 2_076_672), 0);
        // Part of the real data missing from the private size: that part.
        assert_eq!(held_bytes(8192, 8192, 4096), 4096);
        assert_eq!(held_bytes(8192, 8192, 0), 8192);
        // A tiny file occupies a whole block.
        assert_eq!(held_bytes(4096, 100, 4096), 0);
        // Sparse: only what is allocated can be missing.
        assert_eq!(held_bytes(4096, 16 << 20, 0), 4096);
    }

    #[test]
    fn snapshot_blame_needs_a_snapshot_the_file_predates_and_real_data_forks() {
        let snap = 1_000_000;
        assert!(!may_hold(NO_SNAPSHOT, Some(5), 1), "no snapshot, no blame");
        assert!(may_hold(snap, Some(snap - 1), 1));
        assert!(may_hold(snap, Some(snap), 1));
        assert!(!may_hold(snap, Some(snap + 1), 1), "created after it");
        assert!(
            may_hold(snap, None, 1),
            "unknown birth time: cannot rule out"
        );
        assert!(may_hold(UNKNOWN_SNAPSHOT, Some(snap + 1), 1));
        assert!(
            !may_hold(snap, Some(5), 0),
            "refcnt 0: compressed, not held"
        );
    }

    #[test]
    fn snapshot_names_are_read_as_local_time() {
        let a = snapshot_time("com.apple.TimeMachine.2026-11-06-170157.local").unwrap();
        let b = snapshot_time("com.apple.TimeMachine.2026-11-06-170257.local").unwrap();
        assert_eq!(b - a, 60);
        assert!(snapshot_time("com.apple.os.update-ABC").is_none());
        assert!(snapshot_time("com.apple.TimeMachine.garbage").is_none());
        assert!(snapshot_time("").is_none());
    }

    #[test]
    fn a_macos_compressed_file_is_never_blamed_on_a_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::write(r.join("t"), "hello world\n".repeat(200_000)).unwrap();
        let ok = std::process::Command::new("/usr/bin/ditto")
            .arg("--hfsCompression")
            .arg(r.join("t"))
            .arg(r.join("c"))
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "needs ditto");
        let sp = file_space(&r.join("c")).unwrap();
        assert_eq!(sp.snapshots, 0);
        assert_eq!(sp.frees_now, sp.footprint, "it all frees on delete");
    }

    #[test]
    fn a_folder_not_yet_started_is_queued_and_a_finished_one_is_not() {
        let row = |is_dir, scanning| Row {
            name: "x".into(),
            path: "/x".into(),
            is_dir,
            size: 0,
            disk: 0,
            files: 0,
            scanning,
            node: None,
            space: None,
            frees: None,
        };
        assert!(row(true, true).queued(), "undiscovered folder waits");
        assert!(!row(true, false).queued());
        assert!(!row(false, false).queued(), "files are never queued");

        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d")).unwrap();
        let s = Scanner::new(2);
        let root = s.scan_root(t.path());
        assert!(s.wait_idle(Duration::from_secs(30)));
        let rows = s.list_children(&root);
        assert!(rows.iter().all(|r| !r.queued()), "nothing queued when done");
        assert!(root.children().iter().all(|n| n.is_started()));
    }

    #[test]
    fn live_rows_keep_what_they_free() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("d/f"), 8192);
        let (s, root) = scan(dir.path(), 2);
        let rows = s.list_children(&root);
        let d = rows.iter().find(|x| x.name == "d").unwrap();
        assert_eq!(d.live().frees, d.frees);
        assert!(d.frees.is_some());
    }
}
