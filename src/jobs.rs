/// How a confirmed deletion proceeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Move to the ToBeDeleted folder only.
    SoftOnly,
    /// Move, then permanently empty what was just moved.
    SoftThenEmpty,
}

use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;

use crate::safety::Policy;
use crate::softdelete::Holding;
use crate::usage::Usage;

#[derive(Clone, Debug)]
pub enum DeleteEvent {
    /// One step finished. `total` counts every step of every phase.
    Progress {
        done: usize,
        total: usize,
        usage: Usage,
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
    pub fn start(
        holding: Arc<Holding>,
        policy: Arc<Policy>,
        items: Vec<(PathBuf, Usage)>,
        mode: DeleteMode,
    ) -> DeleteJob {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || run(&holding, &policy, items, mode, &tx));
        DeleteJob { rx }
    }

    /// Everything that happened since the last poll (never blocks).
    pub fn poll(&self) -> Vec<DeleteEvent> {
        self.rx.try_iter().collect()
    }
}

fn run(
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
    let total = items.len() * phases;
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
        let _ = tx.send(DeleteEvent::Progress {
            done,
            total,
            usage: moved,
            current: path,
        });
    }

    if mode == DeleteMode::SoftThenEmpty {
        for (id, path) in held {
            match holding.delete_forever(&id) {
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
            let _ = tx.send(DeleteEvent::Progress {
                done,
                total,
                usage: moved + emptied,
                current: path,
            });
        }
    }

    let _ = tx.send(DeleteEvent::Finished {
        moved,
        emptied,
        failures,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::Policy;
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
}
