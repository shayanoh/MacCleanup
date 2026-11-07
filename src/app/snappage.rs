//! The Time Machine page's behaviour on the App: reading the snapshots and
//! measuring them on threads, marking them, and the review commands that
//! delete them. Free space is watched after a delete (`Settle`).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::snapshots::{self, Held, Progress, Settle, Snapshot};

/// Is this key in the shared selection a snapshot's?
pub fn is_snapshot_id(id: &str) -> bool {
    id.strip_prefix("snapshot:")
        .is_some_and(snapshots::valid_date)
}

/// What a measuring job hands back: each date with its size or the reason it failed.
pub type Measured = Vec<(String, Result<Held, String>)>;

pub struct SnapState {
    pub job: Latest<Result<Vec<Snapshot>, String>>,
    pub items: Vec<Snapshot>,
    pub error: Option<String>,
    /// The first read has come back.
    pub loaded: bool,
    /// A reread was asked for while one was running.
    pub stale: bool,
    pub cursor: usize,
    pub offset: usize,
    /// Estimated bytes each Time Machine snapshot holds, by date. Present
    /// only after the user measured it (a minute on a big disk).
    pub sizes: HashMap<String, Held>,
    pub measure: Latest<Measured>,
    pub cancel: Arc<AtomicBool>,
    pub measuring: Vec<String>,
    /// Dates asked for while a measurement ran; they run next, in order.
    pub queue: Vec<String>,
    /// The tick the running measurement began on (for its elapsed time).
    pub started_tick: u64,
    /// Files looked at and bytes found so far by the running measurement.
    pub progress: Arc<Progress>,
    pub settle: Settle,
}

impl SnapState {
    pub fn new() -> SnapState {
        SnapState {
            job: Latest::new(),
            items: Vec::new(),
            error: None,
            loaded: false,
            stale: false,
            cursor: 0,
            offset: 0,
            sizes: HashMap::new(),
            measure: Latest::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            measuring: Vec::new(),
            queue: Vec::new(),
            started_tick: 0,
            progress: Progress::new(),
            settle: Settle::default(),
        }
    }

    /// The snapshot under the cursor; `None` on the heading (cursor 0), which
    /// stands for all of them.
    pub fn selected(&self) -> Option<&Snapshot> {
        self.cursor.checked_sub(1).and_then(|i| self.items.get(i))
    }

    /// How many list rows there are: the heading and one per snapshot.
    pub fn rows(&self) -> usize {
        self.items.len() + 1
    }

    /// Sum of the sizes measured so far, and how many Time Machine snapshots
    /// they cover.
    pub fn measured_total(&self) -> (Held, usize) {
        let sum = self.sizes.values().fold(Held::default(), |a, h| Held {
            low: a.low + h.low,
            high: a.high + h.high,
        });
        (sum, self.sizes.len())
    }

    fn dates(&self) -> Vec<String> {
        self.items.iter().filter_map(|s| s.date.clone()).collect()
    }
}

impl Default for SnapState {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Read the snapshots on a thread (queued if one is already running).
    pub fn reload_snapshots(&mut self) {
        if self.snaps.job.pending {
            self.snaps.stale = true;
            return;
        }
        self.snaps.stale = false;
        let runner = Arc::clone(&self.runner);
        self.snaps.job.spawn(move || snapshots::load(&*runner));
    }

