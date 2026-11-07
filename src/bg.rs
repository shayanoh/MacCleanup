//! Work done on a thread where only the newest request matters: marking a
//! row asks for fresh numbers, and the answer to an older request that
//! arrives late must never replace the answer to a newer one.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread;

/// The latest result of a background computation.
pub struct Latest<T> {
    generation: u64,
    tx: mpsc::Sender<(u64, Option<T>)>,
    rx: mpsc::Receiver<(u64, Option<T>)>,
    /// The newest result that arrived (kept while a newer one is computed).
    pub value: Option<T>,
    /// True from a request until its result (or failure) is seen by [`poll`](Latest::poll).
    pub pending: bool,
    /// How many threads have been started (for tests that guard against a
    /// request storm).
    pub spawned: u64,
}

impl<T: Send + 'static> Latest<T> {
    pub fn new() -> Latest<T> {
        let (tx, rx) = mpsc::channel();
        Latest {
            generation: 0,
            tx,
            rx,
            value: None,
            pending: false,
            spawned: 0,
        }
    }

    /// Start computing on a new thread; older requests are forgotten.
    pub fn spawn(&mut self, work: impl FnOnce() -> T + Send + 'static) {
        self.generation += 1;
        self.spawned += 1;
        self.pending = true;
        let (generation, tx) = (self.generation, self.tx.clone());
        thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work)).ok();
            let _ = tx.send((generation, result));
        });
    }

    /// Use a value that is already known; older requests are forgotten.
    pub fn set_now(&mut self, value: T) {
        self.generation += 1;
        self.value = Some(value);
        self.pending = false;
    }

    /// Take in what arrived. Returns true if the value changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, result)) = self.rx.try_recv() {
            if generation != self.generation {
                continue; // an older request answering late
            }
            self.pending = false;
            if let Some(v) = result {
                self.value = Some(v);
                changed = true;
            }
        }
        changed
    }
}

impl<T: Send + 'static> Default for Latest<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// The threads for long background walks over many files (measuring a
/// snapshot, sizing a pnpm store). They must not use rayon's global pool:
/// the UI thread's own parallel work (`Scanner::fill_frees`) is queued behind
/// a running walk there, and the screen froze for as long as the walk lasted
/// (47 s measured). Half the cores, so the UI keeps some.
pub fn walk_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        let build = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .thread_name(|i| format!("maccleanup-walk-{i}"))
                .build()
        };
        // If the system will not give that many threads, try one.
        build((cores / 2).clamp(2, 8))
            .or_else(|_| build(1))
            .expect("could not start a thread for background work")
    })
}

/// Progress of one piece of background work, readable from the UI thread:
/// what it is doing, and how far along when the number of steps is known.
pub struct Work {
    phase: std::sync::Mutex<String>,
    done: std::sync::atomic::AtomicUsize,
    total: std::sync::atomic::AtomicUsize,
}

/// A moment in a [`Work`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkSnapshot {
    pub phase: String,
    pub done: usize,
    /// 0 when the number of steps is not known.
    pub total: usize,
}

impl WorkSnapshot {
    /// How far along (0..=1), or `None` when the length is not known.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| (self.done.min(self.total) as f64) / self.total as f64)
    }
}

impl Work {
    pub fn new() -> std::sync::Arc<Work> {
        std::sync::Arc::new(Work {
            phase: std::sync::Mutex::new(String::new()),
            done: std::sync::atomic::AtomicUsize::new(0),
            total: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Start a phase of `total` steps (0 when unknown).
    pub fn begin(&self, phase: &str, total: usize) {
        self.stage(0, total, phase);
    }

    /// Jump to `done` of `total` steps in `phase`.
    pub fn stage(&self, done: usize, total: usize, phase: &str) {
        use std::sync::atomic::Ordering::Relaxed;
        if let Ok(mut p) = self.phase.lock() {
            *p = phase.to_string();
        }
        self.total.store(total, Relaxed);
        self.done.store(done, Relaxed);
    }

    /// One more step finished.
    pub fn step(&self) {
        self.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> WorkSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        WorkSnapshot {
            phase: self.phase.lock().map(|p| p.clone()).unwrap_or_default(),
            done: self.done.load(Relaxed),
            total: self.total.load(Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait<T: Send + 'static>(l: &mut Latest<T>) {
        for _ in 0..400 {
            l.poll();
            if !l.pending {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("background work did not finish");
    }

    #[test]
    fn the_result_arrives_and_pending_clears() {
        let mut l: Latest<u32> = Latest::new();
        assert!(!l.pending);
        l.spawn(|| 7);
        assert!(l.pending);
        wait(&mut l);
        assert_eq!(l.value, Some(7));
    }

    #[test]
    fn a_stale_result_is_dropped_when_a_newer_request_exists() {
        let mut l: Latest<&'static str> = Latest::new();
        l.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            "old"
        });
        l.spawn(|| "new");
        wait(&mut l);
        assert_eq!(l.value, Some("new"));
        // Let the slow one finish and be seen: it must not overwrite.
        std::thread::sleep(Duration::from_millis(250));
        l.poll();
        assert_eq!(l.value, Some("new"));
        assert!(!l.pending);
    }

    #[test]
    fn set_now_replaces_the_value_and_ignores_older_work() {
        let mut l: Latest<u32> = Latest::new();
        l.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            1
        });
        l.set_now(2);
        assert!(!l.pending);
        std::thread::sleep(Duration::from_millis(200));
        l.poll();
        assert_eq!(l.value, Some(2));
    }

    #[test]
    fn a_panicking_job_keeps_the_last_good_value_and_clears_pending() {
        let mut l: Latest<u32> = Latest::new();
        l.set_now(5);
        l.spawn(|| panic!("boom"));
        wait(&mut l);
        assert_eq!(l.value, Some(5), "the last good value survives");
    }

    #[test]
    fn work_reports_its_phase_and_steps() {
        let w = Work::new();
        assert_eq!(w.snapshot().total, 0);
        w.stage(1, 3, "finding folders");
        let s = w.snapshot();
        assert_eq!(
            (s.done, s.total, s.phase.as_str()),
            (1, 3, "finding folders")
        );
        assert!((s.fraction().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        w.begin("asking tools", 4);
        w.step();
        w.step();
        assert_eq!(w.snapshot().fraction(), Some(0.5));
        w.begin("unknown length", 0);
        assert_eq!(w.snapshot().fraction(), None, "no total: no fraction");
    }

    #[test]
    fn work_never_reports_more_than_done() {
        let w = Work::new();
        w.begin("x", 2);
        for _ in 0..5 {
            w.step();
        }
        assert_eq!(w.snapshot().fraction(), Some(1.0));
    }

    #[test]
    fn a_busy_walk_pool_does_not_block_the_global_pool() {
        use rayon::prelude::*;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
        use std::time::Duration;
        let pool = walk_pool();
        let release = Arc::new(AtomicBool::new(false));
        // Keep every walk worker busy, as a long snapshot measurement does.
        for _ in 0..pool.current_num_threads() * 2 {
            let r = Arc::clone(&release);
            pool.spawn(move || {
                while !r.load(Relaxed) {
                    thread::sleep(Duration::from_millis(1));
                }
            });
        }
        // What the UI thread does in `Scanner::fill_frees`.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut rows = vec![0u8; 50];
            rows.par_iter_mut().for_each(|r| *r += 1);
            let _ = tx.send(());
        });
        let done = rx.recv_timeout(Duration::from_secs(10)).is_ok();
        release.store(true, Relaxed);
        assert!(
            done,
            "parallel work on the global pool waited for the walk pool"
        );
    }
}
