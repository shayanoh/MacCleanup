//! The Docker page's behaviour on the App: loading in the background,
//! moving, folding, marking and sorting its list.

use std::collections::BTreeSet;

use super::*;
use crate::docker::analysis::{
    ItemKind, Refusal, TRIM_ITEM, blockers, frees_of_set, removal_order,
};
use crate::docker::browse::BrowseState;
use crate::docker::cli::{DockerCli, vm_disk_allocated};
use crate::docker::model::ItemId;
use crate::docker::state::DockerLoad;
use crate::docker::view::{DKind, DSection};
use crate::docker::vmtools::BrowseSource;

/// Is this key in the shared selection one of Docker's?
pub fn is_docker_id(id: &str) -> bool {
    ["image:", "container:", "volume:", "cache:", "vm:"]
        .iter()
        .any(|p| id.starts_with(p))
}

/// More than this many removals of one kind (containers, images, build-cache
/// records) are shown as a single review row: the review cannot scroll its
/// commands, and fifty rows would make it impossible to confirm. Volumes are
/// never folded together: each can hold data, so each is named.
const COLLAPSE_ABOVE: usize = 4;

/// Fold long runs of the same kind of removal into one row that still runs
/// every command (`ReviewCommand::group`).
fn collapse_bulk(items: Vec<(ItemKind, ReviewCommand, String)>) -> Vec<ReviewCommand> {
    let mut out: Vec<ReviewCommand> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let kind = items[i].0;
        let run: Vec<&(ItemKind, ReviewCommand, String)> = items[i..]
            .iter()
            .take_while(|(k, _, _)| *k == kind)
            .collect();
        i += run.len();
        let n = run.len();
        let foldable = matches!(
            kind,
            ItemKind::Container | ItemKind::Image | ItemKind::CacheRecord
        );
        if !foldable || n <= COLLAPSE_ABOVE {
            out.extend(run.iter().map(|(_, c, _)| c.clone()));
            continue;
        }
        let mut head = run[0].1.clone();
        head.up_to = run.iter().map(|(_, c, _)| c.up_to).sum();
        head.group = run[1..]
            .iter()
            .map(|(_, c, _)| (c.id.clone(), c.spec.clone()))
            .collect();
        let names = {
            let first: Vec<&str> = run.iter().take(3).map(|(_, _, n)| n.as_str()).collect();
            format!("{}{}", first.join(", "), if n > 3 { ", ..." } else { "" })
        };
        head.line = match kind {
            ItemKind::Container => format!("docker container rm: {n} containers ({names})"),
            ItemKind::Image => format!("docker image rm: {n} images ({names})"),
            _ => format!("docker builder prune: {n} build-cache records"),
        };
        out.push(head);
    }
    out
}

