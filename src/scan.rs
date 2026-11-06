use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use crate::usage::Usage;

/// Publish partial totals after this many files so counters keep moving
/// even while scanning one huge folder.
const FLUSH_EVERY: u64 = 4096;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
    /// 1 for this directory's own listing plus one per unfinished child.
    pending: AtomicUsize,
    done: AtomicBool,
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
            pending: AtomicUsize::new(1),
            done: AtomicBool::new(false),
            subdirs: Mutex::new(Vec::new()),
        })
    }

    /// Logical bytes in the subtree.
    pub fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// Allocated bytes on disk in the subtree (`blocks * 512`).
    pub fn disk(&self) -> u64 {
        self.disk.load(Ordering::Relaxed)
    }

    pub fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
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
}

/// Add to a node and every ancestor.
fn add_up(node: &Arc<Node>, size: u64, disk: u64, files: u64) {
    let mut cur = Some(Arc::clone(node));
    while let Some(n) = cur {
        n.size.fetch_add(size, Ordering::Relaxed);
        n.disk.fetch_add(disk, Ordering::Relaxed);
        n.files.fetch_add(files, Ordering::Relaxed);
        cur = n.parent.upgrade();
    }
}

/// Subtract from a node and every ancestor (never below zero).
fn sub_up(node: &Arc<Node>, size: u64, disk: u64, files: u64) {
    let sub = |counter: &AtomicU64, by: u64| {
        let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(by))
        });
    };
    let mut cur = Some(Arc::clone(node));
    while let Some(n) = cur {
        sub(&n.size, size);
        sub(&n.disk, disk);
        sub(&n.files, files);
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

/// One entry shown in the explorer list.
#[derive(Clone)]
pub struct Row {
    pub name: OsString,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub disk: u64,
    pub files: u64,
    pub scanning: bool,
    pub node: Option<Arc<Node>>,
}

impl Row {
    /// Re-read live totals from the node (if the row has one).
    pub fn live(&self) -> Row {
        match &self.node {
            Some(n) => Row {
                size: n.size(),
                disk: n.disk(),
                files: n.files(),
                scanning: !n.is_done(),
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
    /// The folder whose totals include this file (the first one scanned).
    counted_by: Option<PathBuf>,
    /// Every link path seen.
    paths: Vec<PathBuf>,
}

/// How a folder's on-disk space splits between files only it holds and
/// files that also have a hard link somewhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// Every link to these files is inside the folder: deleting it frees them.
    pub unique: Usage,
    /// These files also have a link outside the folder: deleting it frees
    /// nothing for them.
    pub shared: Usage,
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

/// Unique/shared for a single file: a hard-linked file frees nothing when
/// one link is removed.
pub fn file_breakdown(path: &Path) -> std::io::Result<Breakdown> {
    let md = fs::symlink_metadata(path)?;
    let usage = Usage::new(md.len(), md.blocks() * 512);
    Ok(if md.is_file() && md.nlink() > 1 {
        Breakdown {
            unique: Usage::default(),
            shared: usage,
        }
    } else {
        Breakdown {
            unique: usage,
            shared: Usage::default(),
        }
    })
}

struct Shared {
    /// Every hard-linked file seen, by (device, inode). Kept so the explorer
    /// can tell space unique to a folder from space shared with other places.
    links: Mutex<HashMap<(u64, u64), LinkGroup>>,
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
    /// stat'ed directly.
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
                });
            }
        }
        rows
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
                add_up(&owner, size, disk, 1);
                self.shared.files_seen.fetch_add(1, Ordering::Relaxed);
                self.shared.bytes_seen.fetch_add(size, Ordering::Relaxed);
                self.shared.disk_seen.fetch_add(disk, Ordering::Relaxed);
            }
        }
    }

    /// Split a folder's space into what only it holds (deleting it frees
    /// this) and what is shared with hard links elsewhere (this stays).
    /// Independent of which folder the scanner happened to credit first.
    pub fn breakdown(&self, node: &Arc<Node>) -> Breakdown {
        let total = Usage::new(node.size(), node.disk());
        let mut credited_here = Usage::default();
        let mut unique_links = Usage::default();
        let mut shared = Usage::default();
        for group in lock(&self.shared.links).values() {
            let inside = group
                .paths
                .iter()
                .filter(|p| p.starts_with(&node.path))
                .count() as u64;
            if inside == 0 {
                continue;
            }
            let usage = Usage::new(group.size, group.disk);
            if group
                .counted_by
                .as_deref()
                .is_some_and(|dir| dir.starts_with(&node.path))
            {
                credited_here += usage;
            }
            if inside >= group.nlink {
                unique_links += usage;
            } else {
                shared += usage;
            }
        }
        // Plain files plus hard-linked files whose every link is inside.
        Breakdown {
            unique: total.saturating_sub(credited_here) + unique_links,
            shared,
        }
    }

    /// Start over: scan `path` again from scratch with fresh counters.
    /// Returns `None` while a scan is still running.
    pub fn rescan_all(&self, path: &Path) -> Option<Arc<Node>> {
        if !self.is_idle() {
            return None;
        }
        lock(&self.shared.links).clear();
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
        let (size, disk, files) = (node.size(), node.disk(), node.files());

        // Forget what the old subtree contributed.
        sub_up(&parent, size, disk, files);
        for (counter, by) in [
            (&self.shared.files_seen, files),
            (&self.shared.bytes_seen, size),
            (&self.shared.disk_seen, disk),
        ] {
            let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(by))
            });
        }
        self.forget_links_under(node, &parent);
        lock(&self.shared.denied).retain(|p| !p.starts_with(&node.path));

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

