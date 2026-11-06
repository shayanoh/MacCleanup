//! The Docker page's behaviour on the App: loading in the background,
//! moving, folding, marking and sorting its list.

use std::collections::BTreeSet;

use super::*;
use crate::docker::analysis::{ItemKind, Refusal, frees_of_set};
use crate::docker::cli::DockerCli;
use crate::docker::model::ItemId;
use crate::docker::state::DockerLoad;
use crate::docker::view::{DKind, DSection};

/// Is this key in the shared selection one of Docker's?
pub fn is_docker_id(id: &str) -> bool {
    ["image:", "container:", "volume:", "cache:", "vm:"]
        .iter()
        .any(|p| id.starts_with(p))
}

impl App {
    /// Start (or queue) a read of Docker on a thread.
    pub fn reload_docker(&mut self) {
        if self.docker.job.pending {
            self.docker.stale = true;
            return;
        }
        self.docker.stale = false;
        let runner = Arc::clone(&self.runner);
        let home = self.policy.home().to_path_buf();
        let work = Arc::clone(&self.docker.work);
        work.begin("connecting to Docker", 0);
        self.docker.job.spawn(move || {
            let cli = DockerCli::connect(runner, &home)?;
            let loaded = cli.load(&work)?;
            Ok(DockerLoad { cli, loaded })
        });
    }

    /// Take in a finished read; ask again if one was queued meanwhile.
    pub(super) fn poll_docker(&mut self) {
        if self.docker.job.poll() {
            match self.docker.job.value.take() {
                Some(Ok(load)) => {
                    self.docker.accept(load);
                    // Things that no longer exist cannot stay marked.
                    let analysis = &self.docker.analysis;
                    self.tool_marks.retain(|id| {
                        !is_docker_id(id)
                            || analysis.items.contains_key(id)
                            || id == crate::docker::analysis::TRIM_ITEM
                    });
                    self.docker_refilter();
                    self.marks_changed();
                }
                Some(Err(e)) => {
                    let docker = &mut self.docker;
                    docker.error = Some(e);
                    docker.read_once = true;
                    docker.load = None;
                    docker.analysis = Default::default();
                    docker.lines.clear();
                    docker.cursor = 0;
                    docker.offset = 0;
                }
                None => {}
            }
        }
        if self.docker.stale && !self.docker.job.pending {
            self.reload_docker();
        }
    }

    /// The Docker items in the shared selection.
    pub fn docker_marked(&self) -> BTreeSet<ItemId> {
        self.tool_marks
            .iter()
            .filter(|id| is_docker_id(id))
            .cloned()
            .collect()
    }

    /// What everything marked on the Docker page frees inside Docker.
    pub fn docker_frees(&self) -> Option<u64> {
        let (data, layers) = (self.docker.data()?, self.docker.layers()?);
        Some(frees_of_set(
            &self.docker.analysis,
            data,
            layers,
            &self.docker_marked(),
        ))
    }

    pub(super) fn docker_refilter(&mut self) {
        let query = crate::filter::parse(&self.filter.query);
        self.docker.rebuild(&query);
        self.ensure_visible();
    }

    pub(super) fn docker_move(&mut self, delta: i64) {
        self.docker.cursor = step(self.docker.cursor, delta, self.docker.lines.len());
        self.ensure_visible();
    }

    fn docker_set_open(&mut self, key: &str, open: bool) {
        self.docker.view.set_open(key, open);
        self.docker_refilter();
    }

    /// `l`: open the folded line under the cursor.
    pub(super) fn docker_into(&mut self) {
        if let Some(l) = self.docker.selected().cloned()
            && l.open == Some(false)
        {
            self.docker_set_open(&l.key, true);
        }
    }

    /// `h`: fold the open line under the cursor, else climb to its parent.
    pub(super) fn docker_out(&mut self) {
        let Some(l) = self.docker.selected().cloned() else {
            return;
        };
        if l.open == Some(true) {
            self.docker_set_open(&l.key, false);
            return;
        }
        let parent = self.docker.lines[..self.docker.cursor]
            .iter()
            .rposition(|p| p.depth < l.depth);
        if let Some(at) = parent {
            self.docker.cursor = at;
            self.ensure_visible();
        }
    }

    /// A click on list row `row` (0 = the first visible one): select it, or
    /// fold/unfold it when it is already selected.
    pub(super) fn docker_click(&mut self, row: usize) {
        let idx = self.docker.offset + row;
        if idx >= self.docker.lines.len() {
            return;
        }
        if idx != self.docker.cursor {
            self.docker.cursor = idx;
            return;
        }
        if let Some(l) = self.docker.selected().cloned()
            && let Some(open) = l.open
        {
            self.docker_set_open(&l.key, !open);
        }
    }