    /// Every tick: take in finished reads and measurements, and watch free
    /// space while it settles after a delete.
    pub(super) fn poll_snapshots(&mut self) {
        if self.snaps.job.poll() {
            match self.snaps.job.value.clone() {
                Some(Ok(list)) => {
                    self.snaps.error = None;
                    self.snaps.items = list;
                    let dates = self.snaps.dates();
                    self.snaps.sizes.retain(|d, _| dates.contains(d));
                    self.snaps.queue.retain(|d| dates.contains(d));
                    // A mark of a snapshot that is gone cannot run.
                    self.tool_marks.retain(|id| {
                        !is_snapshot_id(id) || dates.iter().any(|d| id.ends_with(d.as_str()))
                    });
                    self.marks_changed();
                }
                Some(Err(e)) => {
                    self.snaps.items.clear();
                    self.snaps.error = Some(e);
                }
                None => {}
            }
            // The first read puts the cursor on the first snapshot, not the heading.
            if !self.snaps.loaded && !self.snaps.items.is_empty() {
                self.snaps.cursor = 1;
            }
            self.snaps.loaded = true;
            self.snaps.cursor = self.snaps.cursor.min(self.snaps.items.len());
        }
        if !self.snaps.job.pending && self.snaps.stale {
            self.reload_snapshots();
        }
        if self.snaps.measure.poll() {
            let done = self.snaps.measure.value.take().unwrap_or_default();
            self.snaps.measuring.clear();
            let mut failed = None;
            for (date, result) in done {
                match result {
                    Ok(bytes) => {
                        self.snaps.sizes.insert(date, bytes);
                    }
                    Err(e) => failed = Some(e),
                }
            }
            if let Some(e) = failed {
                self.notice = Some(format!("Could not measure: {e}"));
            }
            // What was asked for meanwhile runs next.
            if !self.snaps.queue.is_empty() {
                let next = std::mem::take(&mut self.snaps.queue);
                self.start_measure(next);
            }
        }
        if self.snaps.settle.active(self.tick) {
            if self.tick.is_multiple_of(5) {
                self.refresh_free();
            }
            self.snaps.settle.observe(self.tick, self.free_now);
        }
    }

    pub(super) fn snap_move(&mut self, delta: i64) {
        self.snaps.cursor = step(self.snaps.cursor, delta, self.snaps.rows());
    }

    pub(super) fn snap_click(&mut self, row: usize) {
        let idx = self.snaps.offset + row;
        if idx < self.snaps.rows() {
            self.snaps.cursor = idx;
        }
    }

    /// Space: mark or unmark the selected snapshot for deletion (on the
    /// heading: all of them).
    pub(super) fn toggle_snapshot_mark(&mut self) {
        if self.snaps.cursor == 0 {
            self.mark_all_snapshots();
            return;
        }
        let Some(snap) = self.snaps.selected() else {
            return;
        };
        let Some(id) = snap.id() else {
            self.notice = Some("Only Time Machine snapshots can be deleted here".to_string());
            return;
        };
        if !self.tool_marks.remove(&id) {
            self.tool_marks.insert(id);
        }
        self.marks_changed();
    }

    /// `a`: mark every Time Machine snapshot, or unmark them all when they are.
    pub(super) fn mark_all_snapshots(&mut self) {
        let ids: Vec<String> = self.snaps.items.iter().filter_map(Snapshot::id).collect();
        if ids.is_empty() {
            return;
        }
        if ids.iter().all(|id| self.tool_marks.contains(id)) {
            for id in &ids {
                self.tool_marks.remove(id);
            }
        } else {
            self.tool_marks.extend(ids);
        }
        self.marks_changed();
    }

    /// `m` / `M`: estimate what the selected (or every) snapshot would free.
    pub(super) fn measure_snapshots(&mut self, all: bool) {
        if self.screen != Screen::Snapshots {
            return;
        }
        let dates: Vec<String> = if all || self.snaps.cursor == 0 {
            self.snaps.dates()
        } else {
            self.snaps
                .selected()
                .and_then(|s| s.date.clone())
                .into_iter()
                .collect()
        };
        if dates.is_empty() {
            self.notice = Some("No Time Machine snapshot to measure".to_string());
            return;
        }
        let busy = self.snaps.measure.pending;
        let new: Vec<String> = dates
            .into_iter()
            .filter(|d| !self.snaps.measuring.contains(d) && !self.snaps.queue.contains(d))
            .collect();
        if new.is_empty() {
            self.notice = Some("Already measuring or queued".to_string());
        } else if busy {
            self.notice = Some(format!(
                "Queued {} snapshot(s): they run after the one being measured",
                new.len()
            ));
            self.snaps.queue.extend(new);
        } else {
            self.start_measure(new);
        }
    }