fn walk(shared: Arc<Shared>, node: Arc<Node>) {
    lock(&shared.threads).insert(thread::current().id());
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
    let (mut size, mut disk, mut files) = (0u64, 0u64, 0u64);
    let flush = |size: &mut u64, disk: &mut u64, files: &mut u64| {
        if *files == 0 && *size == 0 {
            return;
        }
        add_up(node, *size, *disk, *files);
        shared.files_seen.fetch_add(*files, Ordering::Relaxed);
        shared.bytes_seen.fetch_add(*size, Ordering::Relaxed);
        shared.disk_seen.fetch_add(*disk, Ordering::Relaxed);
        (*size, *disk, *files) = (0, 0, 0);
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
            if md.nlink() > 1 && md.is_file() {
                let mut links = lock(&shared.links);
                let group = links
                    .entry((md.dev(), md.ino()))
                    .or_insert_with(|| LinkGroup {
                        nlink: md.nlink(),
                        size: md.len(),
                        disk: md.blocks() * 512,
                        counted_by: None,
                        paths: Vec::new(),
                    });
                group.paths.push(entry.path());
                if group.counted_by.is_some() {
                    continue; // hard link already counted
                }
                group.counted_by = Some(node.path.clone());
            }
            size += md.len();
            disk += md.blocks() * 512;
            files += 1;
            if files >= FLUSH_EVERY {
                flush(&mut size, &mut disk, &mut files);
            }
        }
    }
    flush(&mut size, &mut disk, &mut files);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Usage;
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
    fn breakdown_of_a_plain_folder_is_all_unique() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/f1"), 5000);
        write(&dir.path().join("a/f2"), 6000);
        let (s, root) = scan(dir.path(), 2);
        let a = root.subdir(&dir.path().join("a")).unwrap();
        let b = s.breakdown(&a);
        assert_eq!(b.shared, Usage::default());
        assert_eq!(b.unique, Usage::new(a.size(), a.disk()));
    }

    #[test]
    fn breakdown_splits_shared_hard_links_whichever_thread_won() {
        // store/pkg is hard-linked into proj/nm/pkg; proj/nm/own is a plain file.
        // Which folder the scanner credits with pkg is a race; the breakdown
        // must not depend on it.
        for _ in 0..6 {
            let dir = tempfile::tempdir().unwrap();
            let r = dir.path();
            write(&r.join("store/pkg"), 8192);
            write(&r.join("proj/nm/own"), 4096);
            link(r, "store/pkg", "proj/nm/pkg");
            let (s, root) = scan(r, 4);
            let proj = root.subdir(&r.join("proj")).unwrap();
            let store = root.subdir(&r.join("store")).unwrap();

            let bp = s.breakdown(&proj);
            assert_eq!((bp.unique.size, bp.shared.size), (4096, 8192), "proj");
            let bs = s.breakdown(&store);
            assert_eq!((bs.unique.size, bs.shared.size), (0, 8192), "store");
            // Everything is inside the root, so nothing is shared there.
            let br = s.breakdown(&root);
            assert_eq!((br.unique.size, br.shared.size), (8192 + 4096, 0), "root");
        }
    }

    #[test]
    fn breakdown_counts_links_outside_the_scan_root_as_shared() {
        let outer = tempfile::tempdir().unwrap();
        let o = outer.path();
        write(&o.join("store/pkg"), 8192);
        write(&o.join("proj/own"), 4096);
        link(o, "store/pkg", "proj/pkg");
        // Scan only proj: the store link is never seen, but nlink says it exists.
        let (s, root) = scan(&o.join("proj"), 2);
        let b = s.breakdown(&root);
        assert_eq!((b.unique.size, b.shared.size), (4096, 8192));
    }

    #[test]
    fn breakdown_hard_links_inside_one_folder_are_unique_once() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a/x"), 8192);
        link(r, "a/x", "a/y");
        let (s, root) = scan(r, 2);
        let a = root.subdir(&r.join("a")).unwrap();
        let b = s.breakdown(&a);
        assert_eq!((b.unique.size, b.shared.size), (8192, 0));
    }

    #[test]
    fn file_breakdown_tells_linked_files_from_plain_ones() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("plain"), 100);
        write(&r.join("one"), 200);
        link(r, "one", "two");
        let plain = file_breakdown(&r.join("plain")).unwrap();
        assert_eq!((plain.unique.size, plain.shared.size), (100, 0));
        let linked = file_breakdown(&r.join("one")).unwrap();
        assert_eq!((linked.unique.size, linked.shared.size), (0, 200));
        assert!(file_breakdown(&r.join("missing")).is_err());
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
}