    /// `m`: mark the stopped containers the selected image or volume needs
    /// removed first (Docker refuses to remove it otherwise).
    pub(super) fn docker_mark_needed(&mut self) {
        let Some(line) = self.docker.selected().cloned() else {
            return;
        };
        let needs: Vec<String> = self
            .docker
            .facts(&line.key)
            .map(|f| f.needs.clone())
            .unwrap_or_default();
        let mut changed = 0;
        for id in needs {
            let ok = self.docker.facts(&id).is_some_and(|f| f.removable.is_ok());
            if ok && self.tool_marks.insert(id) {
                changed += 1;
            }
        }
        if changed > 0 {
            self.marks_changed();
            self.notice = Some(format!(
                "Marked {changed} container(s) that must go with it"
            ));
        } else {
            self.notice = Some("Nothing more to mark here".to_string());
        }
    }

    /// Shift-J / Shift-K: scroll the details pane, never past its ends.
    pub(super) fn docker_scroll_details(&mut self, by: i16) {
        if self.screen != Screen::Docker {
            return;
        }
        let d = &mut self.docker;
        d.detail_scroll = d
            .detail_scroll
            .saturating_add_signed(isize::from(by))
            .min(d.detail_max);
    }

    pub(super) fn docker_cycle_sort(&mut self) {
        self.docker.view.sort = self.docker.view.sort.next();
        self.docker_refilter();
    }

    pub(super) fn toggle_docker_mark(&mut self) {
        let Some(line) = self.docker.selected().cloned() else {
            return;
        };
        match line.kind {
            DKind::Section(section) => self.mark_docker_section(section),
            DKind::Layer(_) => {
                self.notice = Some("A layer goes with its image: mark the image".to_string());
            }
            _ => self.toggle_docker_item(&line.key),
        }
    }

    fn toggle_docker_item(&mut self, key: &str) {
        if self.tool_marks.remove(key) {
            self.marks_changed();
            return;
        }
        let Some(facts) = self.docker.facts(key) else {
            return;
        };
        match &facts.removable {
            Ok(()) => {}
            Err(Refusal::Running(names)) => {
                self.notice = Some(format!(
                    "In use by running container(s): {}. Stop them in Docker first",
                    names.join(", ")
                ));
                return;
            }
            Err(Refusal::CacheInUse) => {
                self.notice = Some("A build is using this cache record right now".to_string());
                return;
            }
        }
        self.tool_marks.insert(key.to_string());
        self.marks_changed();
    }

