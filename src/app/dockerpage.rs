//! The Docker page's behaviour on the App: loading in the background,
//! moving, folding, marking and sorting its list.

use std::collections::BTreeSet;

use super::*;
use crate::docker::analysis::{
    ItemKind, Refusal, TRIM_ITEM, blockers, frees_of_set, removal_order,
};
use crate::docker::cli::{DockerCli, vm_disk_allocated};
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
            let vm_tools = crate::docker::vmtools::is_present(&cli);
            Ok(DockerLoad {
                cli,
                loaded,
                vm_tools,
            })
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
                    self.docker_trim_housekeeping();
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

    fn has_docker_removals(&self) -> bool {
        self.docker_marked().iter().any(|id| id != TRIM_ITEM)
    }

    /// A removal was marked: the VM-disk trim comes along, unless the user
    /// took it off.
    fn docker_marks_added(&mut self) {
        if !self.docker.trim_declined && !self.tool_marks.contains(TRIM_ITEM) {
            self.tool_marks.insert(TRIM_ITEM.to_string());
            self.docker.trim_auto = true;
        }
    }

    /// With no removal left, a trim that only came along goes too, and the
    /// user's earlier "no" is forgotten.
    fn docker_trim_housekeeping(&mut self) {
        if !self.has_docker_removals() {
            if self.docker.trim_auto {
                self.tool_marks.remove(TRIM_ITEM);
            }
            self.docker.trim_auto = false;
            self.docker.trim_declined = false;
        }
    }

    /// The VM-disk trim row, marked or unmarked by the user.
    fn toggle_docker_trim(&mut self) {
        if self.tool_marks.remove(TRIM_ITEM) {
            self.docker.trim_declined = self.has_docker_removals();
            self.docker.trim_auto = false;
        } else {
            self.tool_marks.insert(TRIM_ITEM.to_string());
            self.docker.trim_auto = false;
            self.docker.trim_declined = false;
        }
        self.marks_changed();
    }

    fn toggle_docker_item(&mut self, key: &str) {
        if key == TRIM_ITEM {
            self.toggle_docker_trim();
            return;
        }
        if self.tool_marks.remove(key) {
            self.docker_trim_housekeeping();
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
        self.docker_marks_added();
        self.marks_changed();
    }

    /// The commands that remove what is marked on the Docker page, in the
    /// order they must run, then (when the trim is marked) the build of our
    /// helper image if it is missing and the trim itself.
    pub(super) fn docker_review_commands(&self) -> Vec<ReviewCommand> {
        let Some(load) = &self.docker.load else {
            return Vec::new();
        };
        let marked = self.docker_marked();
        if marked.is_empty() {
            return Vec::new();
        }
        let (a, data) = (&self.docker.analysis, &load.loaded.data);
        let command = |id: String, spec: crate::catalog::CommandSpec, up_to: u64| ReviewCommand {
            id,
            tool: "Docker".to_string(),
            line: spec.display(),
            spec,
            up_to,
            path: PathBuf::new(),
            can_force: false,
            force: false,
        };
        let mut out = Vec::new();
        let mut trim = false;
        for id in removal_order(a, &marked) {
            if id == TRIM_ITEM {
                trim = true;
            } else if let Some(spec) = crate::docker::remove::removal_spec(&load.cli, data, &id) {
                let up_to = a.items.get(&id).map_or(0, |f| f.frees_alone);
                out.push(command(id, spec, up_to));
            }
        }
        if trim {
            if !load.vm_tools {
                out.push(command(
                    "vm:build".to_string(),
                    crate::docker::vmtools::build_spec(&load.cli),
                    0,
                ));
            }
            out.push(command(
                TRIM_ITEM.to_string(),
                crate::docker::vmtools::trim_spec(&load.cli),
                data.vm_allocated.unwrap_or(0).saturating_sub(data.in_use()),
            ));
        }
        out
    }

    /// What stops the marked Docker items from running: a removal Docker
    /// would refuse because stopped containers hold the item.
    pub(super) fn docker_problems(&self) -> Vec<String> {
        let a = &self.docker.analysis;
        blockers(a, &self.docker_marked())
            .into_iter()
            .map(|(item, missing)| {
                let name = |id: &String| {
                    a.items
                        .get(id)
                        .map_or_else(|| id.clone(), |f| f.name.clone())
                };
                format!(
                    "{} is held by container(s) {} that are not marked (m marks them)",
                    name(&item),
                    missing.iter().map(name).collect::<Vec<_>>().join(", ")
                )
            })
            .collect()
    }

    /// Remember what the VM disk file takes, just before Docker commands run.
    pub(super) fn docker_note_vm_before(&mut self, ids: &[String]) {
        self.docker_vm_before = if ids.iter().any(|id| is_docker_id(id)) {
            vm_disk_allocated(self.policy.home())
        } else {
            None
        };
    }

    /// After Docker commands ran: the VM disk file before and after, and a
    /// fresh read of Docker.
    pub(super) fn docker_after_commands(&mut self, ran: bool) -> Option<(u64, u64)> {
        if !ran {
            return None;
        }
        self.reload_docker();
        let before = self.docker_vm_before.take()?;
        Some((before, vm_disk_allocated(self.policy.home()).unwrap_or(0)))
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
            self.docker_trim_housekeeping();
        } else {
            self.tool_marks.extend(ids);
            self.docker_marks_added();
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
        assert_eq!(
            marked,
            ["cache:child1", "cache:root1", "vm:trim"],
            "nolast is in use; the trim comes along"
        );
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

    // --- removing: review, order, trim, refusal, finish ----------------

    use crate::app::{Dialog, FinishInfo, Review};
    use crate::testutil::settle;

    const CTX: &str = "docker --context=desktop-linux";
    const TRIM: &str = "run --rm --pull=never --privileged --pid=host maccleanup/vm-tools";
    const BUILD: &str = "build -q -t maccleanup/vm-tools -";

    fn loaded_with(fake: Arc<FakeRunner>) -> crate::testutil::Env {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = fake;
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        e
    }

    fn mark(app: &mut App, key: &str) {
        cursor_on(app, key);
        app.update(Action::ToggleMark);
    }

    fn review_lines(app: &App) -> Vec<String> {
        match &app.dialog {
            Some(Dialog::Review(r)) => r.commands.iter().map(|c| c.line.clone()).collect(),
            other => panic!("no review: {other:?}"),
        }
    }

    fn review_of(app: &App) -> &Review {
        match &app.dialog {
            Some(Dialog::Review(r)) => r,
            other => panic!("no review: {other:?}"),
        }
    }

    fn finish_of(app: &App) -> &FinishInfo {
        match &app.dialog {
            Some(Dialog::Finish(f)) => f,
            other => panic!("no finish: {other:?}"),
        }
    }

    #[test]
    fn the_review_lists_removals_in_dependency_order_then_builds_and_trims() {
        let mut e = loaded_app();
        for key in [
            "volume:unmeasured",
            "image:sha256:bbb2",
            "container:c0ffee2",
        ] {
            mark(&mut e.app, key);
        }
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        let want = [
            "container rm c0ffee2".to_string(),
            "image rm sha256:bbb2".to_string(),
            "volume rm unmeasured".to_string(),
            BUILD.to_string(),
            TRIM.to_string(),
        ];
        let want: Vec<String> = want
            .iter()
            .map(|w| format!("docker --context=desktop-linux {w}"))
            .collect();
        assert_eq!(lines, want);
        let _ = CTX;
    }

    #[test]
    fn the_review_opens_from_the_docker_page_and_needs_y() {
        let mut e = loaded_app();
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        assert!(!review_of(&e.app).commands.is_empty());
        assert!(review_of(&e.app).items.is_empty());
    }

    #[test]
    fn an_existing_vm_tools_image_skips_the_build_step() {
        let fake = Arc::new(FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT),
            ("/_ping", "OK"),
            ("/system/df", DF),
            ("/containers/json?all=1", CONTAINERS),
            ("image inspect", "1"),
        ]));
        let mut e = loaded_with(fake);
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        assert!(lines.iter().all(|l| !l.contains("build")), "{lines:?}");
        assert!(lines.last().unwrap().contains("maccleanup/vm-tools"));
    }

    #[test]
    fn the_trim_is_added_with_a_removal_and_leaves_with_the_last_one() {
        let mut e = loaded_app();
        mark(&mut e.app, "container:c0ffee2");
        assert!(e.app.tool_marks.contains("vm:trim"));
        mark(&mut e.app, "container:c0ffee2");
        assert!(e.app.tool_marks.is_empty(), "{:?}", e.app.tool_marks);
    }

    #[test]
    fn a_trim_the_user_unmarked_is_not_added_again() {
        let mut e = loaded_app();
        mark(&mut e.app, "container:c0ffee2");
        e.app.toggle_docker_item("vm:trim");
        assert!(!e.app.tool_marks.contains("vm:trim"));
        mark(&mut e.app, "volume:unmeasured");
        assert!(!e.app.tool_marks.contains("vm:trim"), "they said no");
        e.app.update(Action::OpenReview);
        assert!(review_lines(&e.app).iter().all(|l| !l.contains("vm-tools")));
        e.app.update(Action::Cancel);
        // With nothing left marked, the next removal starts over.
        mark(&mut e.app, "container:c0ffee2");
        mark(&mut e.app, "volume:unmeasured");
        assert!(e.app.tool_marks.is_empty());
        mark(&mut e.app, "container:c0ffee2");
        assert!(e.app.tool_marks.contains("vm:trim"));
    }

    #[test]
    fn the_trim_alone_is_reviewed_and_run_without_any_removal() {
        let fake = healthy();
        let mut e = loaded_with(fake.clone());
        e.app.toggle_docker_item("vm:trim");
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        assert_eq!(lines.len(), 2, "build, then trim: {lines:?}");
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn confirming_runs_the_docker_commands_in_order_and_clears_their_marks() {
        let fake = healthy();
        let mut e = loaded_with(fake.clone());
        mark(&mut e.app, "image:sha256:bbb2");
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let ran: Vec<String> = fake.calls().iter().map(|c| c.display()).collect();
        assert_eq!(ran.len(), 4, "{ran:?}");
        assert!(ran[0].ends_with("container rm c0ffee2"), "{ran:?}");
        assert!(ran[1].ends_with("image rm sha256:bbb2"), "{ran:?}");
        assert!(ran[2].ends_with("-t maccleanup/vm-tools -"), "{ran:?}");
        assert!(ran[3].ends_with("maccleanup/vm-tools"), "{ran:?}");
        assert!(e.app.tool_marks.is_empty());
        assert_eq!(finish_of(&e.app).commands.len(), 4);
        assert_eq!(
            fake.calls()[2].stdin.as_deref(),
            Some(crate::docker::vmtools::DOCKERFILE)
        );
    }

    #[test]
    fn a_refused_removal_does_not_stop_the_rest() {
        let fake = Arc::new(
            FakeRunner::new(&["docker"])
                .answering(&[
                    ("context inspect", CONTEXT),
                    ("/_ping", "OK"),
                    ("/system/df", DF),
                    ("/containers/json?all=1", CONTAINERS),
                ])
                .failing(&["docker --context=desktop-linux container rm c0ffee2"]),
        );
        let mut e = loaded_with(fake);
        mark(&mut e.app, "container:c0ffee2");
        mark(&mut e.app, "volume:unmeasured");
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let oks: Vec<bool> = finish_of(&e.app).commands.iter().map(|c| c.ok).collect();
        assert_eq!(
            oks,
            [false, true, true, true],
            "container, volume, build, trim"
        );
    }

    #[test]
    fn an_image_whose_stopped_containers_are_not_marked_cannot_be_confirmed() {
        let fake = healthy();
        let mut e = loaded_with(fake.clone());
        mark(&mut e.app, "image:sha256:bbb2");
        e.app.update(Action::OpenReview);
        assert_eq!(review_of(&e.app).problems.len(), 1);
        assert!(
            review_of(&e.app).problems[0].contains("old"),
            "named by its name"
        );
        e.app.update(Action::Confirm);
        assert!(
            matches!(e.app.dialog, Some(Dialog::Review(_))),
            "still open"
        );
        assert!(e.app.notice.is_some());
        assert!(fake.calls().is_empty(), "nothing ran");
        // Mark the container too and it goes through.
        e.app.update(Action::Cancel);
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        assert!(review_of(&e.app).problems.is_empty());
    }

    #[test]
    fn the_docker_list_is_read_again_after_a_cleanup() {
        let mut e = loaded_app();
        let before = e.app.docker.job.spawned;
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        wait_docker(&mut e.app);
        assert!(e.app.docker.job.spawned > before);
    }

    #[test]
    fn the_finish_report_says_what_the_vm_disk_file_took_before_and_after() {
        let mut e = env(&[("a.txt", 10)]);
        let raw = e
            .home
            .join("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
        std::fs::create_dir_all(raw.parent().unwrap()).unwrap();
        std::fs::write(&raw, vec![1u8; 8192]).unwrap();
        e.app.runner = healthy();
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let (before, after) = finish_of(&e.app).vm_disk.expect("a Docker command ran");
        assert!(before >= 8192 && after >= 8192);
        // A cleanup with no Docker command reports none.
        e.app.update(Action::Cancel);
        assert!(finish_of_none(&e.app));
    }

    fn finish_of_none(app: &App) -> bool {
        !matches!(&app.dialog, Some(Dialog::Finish(f)) if f.vm_disk.is_some())
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
