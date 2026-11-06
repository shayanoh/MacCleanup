/// How a confirmed deletion proceeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the ToBeDeleted folder only.
    SoftOnly,
    /// Move, then permanently empty what was just moved.
    SoftThenEmpty,
    /// Permanently delete items already in ToBeDeleted.
    EmptyHeld,
}

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use crate::safety::Policy;
use crate::softdelete::{DeleteCounters, HeldItem, HoldError, Holding};
use crate::usage::Usage;

/// Which part of a delete is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Moving items into ToBeDeleted (instant renames).
    Moving,
    /// Permanently deleting files.
    Deleting,
}

#[derive(Clone, Debug)]
pub enum DeleteEvent {
    /// Progress so far. `usage` is how much has been processed and `of` the
    /// expected grand total (a move and a delete each count once), so
    /// `usage / of` is the fraction done. `done` / `total` count items.
    Progress {
        done: usize,
        total: usize,
        usage: Usage,
        of: Usage,
        phase: Phase,
        current: PathBuf,
    },
    Failed {
        path: PathBuf,
        error: String,
    },
    Finished {
        moved: Usage,
        emptied: Usage,
        failures: usize,
    },
}

/// A delete running on its own thread; the UI polls it without blocking.
pub struct DeleteJob {
    rx: mpsc::Receiver<DeleteEvent>,
}

impl DeleteJob {
    /// Move items to ToBeDeleted, then (for `SoftThenEmpty`) delete them for good.
    pub fn start(
        holding: Arc<Holding>,
        policy: Arc<Policy>,
        items: Vec<(PathBuf, Usage)>,
        mode: DeleteMode,
    ) -> DeleteJob {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || run_move(&holding, &policy, items, mode, &tx));
        DeleteJob { rx }
    }

    /// Permanently delete items that are already in ToBeDeleted.
    pub fn empty_held(holding: Arc<Holding>, items: Vec<HeldItem>) -> DeleteJob {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || run_empty(&holding, items, &tx));
        DeleteJob { rx }
    }

    /// Everything that happened since the last poll (never blocks).
    pub fn poll(&self) -> Vec<DeleteEvent> {
        self.rx.try_iter().collect()
    }
}

/// Where a running delete stands, for progress events.
struct Tally<'a> {
    tx: &'a mpsc::Sender<DeleteEvent>,
    total: usize,
    of: Usage,
}

impl Tally<'_> {
    fn progress(&self, done: usize, usage: Usage, phase: Phase, current: &Path) {
        let _ = self.tx.send(DeleteEvent::Progress {
            done,
            total: self.total,
            usage,
            of: self.of,
            phase,
            current: current.to_path_buf(),
        });
    }

    /// Delete one held item for good on a helper thread while this thread
    /// reports the helper's live counters, so a single huge folder still
    /// shows steady progress.
    fn delete_forever(
        &self,
        holding: &Holding,
        id: &str,
        base: Usage,
        done: usize,
        current: &Path,
    ) -> Result<Usage, HoldError> {
        let counters = DeleteCounters::default();
        thread::scope(|scope| {
            let worker = scope.spawn(|| holding.delete_forever_with(id, &counters));
            while !worker.is_finished() {
                thread::sleep(Duration::from_millis(40));
                self.progress(done, base + counters.usage(), Phase::Deleting, current);
            }
            worker
                .join()
                .unwrap_or_else(|_| Err(HoldError::Io(io::Error::other("delete thread panicked"))))
        })
    }
}

fn run_move(
    holding: &Holding,
    policy: &Policy,
    items: Vec<(PathBuf, Usage)>,
    mode: DeleteMode,
    tx: &mpsc::Sender<DeleteEvent>,
) {
    let phases = if mode == DeleteMode::SoftThenEmpty {
        2
    } else {
        1
    };
    let items_total: Usage = items.iter().map(|(_, u)| *u).sum();
    let tally = Tally {
        tx,
        total: items.len() * phases,
        of: (0..phases).fold(Usage::default(), |acc, _| acc + items_total),
    };
    let (mut done, mut failures) = (0usize, 0usize);
    let (mut moved, mut emptied) = (Usage::default(), Usage::default());
    let mut held: Vec<(String, PathBuf)> = Vec::new();

    for (path, usage) in items {
        match holding.move_in(policy, &path, usage) {
            Ok(item) => {
                moved += usage;
                held.push((item.id, path.clone()));
            }
            Err(e) => {
                failures += 1;
                let _ = tx.send(DeleteEvent::Failed {
                    path: path.clone(),
                    error: e.to_string(),
                });
            }
        }
        done += 1;
        tally.progress(done, moved, Phase::Moving, &path);
    }

    if mode == DeleteMode::SoftThenEmpty {
        for (id, path) in held {
            match tally.delete_forever(holding, &id, moved + emptied, done, &path) {
                Ok(usage) => emptied += usage,
                Err(e) => {
                    failures += 1;
                    let _ = tx.send(DeleteEvent::Failed {
                        path: path.clone(),
                        error: e.to_string(),
                    });
                }
            }
            done += 1;
            tally.progress(done, moved + emptied, Phase::Deleting, &path);
        }
    }

    let _ = tx.send(DeleteEvent::Finished {
        moved,
        emptied,
        failures,
    });
}