    /// Mark (or unmark) everything in a section that is safe to take in
    /// bulk: dangling images and unused build cache. Volumes never are.
    fn mark_docker_section(&mut self, section: DSection) {
        if section == DSection::Volumes {
            self.notice = Some("Volumes hold data: mark them one at a time".to_string());
            return;
        }
        let start = self.docker.cursor + 1;
        let ids: Vec<String> = self.docker.lines[start.min(self.docker.lines.len())..]
            .iter()
            .take_while(|l| l.depth > 0)
            .filter(|l| {
                self.docker.facts(&l.key).is_some_and(|f| {
                    f.bulk_ok && matches!(f.kind, ItemKind::Image | ItemKind::CacheRecord)
                })
            })
            .map(|l| l.key.clone())
            .collect();
        if ids.is_empty() {
            self.notice = Some("Nothing here is safe to mark in bulk".to_string());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use crate::docker::cli::DockerError;
    use crate::docker::view::DSort;
    use crate::testutil::env;
    use std::sync::Arc;

    const DF: &str = include_str!("../../tests/fixtures/docker/system_df.json");
    const CONTAINERS: &str = include_str!("../../tests/fixtures/docker/containers.json");
    const CONTAINER_INSPECT: &str =
        include_str!("../../tests/fixtures/docker/container_inspect.json");
    const HISTORY: &str = include_str!("../../tests/fixtures/docker/history.json");
    const INSPECT: &str = include_str!("../../tests/fixtures/docker/inspect_image.json");
    const CONTEXT: &str = include_str!("../../tests/fixtures/docker/context_inspect.json");

    fn healthy() -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT),
            ("/_ping", "OK"),
            ("/system/df", DF),
            ("/containers/json?all=1", CONTAINERS),
            ("/containers/c0ffee1/json", CONTAINER_INSPECT),
            ("/images/sha256:aaa1/history", HISTORY),
            ("/images/sha256:aaa1/json", INSPECT),
        ]))
    }

    /// An app on the Docker page with everything read.
    fn loaded_app() -> crate::testutil::Env {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = healthy();
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        e
    }

    fn wait_docker(app: &mut App) {
        for _ in 0..2000 {
            app.update(Action::Tick);
            if !app.docker.job.pending && !app.docker.stale {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("docker read did not finish");
    }

    fn cursor_on(app: &mut App, key: &str) {
        app.docker.cursor = app
            .docker
            .lines
            .iter()
            .position(|l| l.key == key)
            .unwrap_or_else(|| panic!("no line {key}: {:?}", app.docker.lines));
    }

    #[test]
    fn page_loads_in_the_background_and_never_blocks_update() {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = healthy();
        e.app.update(Action::GoTo(Screen::Docker));
        assert!(e.app.docker.job.pending, "the read runs on a thread");
        assert!(
            e.app.docker.lines.is_empty(),
            "the first paint does not wait"
        );
        wait_docker(&mut e.app);
        assert!(!e.app.docker.lines.is_empty());
        assert!(e.app.docker.error.is_none());
    }

    #[test]
    fn entering_the_page_twice_starts_one_read() {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = healthy();
        e.app.update(Action::GoTo(Screen::Docker));
        e.app.update(Action::GoTo(Screen::Explorer));
        e.app.update(Action::GoTo(Screen::Docker));
        assert_eq!(e.app.docker.job.spawned, 1);
        wait_docker(&mut e.app);
        assert_eq!(
            e.app.docker.job.spawned, 2,
            "the second request was remembered and ran once the first landed"
        );
    }

    #[test]
    fn sidebar_digit_opens_the_docker_page() {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = healthy();
        let n = SIDEBAR.iter().position(|s| s.label == "Docker").unwrap() + 1;
        e.app.update(Action::Menu(n));
        assert_eq!(e.app.screen, Screen::Docker);
        assert_eq!(e.app.focus, Focus::Main);
    }

    #[test]
    fn not_running_shows_the_reason_and_r_retries() {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner =
            Arc::new(FakeRunner::new(&["docker"]).answering(&[("context inspect", CONTEXT)]));
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        assert_eq!(e.app.docker.error, Some(DockerError::NotRunning));
        assert!(e.app.docker.lines.is_empty());
        e.app.runner = healthy();
        e.app.update(Action::Rescan);
        wait_docker(&mut e.app);
        assert!(e.app.docker.error.is_none());
        assert!(!e.app.docker.lines.is_empty());
    }

    #[test]
    fn running_containers_refuse_marks_with_the_reason() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "image:sha256:aaa1");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
        let note = e.app.notice.clone().unwrap();
        assert!(note.contains("web-1"), "{note}");
    }

    #[test]
    fn a_stopped_containers_image_can_be_marked_and_unmarked() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "container:c0ffee2");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.contains("container:c0ffee2"));
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
    }

    #[test]
    fn a_section_mark_takes_only_bulk_ok_rows() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "section:cache");
        e.app.update(Action::ToggleMark);
        let marked: Vec<&String> = e.app.tool_marks.iter().collect();
        assert_eq!(marked, ["cache:child1", "cache:root1"], "nolast is in use");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty(), "again unmarks them");
    }

    #[test]
    fn volumes_are_never_taken_by_a_section_mark() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "section:volumes");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
        assert!(e.app.notice.is_some());
    }

    #[test]
    fn docker_frees_follows_the_marks() {
        let mut e = loaded_app();
        assert_eq!(e.app.docker_frees(), Some(0));
        cursor_on(&mut e.app, "section:cache");
        e.app.update(Action::ToggleMark);
        assert_eq!(e.app.docker_frees(), Some(117_480_604 + 1_297_818_308));
    }

    #[test]
    fn sort_key_cycles_the_docker_sort_not_the_explorers() {
        let mut e = loaded_app();
        let explorer_sort = e.app.sort;
        assert_eq!(e.app.docker.view.sort, DSort::Size);
        e.app.update(Action::CycleSort);
        assert_eq!(e.app.docker.view.sort, DSort::Frees);
        assert_eq!(e.app.sort, explorer_sort);
    }

    #[test]
    fn into_and_out_fold_and_unfold_and_out_climbs_to_the_parent() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "image:sha256:bbb2");
        let open = e.app.docker.lines.len();
        e.app.update(Action::Out);
        assert!(e.app.docker.lines.len() < open, "folded");
        e.app.update(Action::Into);
        assert_eq!(e.app.docker.lines.len(), open, "unfolded again");
        cursor_on(&mut e.app, "container:c0ffee2");
        e.app.update(Action::Out);
        assert_eq!(
            e.app.docker.selected().unwrap().key,
            "image:sha256:bbb2",
            "from a leaf, out goes to the parent"
        );
    }

    #[test]
    fn m_marks_the_stopped_containers_an_image_needs() {
        let mut e = loaded_app();
        cursor_on(&mut e.app, "image:sha256:bbb2");
        e.app.update(Action::MarkRelated);
        assert!(e.app.tool_marks.contains("container:c0ffee2"));
        assert!(
            !e.app.tool_marks.contains("image:sha256:bbb2"),
            "only the related ones"
        );
        e.app.update(Action::MarkRelated);
        assert_eq!(e.app.notice.as_deref(), Some("Nothing more to mark here"));
    }

    #[test]
    fn marks_of_things_that_vanished_are_dropped_on_the_next_read() {
        let mut e = loaded_app();
        e.app.tool_marks.insert("image:gone".to_string());
        e.app.tool_marks.insert("pnpm".to_string());
        e.app.update(Action::Rescan);
        wait_docker(&mut e.app);
        assert!(!e.app.tool_marks.contains("image:gone"));
        assert!(
            e.app.tool_marks.contains("pnpm"),
            "other kinds are not touched"
        );
    }

    #[test]
    fn filter_narrows_the_docker_list() {
        let mut e = loaded_app();
        let all = e.app.docker.lines.len();
        e.app.filter.query = "pgdata".to_string();
        e.app.refilter(false);
        assert!(e.app.docker.lines.len() < all);
        assert!(e.app.docker.lines.iter().any(|l| l.key == "volume:pgdata"));
    }
}
