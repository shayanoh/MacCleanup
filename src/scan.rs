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

struct Shared {
    seen: Mutex<HashSet<(u64, u64)>>,
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
                seen: Mutex::new(HashSet::new()),
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
            if md.nlink() > 1 && md.is_file() && !lock(&shared.seen).insert((md.dev(), md.ino())) {
                continue; // hard link already counted
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
}