fn run_empty(holding: &Holding, items: Vec<HeldItem>, tx: &mpsc::Sender<DeleteEvent>) {
    let tally = Tally {
        tx,
        total: items.len(),
        of: items.iter().map(HeldItem::usage).sum(),
    };
    let (mut done, mut failures) = (0usize, 0usize);
    let mut emptied = Usage::default();
    for item in items {
        let shown = PathBuf::from(&item.held_name);
        match tally.delete_forever(holding, &item.id, emptied, done, &shown) {
            Ok(usage) => emptied += usage,
            Err(e) => {
                failures += 1;
                let _ = tx.send(DeleteEvent::Failed {
                    path: item.original.clone(),
                    error: e.to_string(),
                });
            }
        }
        done += 1;
        tally.progress(done, emptied, Phase::Deleting, &shown);
    }
    let _ = tx.send(DeleteEvent::Finished {
        moved: Usage::default(),
        emptied,
        failures,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::Policy;
    use crate::softdelete::HeldItem;
    use crate::usage::Usage;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    struct Sandbox {
        _dir: tempfile::TempDir,
        home: PathBuf,
        holding: Arc<Holding>,
        policy: Arc<Policy>,
    }

    fn sandbox() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let hold = base.join("hold");
        Sandbox {
            _dir: dir,
            home: home.clone(),
            holding: Arc::new(Holding::open(hold.clone()).unwrap()),
            policy: Arc::new(Policy::new(&home, &hold)),
        }
    }

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn run_to_end(job: &DeleteJob) -> Vec<DeleteEvent> {
        let start = Instant::now();
        let mut all = Vec::new();
        loop {
            all.extend(job.poll());
            if all
                .iter()
                .any(|e| matches!(e, DeleteEvent::Finished { .. }))
            {
                return all;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "job timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn two_files(s: &Sandbox) -> Vec<(PathBuf, Usage)> {
        write(&s.home.join("p/a"), 100);
        write(&s.home.join("p/b"), 200);
        vec![
            (s.home.join("p/a"), Usage::new(100, 4096)),
            (s.home.join("p/b"), Usage::new(200, 4096)),
        ]
    }

    fn finished(events: &[DeleteEvent]) -> (Usage, Usage, usize) {
        match events.last() {
            Some(DeleteEvent::Finished {
                moved,
                emptied,
                failures,
            }) => (*moved, *emptied, *failures),
            other => panic!("last event not Finished: {}", other.is_some()),
        }
    }

    #[test]
    fn soft_only_job_moves_items_and_reports() {
        let s = sandbox();
        let items = two_files(&s);
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            items,
            DeleteMode::SoftOnly,
        );
        let events = run_to_end(&job);
        let (moved, emptied, failures) = finished(&events);
        assert_eq!(moved, Usage::new(300, 8192));
        assert_eq!(emptied, Usage::new(0, 0));
        assert_eq!(failures, 0);
        let dones: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                DeleteEvent::Progress { done, .. } => Some(*done),
                _ => None,
            })
            .collect();
        assert_eq!(dones, [1, 2]);
        assert_eq!(s.holding.list().unwrap().len(), 2);
        assert!(!s.home.join("p/a").exists());
    }

    #[test]
    fn progress_reports_on_disk_size_too() {
        let s = sandbox();
        let items = two_files(&s);
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            items,
            DeleteMode::SoftOnly,
        );
        let events = run_to_end(&job);
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                DeleteEvent::Progress { usage, .. } => Some(*usage),
                _ => None,
            })
            .unwrap();
        assert_eq!(last, Usage::new(300, 8192));
    }

    #[test]
    fn soft_then_empty_job_removes_everything() {
        let s = sandbox();
        let items = two_files(&s);
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            items,
            DeleteMode::SoftThenEmpty,
        );
        let events = run_to_end(&job);
        let (moved, emptied, failures) = finished(&events);
        assert_eq!(moved, Usage::new(300, 8192));
        assert_eq!(emptied, Usage::new(300, 8192));
        assert_eq!(failures, 0);
        assert!(s.holding.list().unwrap().is_empty());
        let totals: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                DeleteEvent::Progress { total, .. } => Some(*total),
                _ => None,
            })
            .collect();
        assert!(totals.iter().all(|t| *t == 4), "{totals:?}");
    }

    #[test]
    fn protected_item_fails_but_job_continues() {
        let s = sandbox();
        fs::create_dir_all(s.home.join("Documents")).unwrap();
        write(&s.home.join("p/a"), 100);
        let items = vec![
            (s.home.join("Documents"), Usage::new(0, 0)),
            (s.home.join("p/a"), Usage::new(100, 4096)),
        ];
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            items,
            DeleteMode::SoftOnly,
        );
        let events = run_to_end(&job);
        let failed = events
            .iter()
            .filter(|e| matches!(e, DeleteEvent::Failed { .. }))
            .count();
        assert_eq!(failed, 1);
        let (moved, _, failures) = finished(&events);
        assert_eq!((moved, failures), (Usage::new(100, 4096), 1));
        assert!(s.home.join("Documents").exists());
        assert!(!s.home.join("p/a").exists());
    }

    #[test]
    fn empty_item_list_finishes_immediately() {
        let s = sandbox();
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            Vec::new(),
            DeleteMode::SoftThenEmpty,
        );
        let events = run_to_end(&job);
        assert_eq!(events.len(), 1);
    }

    fn held_items(s: &Sandbox, files: &[(&str, usize)]) -> Vec<HeldItem> {
        let mut out = Vec::new();
        for (rel, n) in files {
            write(&s.home.join(rel), *n);
            let usage = Usage::new(*n as u64, 4096);
            out.push(
                s.holding
                    .move_in(&s.policy, &s.home.join(rel), usage)
                    .unwrap(),
            );
        }
        out
    }

    #[test]
    fn progress_covers_both_phases_in_bytes() {
        let s = sandbox();
        let items = two_files(&s);
        let job = DeleteJob::start(
            Arc::clone(&s.holding),
            Arc::clone(&s.policy),
            items,
            DeleteMode::SoftThenEmpty,
        );
        let events = run_to_end(&job);
        let progress: Vec<(Usage, Usage, Phase)> = events
            .iter()
            .filter_map(|e| match e {
                DeleteEvent::Progress {
                    usage, of, phase, ..
                } => Some((*usage, *of, *phase)),
                _ => None,
            })
            .collect();
        assert!(
            progress
                .iter()
                .all(|(_, of, _)| *of == Usage::new(600, 16384))
        );
        let last = progress.last().unwrap();
        assert_eq!(last.0.size, 600, "300 moved + 300 deleted");
        // Every Moving event comes before every Deleting event.
        let first_delete = progress
            .iter()
            .position(|p| p.2 == Phase::Deleting)
            .unwrap();
        assert!(
            progress[..first_delete]
                .iter()
                .all(|p| p.2 == Phase::Moving)
        );
        assert!(
            progress[first_delete..]
                .iter()
                .all(|p| p.2 == Phase::Deleting)
        );
        // Bytes never go backwards.
        assert!(progress.windows(2).all(|w| w[1].0.size >= w[0].0.size));
    }

    #[test]
    fn empty_held_job_deletes_what_is_held_and_reports() {
        let s = sandbox();
        let items = held_items(&s, &[("p/a", 100), ("p/b", 200)]);
        let job = DeleteJob::empty_held(Arc::clone(&s.holding), items);
        let events = run_to_end(&job);
        let (moved, emptied, failures) = finished(&events);
        assert_eq!(moved, Usage::default());
        assert_eq!(emptied, Usage::new(300, 8192));
        assert_eq!(failures, 0);
        assert!(s.holding.list().unwrap().is_empty());
        let phases: Vec<Phase> = events
            .iter()
            .filter_map(|e| match e {
                DeleteEvent::Progress { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect();
        assert!(!phases.is_empty());
        assert!(phases.iter().all(|p| *p == Phase::Deleting));
    }

    #[test]
    fn empty_held_job_with_a_big_tree_ends_with_full_progress() {
        let s = sandbox();
        for i in 0..30 {
            for j in 0..20 {
                write(&s.home.join(format!("big/d{i}/f{j}")), 1);
            }
        }
        let item = s
            .holding
            .move_in(&s.policy, &s.home.join("big"), Usage::new(600, 4096 * 600))
            .unwrap();
        let job = DeleteJob::empty_held(Arc::clone(&s.holding), vec![item]);
        let events = run_to_end(&job);
        let (_, emptied, failures) = finished(&events);
        assert_eq!((emptied.size, failures), (600, 0));
        assert!(s.holding.list().unwrap().is_empty());
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                DeleteEvent::Progress { usage, of, .. } => Some((*usage, *of)),
                _ => None,
            })
            .unwrap();
        assert_eq!(last.0.size, last.1.size.min(last.0.size));
        assert!(last.0.size >= 600);
    }

    #[test]
    fn empty_held_job_tolerates_a_stale_entry() {
        let s = sandbox();
        let items = held_items(&s, &[("p/a", 100)]);
        // Simulate a crash between manifest write and move: payload missing.
        std::fs::remove_dir_all(s.holding.dir().join("items").join(&items[0].id)).unwrap();
        let job = DeleteJob::empty_held(Arc::clone(&s.holding), items);
        let events = run_to_end(&job);
        let (_, _, failures) = finished(&events);
        assert_eq!(failures, 0, "a missing payload just clears the entry");
        assert!(s.holding.list().unwrap().is_empty());
    }
}