    /// Start measuring `dates`, one after the other, on a thread.
    fn start_measure(&mut self, dates: Vec<String>) {
        let runner = Arc::clone(&self.runner);
        let every = self.snaps.dates();
        let progress = Arc::clone(&self.snaps.progress);
        progress.start(0, dates.len() as u64);
        let cancel = Arc::new(AtomicBool::new(false));
        self.snaps.cancel = Arc::clone(&cancel);
        self.snaps.measuring = dates.clone();
        self.snaps.started_tick = self.tick;
        self.snaps.measure.spawn(move || {
            let of = dates.len() as u64;
            dates
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    progress.start(i as u64, of);
                    let r = snapshots::measure(&*runner, &d, &every, &progress, &cancel);
                    (d, r)
                })
                .collect()
        });
    }

    /// The marked snapshots as review commands, oldest first.
    pub(super) fn snap_review_commands(&self) -> Vec<ReviewCommand> {
        self.snaps
            .items
            .iter()
            .filter_map(|s| {
                let (id, date) = (s.id()?, s.date.as_deref()?);
                if !self.tool_marks.contains(&id) {
                    return None;
                }
                let spec = snapshots::delete_spec(date)?;
                Some(ReviewCommand {
                    id,
                    tool: "Time Machine".to_string(),
                    line: spec.display(),
                    spec,
                    up_to: self.snaps.sizes.get(date).map_or(0, |h| h.high),
                    path: PathBuf::new(),
                    can_force: false,
                    force: false,
                    group: Vec::new(),
                    warn: None,
                })
            })
            .collect()
    }

    /// Snapshot commands ran: read the list again and watch free space.
    pub(super) fn snapshots_deleted(&mut self) {
        self.reload_snapshots();
        self.snaps.settle.start(self.tick, self.free_now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use crate::testutil::{Env, env, settle};

    const DATES: &str = "Snapshot dates for disk /:\n2026-11-07-032842\n2026-11-06-020608\n";
    const SYSTEM: &str = "|   Name:        com.apple.bless.3D4C\n";

    fn runner() -> Arc<FakeRunner> {
        Arc::new(
            FakeRunner::new(&[])
                .answering(&[("listlocalsnapshotdates", DATES), ("listSnapshots", SYSTEM)]),
        )
    }

    fn wait(app: &mut App) {
        for _ in 0..2000 {
            app.update(Action::Tick);
            if !app.snaps.job.pending && !app.snaps.stale {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("snapshot read did not finish");
    }

    fn loaded(fake: Arc<FakeRunner>) -> Env {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = fake;
        e.app.update(Action::GoTo(Screen::Snapshots));
        wait(&mut e.app);
        e
    }

    #[test]
    fn snapshots_page_lists_what_the_runner_reports() {
        let e = loaded(runner());
        let names: Vec<_> = e.app.snaps.items.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "com.apple.TimeMachine.2026-11-06-020608.local",
                "com.apple.TimeMachine.2026-11-07-032842.local",
                "com.apple.bless.3D4C",
            ],
            "Time Machine's oldest first, then the system's"
        );
        assert!(e.app.snaps.error.is_none());
    }

    #[test]
    fn a_silent_tmutil_shows_an_error_and_no_rows() {
        let e = loaded(Arc::new(FakeRunner::new(&[])));
        assert!(e.app.snaps.items.is_empty());
        assert!(e.app.snaps.error.is_some());
    }

    #[test]
    fn space_marks_a_time_machine_snapshot_and_refuses_a_system_one() {
        let mut e = loaded(runner());
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.contains("snapshot:2026-11-06-020608"));
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty(), "again unmarks");
        e.app.snaps.cursor = 3;
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
        assert!(e.app.notice.as_deref().unwrap().contains("Time Machine"));
    }

    #[test]
    fn space_on_the_heading_marks_every_time_machine_snapshot() {
        let mut e = loaded(runner());
        assert_eq!(e.app.snaps.cursor, 1, "the first read starts on a snapshot");
        e.app.update(Action::Up);
        assert_eq!(e.app.snaps.cursor, 0, "the heading is the row above");
        e.app.update(Action::ToggleMark);
        assert_eq!(e.app.tool_marks.len(), 2, "never the system snapshot");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty(), "again unmarks all");
    }

    #[test]
    fn a_marks_all_then_unmarks_all() {
        let mut e = loaded(runner());
        e.app.update(Action::SnapMarkAll);
        assert_eq!(e.app.tool_marks.len(), 2, "never the system snapshot");
        e.app.update(Action::SnapMarkAll);
        assert!(e.app.tool_marks.is_empty());
    }

    #[test]
    fn review_deletes_marked_snapshots_oldest_first_and_rereads_afterwards() {
        let fake = runner();
        let mut e = loaded(fake.clone());
        e.app.update(Action::SnapMarkAll);
        e.app.update(Action::OpenReview);
        match &e.app.dialog {
            Some(crate::app::Dialog::Review(r)) => {
                let lines: Vec<_> = r.commands.iter().map(|c| c.line.as_str()).collect();
                assert_eq!(
                    lines,
                    [
                        "tmutil deletelocalsnapshots 2026-11-06-020608",
                        "tmutil deletelocalsnapshots 2026-11-07-032842"
                    ]
                );
            }
            other => panic!("expected a review: {other:?}"),
        }
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let ran: Vec<String> = fake
            .calls()
            .iter()
            .filter(|c| c.display().contains("deletelocalsnapshots"))
            .map(|c| c.display())
            .collect();
        assert_eq!(ran.len(), 2, "{ran:?}");
        assert!(e.app.tool_marks.is_empty());
        assert!(
            e.app.snaps.settle.active(e.app.tick),
            "free space is watched after a delete"
        );
    }

    #[test]
    fn a_mark_whose_snapshot_vanished_is_dropped_on_reread() {
        let mut e = loaded(runner());
        e.app
            .tool_marks
            .insert("snapshot:2020-01-01-000000".to_string());
        e.app.update(Action::Rescan);
        wait(&mut e.app);
        assert!(e.app.tool_marks.is_empty());
    }

    #[test]
    fn m_without_snapshots_only_says_so() {
        let mut e = loaded(Arc::new(FakeRunner::new(&[])));
        e.app.update(Action::SnapMeasure { all: false });
        assert!(e.app.notice.as_deref().unwrap().contains("No Time Machine"));
        assert!(!e.app.snaps.measure.pending);
    }

    #[test]
    fn m_while_measuring_queues_the_next_snapshot_and_runs_it_after() {
        let fake = Arc::new(
            FakeRunner::new(&[])
                .answering(&[
                    (
                        "listlocalsnapshotdates",
                        "2001-01-01-010101\n2001-01-01-020202",
                    ),
                    ("listSnapshots", ""),
                ])
                .slow(std::time::Duration::from_millis(300)),
        );
        let mut e = loaded(fake);
        e.app.update(Action::SnapMeasure { all: false });
        assert!(e.app.snaps.measure.pending);
        e.app.update(Action::Down);
        e.app.update(Action::SnapMeasure { all: false });
        assert_eq!(e.app.snaps.queue, ["2001-01-01-020202"]);
        assert!(e.app.notice.as_deref().unwrap().contains("Queued"));
        e.app.update(Action::SnapMeasure { all: false });
        assert!(e.app.notice.as_deref().unwrap().contains("Already"));
        for _ in 0..4000 {
            e.app.update(Action::Tick);
            if e.app.snaps.sizes.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            e.app.snaps.sizes.len(),
            2,
            "both measured, one after the other"
        );
        assert!(e.app.snaps.queue.is_empty());
    }

    #[test]
    fn the_digit_for_time_machine_is_its_menu_position() {
        let n = SIDEBAR
            .iter()
            .position(|s| s.label == "Time Machine")
            .unwrap()
            + 1;
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = runner();
        e.app.update(Action::Menu(n));
        assert_eq!(e.app.screen, Screen::Snapshots);
    }
}