/// A Docker command as the review shows it: without the context flag
/// (noise) and with long image ids shortened. Only for display: what runs is
/// the spec itself.
pub fn display_line(spec: &crate::catalog::CommandSpec) -> String {
    spec.display()
        .split(' ')
        .filter(|w| !w.starts_with("--context="))
        .map(|w| match w.strip_prefix("sha256:") {
            Some(hex) if hex.len() > 12 => format!("sha256:{}\u{2026}", &hex[..12]),
            _ => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
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
                    self.docker_check_removals();
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

    /// After a cleanup: if Docker still lists something it was asked to
    /// remove, say so instead of letting "ok" stand (a prune filter that
    /// matched nothing, a record an image still holds...).
    fn docker_check_removals(&mut self) {
        self.docker.warning = None;
        let expected = std::mem::take(&mut self.docker_check_gone);
        let a = &self.docker.analysis;
        let still: Vec<String> = expected
            .into_iter()
            .filter_map(|id| a.items.get(&id).map(|f| f.name.clone()))
            .collect();
        if !still.is_empty() {
            let names: Vec<String> = still
                .iter()
                .take(3)
                .map(|n| crate::ui::docker::tidy_command(n))
                .collect();
            self.docker.warning = Some(format!(
                "Docker still lists {} item(s) it was asked to remove: {}{}",
                still.len(),
                names.join(", "),
                if still.len() > 3 { ", ..." } else { "" }
            ));
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

    /// `l` / Enter: open the folded line under the cursor; on an image or
    /// volume with nothing to unfold, look inside it.
    pub(super) fn docker_into(&mut self) {
        let Some(l) = self.docker.selected().cloned() else {
            return;
        };
        if l.open == Some(false) {
            self.docker_set_open(&l.key, true);
        } else if l.open.is_none() && matches!(l.kind, DKind::Image | DKind::Volume) {
            self.docker_browse();
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
        let needs_marked = needs.clone();
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
            return;
        }
        // All of them were already marked: pressing `m` again undoes it.
        let undone = needs_marked
            .iter()
            .filter(|id| self.tool_marks.remove(*id))
            .count();
        if undone > 0 {
            self.marks_changed();
            self.notice = Some(format!("Unmarked {undone} container(s)"));
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
            line: display_line(&spec),
            spec,
            up_to,
            path: PathBuf::new(),
            can_force: false,
            force: false,
            group: Vec::new(),
        };
        let mut out: Vec<(ItemKind, ReviewCommand, String)> = Vec::new();
        let mut trim = false;
        for id in removal_order(a, &marked) {
            if id == TRIM_ITEM {
                trim = true;
                continue;
            }
            let (Some(spec), Some(facts)) = (
                crate::docker::remove::removal_spec(&load.cli, data, &id),
                a.items.get(&id),
            ) else {
                continue;
            };
            let mut c = command(id.clone(), spec, facts.frees_alone);
            // Say which one it is: a name or a step, not only an id.
            let short: String = id
                .rsplit(':')
                .next()
                .unwrap_or("")
                .chars()
                .take(12)
                .collect();
            match facts.kind {
                ItemKind::Container => {
                    c.line = format!("docker container rm {} ({short})", facts.name);
                }
                ItemKind::CacheRecord => {
                    c.line = format!(
                        "docker builder prune: {} ({short})",
                        crate::ui::docker::tidy_command(&facts.name)
                    );
                }
                _ => {}
            }
            out.push((facts.kind, c, facts.name.clone()));
        }
        let mut out = collapse_bulk(out);
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

    /// Remember which Docker items the review is about to remove, so the
    /// next read can say if Docker still lists any of them.
    pub(super) fn docker_note_expected_gone(&mut self, commands: &[ReviewCommand]) {
        self.docker_expect_gone = commands
            .iter()
            .flat_map(|c| std::iter::once(&c.id).chain(c.group.iter().map(|(id, _)| id)))
            .filter(|id| is_docker_id(id) && !id.starts_with("vm:"))
            .cloned()
            .collect();
    }

    /// After Docker commands ran: the VM disk file before and after, and a
    /// fresh read of Docker.
    pub(super) fn docker_after_commands(&mut self, ran: bool) -> Option<(u64, u64)> {
        if !ran {
            return None;
        }
        self.docker_check_gone = std::mem::take(&mut self.docker_expect_gone);
        self.reload_docker();
        let before = self.docker_vm_before.take()?;
        Some((before, vm_disk_allocated(self.policy.home()).unwrap_or(0)))
    }

    /// Mark (or unmark) everything in a section that is safe to take in
    /// bulk: dangling images and unused build cache. Volumes never are.
    fn mark_docker_section(&mut self, section: DSection) {
        match section {
            DSection::Volumes => {
                self.notice = Some("Volumes hold data: mark them one at a time".to_string());
                return;
            }
            DSection::VmDisk => {
                self.notice =
                    Some("Mark the Shrink VM disk row itself (space on the row)".to_string());
                return;
            }
            _ => {}
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

impl App {
    /// `b` (or Enter on an image or volume with nothing to unfold): look
    /// inside it, read-only. Containers cannot be listed without starting
    /// them, so they are refused.
    pub(super) fn docker_browse(&mut self) {
        let Some(line) = self.docker.selected().cloned() else {
            return;
        };
        let name = self
            .docker
            .facts(&line.key)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        let (source, label) = match line.kind {
            DKind::Image => (
                BrowseSource::Image(line.key.strip_prefix("image:").unwrap_or(&line.key).into()),
                format!("image {name}"),
            ),
            DKind::Volume => (BrowseSource::Volume(name.clone()), format!("volume {name}")),
            DKind::Container => {
                self.notice = Some(
                    "A container cannot be listed without starting it: look inside its image or its volumes"
                        .to_string(),
                );
                return;
            }
            _ => {
                self.notice = Some("Only images and volumes can be looked into".to_string());
                return;
            }
        };
        let Some(load) = &self.docker.load else {
            return;
        };
        let (cli, ready) = (load.cli.clone(), load.vm_tools);
        let mut state = BrowseState::new(source, label);
        state.start(&cli, "/", ready);
        self.docker.browse = Some(state);
        self.goto(Screen::DockerFiles);
        self.focus = Focus::Main;
    }

    /// Esc in the file browser: back to the Docker page (no new read).
    pub(super) fn docker_close_browse(&mut self) {
        if self.screen != Screen::DockerFiles {
            return;
        }
        self.reset_filter();
        self.filter.editing = false;
        self.screen = Screen::Docker;
        self.transition_start = self.tick;
        self.docker_refilter();
    }

    fn list_dir(&mut self, dir: &str) {
        let Some(load) = &self.docker.load else {
            return;
        };
        let (cli, ready) = (load.cli.clone(), load.vm_tools);
        if let Some(b) = &mut self.docker.browse {
            let ready = ready || b.listed_once;
            b.start(&cli, dir, ready);
        }
    }

    pub(super) fn browse_move(&mut self, delta: i64) {
        if let Some(b) = &mut self.docker.browse {
            b.cursor = step(b.cursor, delta, b.shown.len());
        }
        self.ensure_visible();
    }

    /// A click on list row `row`: select it, or open it when it is already
    /// selected.
    pub(super) fn browse_click(&mut self, row: usize) {
        let Some(b) = &mut self.docker.browse else {
            return;
        };
        let idx = b.offset + row;
        if idx >= b.shown.len() {
            return;
        }
        if idx == b.cursor {
            self.browse_into();
        } else {
            b.cursor = idx;
        }
    }

    /// `s` in the file browser: the next sort column.
    pub(super) fn browse_cycle_sort(&mut self) {
        let query = crate::filter::parse(&self.filter.query);
        if let Some(b) = &mut self.docker.browse {
            b.cycle_sort(&query);
        }
        self.ensure_visible();
    }

    pub(super) fn browse_refilter(&mut self) {
        let query = crate::filter::parse(&self.filter.query);
        if let Some(b) = &mut self.docker.browse {
            b.refilter(&query);
        }
        self.ensure_visible();
    }

    pub(super) fn browse_reload(&mut self) {
        let dir = self.docker.browse.as_ref().map(|b| b.dir.clone());
        if let Some(dir) = dir {
            self.list_dir(&dir);
        }
    }

    /// Open the folder under the cursor (files do nothing).
    pub(super) fn browse_into(&mut self) {
        let next = self.docker.browse.as_ref().and_then(|b| {
            b.selected()
                .filter(|e| e.is_dir)
                .map(|e| b.child_dir(&e.name))
        });
        if let Some(dir) = next {
            self.reset_filter();
            self.list_dir(&dir);
        }
    }

    /// Up one folder; at the top, back to the Docker page.
    pub(super) fn browse_out(&mut self) {
        let up = self
            .docker
            .browse
            .as_ref()
            .map(|b| (b.dir != "/").then(|| b.parent_dir()));
        match up {
            Some(Some(dir)) => {
                self.reset_filter();
                self.list_dir(&dir);
            }
            Some(None) => self.docker_close_browse(),
            None => {}
        }
    }

    pub(super) fn poll_browse(&mut self) {
        let query = crate::filter::parse(&self.filter.query);
        if let Some(b) = &mut self.docker.browse {
            b.poll(&query);
        }
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
        assert_eq!(e.app.notice.as_deref(), Some("Unmarked 1 container(s)"));
        assert!(!e.app.tool_marks.contains("container:c0ffee2"));
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
            "container rm old (c0ffee2)".to_string(),
            "image rm sha256:bbb2".to_string(),
            "volume rm unmeasured".to_string(),
            BUILD.to_string(),
            TRIM.to_string(),
        ];
        let want: Vec<String> = want.iter().map(|w| format!("docker {w}")).collect();
        assert_eq!(lines, want);
        let _ = CTX;
    }

    #[test]
    fn the_review_line_names_the_target_and_drops_the_context_flag() {
        let mut e = loaded_app();
        mark(&mut e.app, "container:c0ffee2");
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        assert_eq!(
            lines[0], "docker container rm old (c0ffee2)",
            "the container's name, then its id"
        );
        assert!(lines.iter().all(|l| !l.contains("--context")), "{lines:?}");
    }

    #[test]
    fn a_cache_record_is_shown_by_its_step_not_only_its_id() {
        let mut e = loaded_app();
        mark(&mut e.app, "cache:child1");
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        assert!(
            lines[0].contains("apt-get update") && lines[0].contains("(child1)"),
            "{lines:?}"
        );
    }

    /// A Docker whose build cache holds `n` unused records.
    fn many_records(n: usize) -> Arc<FakeRunner> {
        let recs: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    r#"{{"ID":"rec{i:02}","Type":"regular","Description":"step {i}","InUse":false,"Shared":false,"Size":1000,"CreatedAt":"2026-10-23T14:51:03Z","LastUsedAt":"2026-10-23T15:00:23Z","UsageCount":1}}"#
                )
            })
            .collect();
        let df = format!(
            r#"{{"LayersSize":0,"Images":[],"Volumes":[],"BuildCache":[{}]}}"#,
            recs.join(",")
        );
        Arc::new(FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT),
            ("/_ping", "OK"),
            ("/system/df", Box::leak(df.into_boxed_str())),
            ("/containers/json?all=1", "[]"),
        ]))
    }

    #[test]
    fn a_bulk_mark_of_many_records_is_one_reviewable_row_that_runs_every_removal() {
        let fake = many_records(12);
        let mut e = loaded_with(fake.clone());
        cursor_on(&mut e.app, "section:cache");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        let r = review_of(&e.app);
        // The records, plus the helper build and the trim: three rows, not fourteen.
        assert!(r.commands.len() <= 3, "{:?}", review_lines(&e.app));
        assert!(
            r.commands[0].line.contains("12 build-cache records"),
            "{}",
            r.commands[0].line
        );
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let ran: Vec<String> = fake.calls().iter().map(|c| c.display()).collect();
        for i in 0..12 {
            let want = format!("--filter id=rec{i:02}");
            assert!(ran.iter().any(|c| c.contains(&want)), "{want} in {ran:?}");
        }
        assert_eq!(ran.len(), 14, "twelve prunes, the build, the trim");
        assert_eq!(finish_of(&e.app).commands.len(), 14, "every one reported");
        assert!(e.app.tool_marks.is_empty());
    }

    #[test]
    fn a_few_records_are_still_listed_one_by_one() {
        let mut e = loaded_with(many_records(3));
        cursor_on(&mut e.app, "section:cache");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        let lines = review_lines(&e.app);
        assert_eq!(
            lines.iter().filter(|l| l.contains("builder prune")).count(),
            3,
            "{lines:?}"
        );
    }

    /// A Docker that really loses a container once a command has run.
    struct Vanishing {
        removed: std::sync::atomic::AtomicBool,
        really: bool,
    }

    impl crate::catalog::CommandRunner for Vanishing {
        fn run(
            &self,
            _: &crate::catalog::CommandSpec,
            _: &crate::catalog::RunControl,
        ) -> crate::catalog::CommandResult {
            self.removed
                .store(self.really, std::sync::atomic::Ordering::SeqCst);
            crate::catalog::CommandResult {
                ok: true,
                cancelled: false,
                output_tail: String::new(),
            }
        }
        fn query(&self, spec: &crate::catalog::CommandSpec) -> Option<String> {
            let full = spec.full();
            let gone = self.removed.load(std::sync::atomic::Ordering::SeqCst);
            let containers = if gone {
                "[]"
            } else {
                r#"[{"Id":"old1","Names":["/old"],"ImageID":"sha256:bbb2","State":"exited","Created":1,"Mounts":[]}]"#
            };
            let df = r#"{"LayersSize":0,"Images":[],"Volumes":[],"BuildCache":[]}"#;
            if full.contains("context inspect") {
                Some(CONTEXT.to_string())
            } else if full.contains("/_ping") {
                Some("OK".to_string())
            } else if full.contains("/system/df") {
                Some(df.to_string())
            } else if full.contains("/containers/json?all=1") {
                Some(containers.to_string())
            } else if full.contains("/containers/old1/json") {
                Some("{}".to_string())
            } else {
                None
            }
        }
        fn available(&self, program: &str) -> bool {
            program.ends_with("docker")
        }
    }

    fn removal_run(really: bool) -> App {
        let mut e = env(&[("a.txt", 10)]);
        e.app.runner = Arc::new(Vanishing {
            removed: std::sync::atomic::AtomicBool::new(false),
            really,
        });
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        mark(&mut e.app, "container:old1");
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        wait_docker(&mut e.app);
        // Keep the tempdir alive until here by leaking the Env's drop order.
        std::mem::forget(e._dir);
        e.app
    }

    #[test]
    fn a_removal_docker_ignored_is_reported_not_called_ok() {
        let app = removal_run(false);
        let note = app.docker.warning.clone().unwrap_or_default();
        assert!(note.contains("still lists"), "{note:?}");
        assert!(note.contains("old"), "{note:?}");
    }

    #[test]
    fn a_removal_that_really_happened_raises_no_alarm() {
        let app = removal_run(true);
        assert!(
            !app.docker
                .warning
                .clone()
                .unwrap_or_default()
                .contains("still lists"),
            "{:?}",
            app.docker.warning
        );
    }

    #[test]
    fn long_image_ids_are_shortened_for_display_only() {
        let id = "sha256:063f36aae39753ad4d99f8ba34dbc828bb9b419819959ad1d2c2d1d04af239d4";
        let spec = crate::catalog::CommandSpec::new(
            "/usr/local/bin/docker",
            &["--context=desktop-linux", "image", "rm", id],
        );
        assert_eq!(
            display_line(&spec),
            "docker image rm sha256:063f36aae397\u{2026}"
        );
        assert_eq!(spec.args[3], id, "what runs is untouched");
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
            ("image inspect", crate::docker::vmtools::VERSION),
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

    /// An app whose home holds a Docker.raw, so the trim row exists.
    fn app_with_vm_disk(fake: Arc<FakeRunner>) -> crate::testutil::Env {
        let mut e = env(&[("a.txt", 10)]);
        let raw = e
            .home
            .join("Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw");
        std::fs::create_dir_all(raw.parent().unwrap()).unwrap();
        std::fs::write(&raw, vec![1u8; 8192]).unwrap();
        e.app.runner = fake;
        e.app.update(Action::GoTo(Screen::Docker));
        wait_docker(&mut e.app);
        e
    }

    #[test]
    fn the_trim_row_alone_runs_only_the_build_and_the_trim() {
        let fake = healthy();
        let mut e = app_with_vm_disk(fake.clone());
        cursor_on(&mut e.app, "vm:trim");
        e.app.update(Action::ToggleMark);
        assert_eq!(e.app.docker_marked().len(), 1);
        e.app.update(Action::OpenReview);
        assert_eq!(review_lines(&e.app).len(), 2);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let ran: Vec<String> = fake.calls().iter().map(|c| c.display()).collect();
        assert!(
            ran[0].contains("build") && ran[1].contains("--privileged"),
            "{ran:?}"
        );
        let (before, after) = finish_of(&e.app).vm_disk.unwrap();
        assert_eq!(
            before, after,
            "nothing changed in the fake: zero is reported as zero"
        );
    }

    #[test]
    fn the_vm_disk_section_is_never_bulk_marked() {
        let mut e = app_with_vm_disk(healthy());
        cursor_on(&mut e.app, "section:vm");
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
        assert!(e.app.notice.is_some());
    }

    #[test]
    fn unmarking_the_trim_row_with_removals_marked_declines_it() {
        let mut e = app_with_vm_disk(healthy());
        mark(&mut e.app, "container:c0ffee2");
        assert!(e.app.tool_marks.contains("vm:trim"));
        mark(&mut e.app, "vm:trim");
        assert!(!e.app.tool_marks.contains("vm:trim"));
        assert!(e.app.docker.trim_declined);
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

    // --- browsing inside volumes and images ---------------------------

    const ROOT_LISTING: &str =
        "d\t4096\t1700000000\tsub\0f\t12\t1700000100\tb.txt\0f\t5\t1700000200\ta.txt\0";
    const SUB_LISTING: &str = "f\t7\t1700000300\tinner.txt\0";

    fn browsing_fake() -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT),
            ("/_ping", "OK"),
            ("/system/df", DF),
            ("/containers/json?all=1", CONTAINERS),
            ("/images/sha256:aaa1/history", HISTORY),
            ("/images/sha256:aaa1/json", INSPECT),
            // The longer path first: the first matching key answers.
            ("sh /data/sub", SUB_LISTING),
            ("sh /data", ROOT_LISTING),
            ("--entrypoint sh sha256:bbb2 -c", ROOT_LISTING),
        ]))
    }

    fn wait_browse(app: &mut App) {
        for _ in 0..2000 {
            app.update(Action::Tick);
            if app.docker.browse.as_ref().is_some_and(|b| !b.loading()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("listing did not finish");
    }

    fn browse_names(app: &App) -> Vec<String> {
        let b = app.docker.browse.as_ref().unwrap();
        b.shown.iter().map(|&i| b.all[i].name.clone()).collect()
    }

    #[test]
    fn b_opens_the_volume_browser_and_lists_it_in_the_background() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        assert_eq!(e.app.screen, Screen::DockerFiles);
        assert!(e.app.docker.browse.as_ref().unwrap().loading());
        wait_browse(&mut e.app);
        assert_eq!(
            browse_names(&e.app),
            ["sub", "b.txt", "a.txt"],
            "biggest first, like the Explorer"
        );
        let b = e.app.docker.browse.as_ref().unwrap();
        assert_eq!(b.dir, "/");
        assert_eq!(b.all[0].size, 4096);
    }

    #[test]
    fn enter_descends_into_a_folder_and_h_climbs_back() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.update(Action::Into); // the cursor is on "sub"
        wait_browse(&mut e.app);
        assert_eq!(browse_names(&e.app), ["inner.txt"]);
        assert_eq!(e.app.docker.browse.as_ref().unwrap().dir, "/sub");
        e.app.update(Action::Out);
        wait_browse(&mut e.app);
        assert_eq!(e.app.docker.browse.as_ref().unwrap().dir, "/");
        assert_eq!(browse_names(&e.app).len(), 3);
        // A file cannot be entered.
        e.app.update(Action::Down);
        e.app.update(Action::Into);
        assert_eq!(e.app.docker.browse.as_ref().unwrap().dir, "/");
    }

    #[test]
    fn s_changes_the_sort_column_in_the_file_browser_without_listing_again() {
        let fake = browsing_fake();
        let mut e = loaded_with(fake.clone());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        let builds = fake.calls().len();
        e.app.update(Action::CycleSort);
        assert_eq!(
            browse_names(&e.app),
            ["a.txt", "b.txt", "sub"],
            "modified: newest first"
        );
        e.app.update(Action::CycleSort);
        assert_eq!(browse_names(&e.app), ["a.txt", "b.txt", "sub"], "name");
        e.app.update(Action::CycleSort);
        assert_eq!(
            browse_names(&e.app),
            ["sub", "b.txt", "a.txt"],
            "size again"
        );
        assert_eq!(fake.calls().len(), builds, "re-sorting reads nothing");
        assert!(!e.app.docker.browse.as_ref().unwrap().loading());
    }

    #[test]
    fn the_cursor_stays_on_the_same_entry_when_the_sort_changes() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.update(Action::Down); // b.txt
        e.app.update(Action::CycleSort);
        let b = e.app.docker.browse.as_ref().unwrap();
        assert_eq!(b.selected().unwrap().name, "b.txt");
    }

    #[test]
    fn closing_the_browser_returns_to_the_docker_page_without_reading_again() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        let reads = e.app.docker.job.spawned;
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.update(Action::CloseBrowse);
        assert_eq!(e.app.screen, Screen::Docker);
        assert_eq!(e.app.docker.job.spawned, reads);
    }

    #[test]
    fn containers_layers_and_sections_cannot_be_browsed() {
        let mut e = loaded_with(browsing_fake());
        for key in ["container:c0ffee2", "layer:sha256:aaa1:0", "section:cache"] {
            cursor_on(&mut e.app, key);
            e.app.update(Action::Browse);
            assert_eq!(e.app.screen, Screen::Docker, "{key}");
            assert!(e.app.notice.is_some(), "{key}");
        }
    }

    #[test]
    fn browsing_never_marks_reviews_or_removes_anything() {
        let fake = browsing_fake();
        let mut e = loaded_with(fake.clone());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        assert!(e.app.tool_marks.is_empty());
        assert!(e.app.dialog.is_none());
        assert!(
            fake.calls().iter().all(|c| !c.display().contains(" rm ")),
            "only the helper build ran"
        );
    }

    #[test]
    fn an_image_is_listed_from_the_image_itself_and_never_builds_the_helper() {
        let fake = browsing_fake();
        let mut e = loaded_with(fake.clone());
        cursor_on(&mut e.app, "image:sha256:bbb2");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        assert_eq!(browse_names(&e.app), ["sub", "b.txt", "a.txt"]);
        assert!(
            fake.calls().is_empty(),
            "no helper image needed for an image"
        );
    }

    #[test]
    fn a_missing_helper_image_is_built_before_a_volume_is_listed() {
        let fake = browsing_fake();
        let mut e = loaded_with(fake.clone());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        let ran = fake.calls();
        assert_eq!(ran.len(), 1);
        assert!(
            ran[0]
                .display()
                .contains("build -q -t maccleanup/vm-tools -")
        );
    }

    #[test]
    fn a_listing_that_fails_shows_why_and_keeps_the_page_usable() {
        let fake = Arc::new(FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT),
            ("/_ping", "OK"),
            ("/system/df", DF),
            ("/containers/json?all=1", CONTAINERS),
        ]));
        let mut e = loaded_with(fake);
        cursor_on(&mut e.app, "image:sha256:bbb2");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        let b = e.app.docker.browse.as_ref().unwrap();
        assert!(b.error.as_deref().unwrap().contains("no shell"));
        e.app.update(Action::CloseBrowse);
        assert_eq!(e.app.screen, Screen::Docker);
    }

    #[test]
    fn the_filter_narrows_the_listing_and_clears_when_leaving() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.filter.query = "txt".to_string();
        e.app.refilter(false);
        assert_eq!(browse_names(&e.app), ["b.txt", "a.txt"]);
        e.app.update(Action::CloseBrowse);
        assert!(!e.app.filter.active());
    }

    #[test]
    fn the_sidebar_still_counts_docker_as_the_current_page_while_browsing() {
        let mut e = loaded_with(browsing_fake());
        cursor_on(&mut e.app, "volume:pgdata");
        e.app.update(Action::Browse);
        wait_browse(&mut e.app);
        e.app.update(Action::FocusToggle);
        e.app.update(Action::Down);
        assert_eq!(
            e.app.screen,
            Screen::Snapshots,
            "down from Docker, not from Home"
        );
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
