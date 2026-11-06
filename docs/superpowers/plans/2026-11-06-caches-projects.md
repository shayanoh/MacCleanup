# Caches & Projects Implementation Plan

> **Status: implemented and merged.** Requests made while using it (cancelling commands and `--force`, products kept apart, workspaces, toolchain detection, npm as two items, a graph per project, the group-aware filter) went beyond this plan; section 11 of the design records them.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One new screen listing project build artifacts and tool caches, with a joint (set-wise) "frees" figure, a detail pane naming related folders, one selection shared with the Explorer, and tool cleanup commands in the same review.

**Architecture:** A pure `catalog` module finds candidates (artifact detection over the scan tree, a hard-coded tool registry, a command runner behind a trait). A `targets` module turns candidates plus scan numbers into sections, groups and rows (reusing `scan::Row`). `App` gets `Screen::Targets`, tool-command marks beside the existing `marks`, and a background worker for set-wise space (`Scanner::space_of_set`). The UI reuses the Explorer's column helpers and graph pane through small extractions. Commands run in a new job after the folder-move job.

**Tech Stack:** Rust 2024, ratatui 0.30 (use `ratatui::crossterm`), rayon, tempfile for tests. No new crates are expected; any addition uses `cargo add`.

**Spec:** `docs/superpowers/specs/2026-11-06-caches-projects-design.md` (parent: `2026-11-06-maccleanup-design.md`). Standing rules: root `CLAUDE.md`.

## Global Constraints

- Format with `cargo fmt`; `cargo clippy --all-targets -- -D warnings` clean; `cargo test` green before every commit.
- Nothing slow on the UI thread. Set-wise space is computed on a worker thread; commands run in a job thread.
- Deleting always goes through the review screen. Enter never confirms an irreversible action. A review containing any tool command needs an explicit `y`.
- Bulk marks (group or section) mark folder rows only; a command row is marked only by marking that row.
- One selection: `App::marks` (folders) plus `App::tool_marks` (command rows). No screen has its own delete list.
- Row Frees column = what deleting that row alone frees (same as Explorer). The joint figure is shown in the footer, headers, detail pane and review.
- Set Frees = `frees_now + shared_inside` of `Scanner::space_of_set` (a clone set whose every holder is inside the set frees fully).
- All filesystem tests run in `tempfile` sandboxes with a throwaway `HOME`. Tests that need clones use `cp -c` (`testutil::clone_file`). Tool commands are tested only through `CommandRunner` fakes; never run a real cleanup command in tests.
- Never escalate privileges. No shell: commands are program plus explicit args.
- Key facts from the code: graph cycle key is `t` (not `g`); `g`/`G` are top/bottom; `Tab` is menu focus; `h`/`l`/`Enter` map to `Action::Out`/`Into`; sort cycle is Name, Size, On disk, Frees.
- Update `CLAUDE.md` code map and `README.md` key table in the same change as the feature (Task 9 finishes this; earlier tasks add their own lines).

## Review Focus

Failure modes the spec implies that no single task's happy path covers (each has its test in the owning task):

1. A `node_modules` with no `package.json` sibling must never be offered (Task 2).
2. A tool whose binary or cache folder is absent must not show a row or crash (Task 3).
3. Marking a parent in the Explorer after marking its child here must replace the child mark, with a notice (Task 6).
4. Unmarking the one holder that keeps clone data alive must lower the set figure and the "also holds this data" list must show it unmarked (Tasks 1, 6).
5. A failing command must not roll back the moved folders or stop later commands (Task 7).
6. A terminal at minimum size, `--ascii`, and no-scan-yet states must render without panics (Task 8).

---

## File Structure

- Create `src/catalog/mod.rs` (re-exports), `src/catalog/artifacts.rs` (project artifact detection), `src/catalog/tools.rs` (tool registry), `src/catalog/command.rs` (`CommandSpec`, `CommandRunner`, `SystemRunner`).
- Create `src/targets.rs` (sections, groups, rows, flattening, sorting, sharing info).
- Create `src/ui/targets.rs` (drawing). Modify `src/ui/explorer.rs` (extract column helpers), `src/ui/graph.rs` (take graph data instead of reading `app.rows`), `src/ui/dialogs.rs`, `src/ui/mod.rs`, `src/ui/sidebar.rs`.
- Modify `src/scan.rs` (set-wise space), `src/app.rs` (screen, state, selection, review), `src/jobs.rs` (command job), `src/input.rs`, `src/glyphs.rs`, `src/lib.rs`, `src/testutil.rs`, `README.md`, `CLAUDE.md`, `scripts/smoke_pty.py`.

---

### Task 1: Set-wise space in the scanner

**Files:** Modify `src/scan.rs`. Test in the same file's `mod tests`.

**Interfaces:**
- Produces: `impl Space { pub fn frees_all(&self) -> u64 }` (= `frees_now + shared_inside`); `pub struct SpaceView` (cloneable, `Send + Sync`, holds `Arc<Shared>`); `Scanner::view(&self) -> SpaceView`; `SpaceView::space_of_set(&self, nodes: &[Arc<Node>]) -> Space`; `Scanner::space_of_set(&self, nodes: &[Arc<Node>]) -> Space` (delegates); `Scanner::spaces_of(&self, nodes: &[Arc<Node>]) -> Vec<Space>` (one pass for many disjoint nodes, locating a path by walking its ancestors in a `HashMap<&Path, usize>`).

- [ ] **Step 1: Write failing tests** (in `scan.rs` tests, using `write`, `clone_file` helpers there):
  - `set_frees_clone_data_only_when_every_holder_is_in_the_set`: sandbox with `store/f` (200 KB) and clones `a/f`, `b/f` of it (3 holders). Scan. Assert for nodes: `space_of_set([a]).frees_all() == 0`; `[a,b]` is 0; `[store,a]` is 0 (b still holds); `[store,a,b].frees_all() >= 200_000` and `shared_outside == 0`.
  - `set_space_identity_holds`: for `[store,a]`, `footprint == frees_now + snapshots + shared_inside + shared_outside`.
  - `spaces_of_matches_space_for_disjoint_nodes`: `spaces_of([a,b])[i] == space(node_i)` for both.
  - `a_set_of_one_node_equals_space`: `space_of_set([n]) == space(n)`.
- [ ] **Step 2:** `cargo test set_ spaces_of a_set_of_one` → FAIL (not defined).
- [ ] **Step 3: Implement.** Refactor `Scanner::compute` into a free `fn compute_with(shared: &Shared, seeds: Vec<Acc>, locate: &dyn Fn(&Path) -> Option<usize>) -> Vec<Space>`; keep `Scanner::compute` building seeds from nodes (`Acc { disk: n.disk(), private: n.private(), snap: n.snapshot_held(), ..}`) and calling it. `space_of_set` builds ONE seed that sums `disk/private/snap` over the (disjoint) nodes and a `locate` returning `Some(0)` for any path under any node (check path ancestors against a `HashSet<&Path>`). `spaces_of` uses per-node seeds and ancestor lookup. `frees_all` is a plain method on `Space`.
- [ ] **Step 4:** run the new tests, then `cargo test scan::` → PASS.
- [ ] **Step 5: Commit** `feat: set-wise space for a group of folders (SpaceView, space_of_set, spaces_of)`.

---

### Task 2: Project artifact detection

**Files:** Create `src/catalog/mod.rs`, `src/catalog/artifacts.rs`; modify `src/lib.rs` (`pub mod catalog;`).

**Interfaces:**
- Produces: `pub enum ArtifactKind { NodeModules, Next, Turbo, RustTarget, XcodeBuild, DartTool, Pods, FlutterBuild }` with `fn label(&self) -> &'static str`, `fn regen(&self) -> &'static str` (e.g. "pnpm install"), `fn dir_name(&self) -> &'static str`; `pub struct Candidate { pub path: PathBuf, pub node: Arc<Node>, pub kind: ArtifactKind, pub project: PathBuf }`; `pub fn detect_artifacts(root: &Arc<Node>) -> Vec<Candidate>`.
- Markers (spec section 6): `package.json` for NodeModules/Next/Turbo; `Cargo.toml` for RustTarget; `pubspec.yaml` for DartTool and FlutterBuild (`build`); `Podfile` for Pods. XcodeBuild means `<project>/build` next to a `*.xcodeproj` or `*.xcworkspace` (entry name ends with one of these); keep it only if cheap, else omit and note in the commit (spec allows listing is "my call").

- [ ] **Step 1: Write failing tests** (tempdir tree + `Scanner::new(2)` + `scan_root` + `wait_idle`): `finds_node_modules_next_to_package_json`; `ignores_node_modules_without_package_json` (Review Focus 1); `finds_rust_target_next_to_cargo_toml`; `does_not_list_nested_node_modules_inside_node_modules`; `project_is_the_parent_folder`; `flutter_build_and_dart_tool_need_pubspec`; `ignores_a_file_named_target`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3: Implement** `detect_artifacts` as a recursive walk over `Node::children()` that, for each directory, checks `name` against the kinds and the marker via `Path::exists` on `parent.join(marker)`, and does not descend into a matched directory. Match is by exact directory name.
- [ ] **Step 4:** tests PASS. **Step 5: Commit** `feat: detect project build artifacts from the scan tree`.

---

### Task 3: Tool registry and command runner

**Files:** Create `src/catalog/tools.rs`, `src/catalog/command.rs`; modify `src/catalog/mod.rs`.

**Interfaces:**
- `pub struct CommandSpec { pub program: String, pub args: Vec<String>, pub label: String }` with `fn display(&self) -> String` ("pnpm store prune").
- `pub trait CommandRunner: Send + Sync { fn run(&self, spec: &CommandSpec) -> CommandResult; fn available(&self, program: &str) -> bool; }`, `pub struct CommandResult { pub ok: bool, pub output_tail: String }`, `pub struct SystemRunner` (runs with `std::process::Command`, no shell, captures stdout+stderr tail of at most 2000 chars; `available` searches `PATH` entries for an executable).
- `pub enum Mode { Folder, Command(CommandSpec) }`.
- `pub struct ToolEntry { pub id: &'static str, pub tool: &'static str, pub label: &'static str, pub paths: Vec<PathBuf>, pub safety_note: &'static str (regen text), pub mode: Mode }`.
- `pub fn tool_registry(home: &Path) -> Vec<ToolEntry>`; `pub fn present(entries: Vec<ToolEntry>, runner: &dyn CommandRunner) -> Vec<ToolEntry>` keeps entries with at least one existing path and, for `Mode::Command`, an available program (otherwise downgrades the entry to `Mode::Folder`).

- [ ] **Step 1: Verify commands against the installed tools** (not from memory): run each tool's `--help` and record the exact clean command in the registry comment: `pnpm store prune`, `npm cache clean --force`, `yarn cache clean`, `go clean -cache` (and `-modcache` as a separate entry), `uv cache clean`, `pip cache purge`, `brew cleanup`, `flutter pub cache clean` (check availability), and folder-only entries: cargo registry, `~/Library/Caches` per-app folders, `~/Library/Developer/Xcode/DerivedData`, Playwright (`~/Library/Caches/ms-playwright`), `~/.cache`. Per the spec, command mode only where a documented command exists; Xcode DerivedData is folder mode.
- [ ] **Step 2: Write failing tests** with a `FakeRunner` (records specs; configurable `available`): `registry_paths_live_under_home`; `entries_with_no_existing_path_are_dropped` (Review Focus 2); `command_entry_downgrades_to_folder_when_program_missing`; `command_spec_display_joins_program_and_args`; `registry_paths_are_disjoint` (no entry path is inside another entry's path); `system_runner_runs_true_and_reports_failure_for_false` (`true`/`false` binaries only); `system_runner_never_uses_a_shell` (an argument containing `; echo hi` is passed literally: run `echo` and assert output).
- [ ] **Step 3: Implement.** **Step 4:** tests PASS. **Step 5: Commit** `feat: tool cache registry and command runner`.

---

### Task 4: The target list model

**Files:** Create `src/targets.rs`; modify `src/lib.rs`.

**Interfaces:**
- Consumes: `catalog::{detect_artifacts, Candidate, ArtifactKind, ToolEntry, Mode, CommandSpec}`, `Scanner::spaces_of`, `scan::{Row, Space, Node}`, `Policy::classify`.
- Produces:
  - `pub enum SectionKind { Projects, Tools }`
  - `pub enum RowKind { Folder, Command { id: &'static str, spec: CommandSpec } }`
  - `pub struct TRow { pub row: Row, pub kind: RowKind, pub group: usize, pub regen: &'static str, pub safety: Safety }` (`row.frees` standalone; for command rows `row.frees` is the "up to" figure).
  - `pub struct Group { pub title: String, pub subtitle: String, pub section: SectionKind, pub rows: Vec<TRow>, pub open: bool }`
  - `pub struct Targets { pub groups: Vec<Group> }` with `Targets::build(root: &Arc<Node>, home: &Path, scanner: &Scanner, policy: &Policy, runner: &dyn CommandRunner) -> Targets`, `fn sort(&mut self, by: SortBy)`, `fn lines(&self, filter: &Query) -> Vec<Line>` where `pub enum Line { Section(SectionKind), Group(usize), Row { group: usize, row: usize } }` (closed groups hide their rows; groups with no filter match are skipped), `fn row(&self, g: usize, r: usize) -> &TRow`, `fn find(&self, path: &Path) -> Option<(usize, usize)>`, `fn sharers(&self, root: &Arc<Node>, scanner: &Scanner, path: &Path) -> Vec<PathBuf>` is NOT here (see Task 6 detail).
- Rules: a path appears once (registry wins on equal path; otherwise outer wins; nested candidates absorbed); project groups are titled by the project folder (display path with `~`), tool groups by tool; group order follows the sort key on group totals, sections fixed (Projects, then Tools); `SortBy` imported from `app`.

- [ ] **Step 1: Failing tests** (sandbox home with projects `shop` (package.json + node_modules + .next), `api` (Cargo.toml + target), and tool cache folders under `Library/Caches/...`; `FakeRunner`): `groups_projects_by_folder_and_tools_by_tool`; `registry_wins_over_artifact_on_the_same_path`; `nested_candidates_are_absorbed`; `row_numbers_match_the_scan` (`row.size`, `row.disk` footprint, `row.frees` from `Space.frees_now`); `sorting_orders_rows_in_each_group_and_groups_by_total`; `sections_stay_in_fixed_order_under_every_sort`; `closed_group_hides_its_rows`; `filter_hides_groups_without_matches`; `command_rows_carry_their_spec`.
- [ ] **Step 2:** FAIL. **Step 3: Implement** using `Row` built from the node (`size`, `disk`, `files`, `scanning`, `node`, `space`, `frees`). **Step 4:** PASS. **Step 5: Commit** `feat: target list model (sections, groups, rows)`.

---

### Task 5: Screen, state and navigation

**Files:** Modify `src/app.rs`, `src/input.rs`, `src/ui/sidebar.rs` (only the entry list in `app.rs`), `src/glyphs.rs` (`nav` array stays 8 long; no change needed if entries keep their index), `src/testutil.rs`.

**Interfaces:**
- `Screen::Targets`; `SIDEBAR` becomes 7 entries: Home, Explorer, Leftovers, Uninstall, `Caches & Projects` (`Some(Screen::Targets)`), Docker, ToBeDeleted. The sidebar `move_by` order becomes `[Home, Explorer, Targets, Holding]`; key `4` goes to Targets; the footer hint text updates ("1/2/3/4").
- App state: `pub targets: Targets`, `pub tlines: Vec<targets::Line>`, `pub tcursor: usize`, `pub toffset: usize`, `pub runner: Arc<dyn CommandRunner>` (set by `App::new`; `App::with_runner` for tests), `fn refresh_targets(&mut self)` (rebuild on goto, on rescan completion, every 10 ticks while scanning, keeping the cursor on the same path), `pub fn selected_target(&self) -> Option<&TRow>`.
- Behavior: `Up/Down/PageUp/PageDown/Top/Bottom/Scroll` move over `tlines` (headers are selectable lines); `Out` (`h`) on a row jumps to its group header and folds it, on a header folds; `Into` (`l`/`Enter`) on a header unfolds, on a row opens that path in the Explorer (`goto(Explorer)` with `stack` set to the row's parent node and `pending_select` set to the row path); `CycleSort` cycles the same four keys and rebuilds the order; `CycleGraph`, `GrowGraph`, `ShrinkGraph` work as in the Explorer; `FilterOpen` (`/`) works on this screen too with its own query held in the existing `filter` state (cleared when leaving the screen); `Click` selects a line.
- Constraint: keep `Action` variants as they are (no new action) except nothing is needed here.

- [ ] **Step 1: Failing tests** in `app.rs` tests using `testutil::env_with` building a project and a cache folder, `GoTo(Screen::Targets)`: `targets_screen_lists_projects_before_tools`; `h_folds_a_group_and_l_unfolds_it`; `enter_on_a_row_opens_it_in_the_explorer` (screen becomes Explorer, `selected_row().path` is the artifact); `sort_cycle_on_targets_follows_header_order`; `sidebar_has_one_caches_and_projects_entry`; `key_4_goes_to_targets` (input.rs test); `filter_narrows_target_rows_and_esc_clears_it`.
- [ ] **Step 2:** FAIL. **Step 3: Implement** navigation helpers mirroring the Explorer's (`step`, `window`). **Step 4:** PASS. **Step 5: Commit** `feat: Caches & Projects screen state and navigation`.

---

### Task 6: Shared selection and live joint Frees

**Files:** Modify `src/app.rs`, `src/targets.rs`; tests in `app.rs`.

**Interfaces:**
- State: `pub tool_marks: BTreeSet<String>` (command row ids), `pub set_space: SetSpace` where `pub struct SetSpace { pub generation: u64, pub value: Option<Space>, pub pending: bool }`; worker: on any change of `marks` or `tool_marks` call `fn marks_changed(&mut self)` which bumps `generation`, resolves marked paths to `Arc<Node>` via `scan::locate`, and spawns a thread running `scanner.view().space_of_set(&nodes)` (plus `file_space(..).frees_now` summed for marked files, added to `frees_now`/`footprint`) that sends `(generation, Space)` over an `mpsc` channel stored in `App`; `on_tick` drains it and keeps only the latest generation (stale results are dropped, `pending` stays true until the latest arrives).
- `fn mark_path(&mut self, path: PathBuf, usage: Usage, name: &str) -> bool` extracted from `toggle_mark` (same protected / covered / absorb rules and notices; the Explorer calls it). `ToggleMark` on Targets: folder row → `mark_path`; command row → toggle in `tool_marks`; group header → bulk-mark its **folder** rows (all marked → unmark all); section header → same for all folder rows in that section. Bulk marking skips rows already covered and never touches command rows.
- `pub fn row_state(&self, path: &Path) -> MarkState` with `enum MarkState { Unmarked, Marked, Covered(PathBuf) }`.
- `pub fn joint_frees(&self) -> Option<u64>` = `set_space.value.map(|s| s.frees_all())` (None while pending).
- `pub fn also_holds(&self, path: &Path) -> Vec<Holder>` where `pub struct Holder { pub path: PathBuf, pub usage: Usage, pub state: MarkState }`: other folders in the scan tree that share data with `path` (clone sets and hard-link groups), each reduced to the topmost scanned folder that is a registry/candidate row if one contains it, else the containing folder; built from the clone/link tables via a new `SpaceView::sharers(&self, node: &Arc<Node>) -> Vec<PathBuf>` (add it in `scan.rs` with its own test: `sharers_lists_other_folders_holding_the_same_clone`).
- `mark_all_related` (key `m` on Targets): marks every unmarked folder in `also_holds(selected)` that passes `Policy::can_mark`, then calls `marks_changed`.

- [ ] **Step 1: Failing tests:** `marks_made_in_the_explorer_show_as_marked_on_targets` and the reverse; `marking_a_parent_in_the_explorer_replaces_a_child_mark_made_on_targets` with notice text (Review Focus 3); `marking_inside_a_marked_folder_is_refused_on_targets`; `bulk_group_mark_marks_folder_rows_only` and `bulk_mark_skips_command_rows`; `command_row_marks_toggle_individually`; `joint_frees_rises_when_every_holder_is_marked` (store + two project clones via `clone_file`; mark one holder, then all) (Review Focus 4); `unmarking_one_holder_lowers_the_joint_figure`; `stale_worker_results_are_ignored` (two quick changes, only the latest generation lands); `also_holds_lists_unmarked_holders_with_state`; `m_marks_all_related_holders`; `deleting_clears_marks_that_no_longer_exist` (existing behavior retained for `tool_marks` after a command run is Task 7).
- [ ] **Step 2:** FAIL. **Step 3: Implement.** **Step 4:** PASS (`cargo test`). **Step 5: Commit** `feat: shared selection across screens and live joint frees`.

---

### Task 7: Review with commands, command job, finish

**Files:** Modify `src/app.rs`, `src/jobs.rs`, `src/input.rs`, `src/ui/dialogs.rs`.

**Interfaces:**
- `Review` gains `pub commands: Vec<ReviewCommand>`; `pub struct ReviewCommand { pub id: String, pub tool: String, pub line: String, pub spec: CommandSpec, pub up_to: u64 }`; `Review.space` is now the set-wise `Space` (use `set_space.value` if current, else compute with `scanner.space_of_set` as a fallback).
- `open_review` works from every screen that has marks (Explorer, Targets), lists folder items and commands, and refuses when both are empty.
- Key rule: a review with `commands` non-empty is confirmed only by `y` (`input::dialog_key` maps `y` → `Action::Confirm` for it; Enter is ignored; the dialog says so); `x` (move, then empty now) stays available and applies to folder items only.
- `jobs.rs`: `pub struct CommandJob` with `CommandJob::start(runner: Arc<dyn CommandRunner>, specs: Vec<CommandSpec>) -> CommandJob`, `poll() -> Vec<CommandEvent>`; `pub enum CommandEvent { Started { index: usize, total: usize, line: String }, Done { line: String, ok: bool, output_tail: String }, Finished }`. `Phase::Command` is added to `Phase`.
- Flow in `App::run_delete`: if `items` non-empty start `DeleteJob` first and remember the pending command specs; when it finishes (`poll_job`), start `CommandJob` (progress dialog shows phase Command with the current line). With no items, go straight to `CommandJob`. A failed command is recorded and the next still runs; moves are never undone. After all finish: remove the ran ids from `tool_marks`, refresh free space, and open `Dialog::Finish` whenever any command ran (also for soft-only moves), with `FinishInfo.commands: Vec<CommandOutcome { line: String, ok: bool, note: String }>`; free before/after is the volume's free space (existing fields).
- Accounting: command results count as freed only through `free_before`/`free_after`; no pending is recorded for commands.

- [ ] **Step 1: Failing tests** (fake runner injected via `App::with_runner`): `review_lists_folders_and_commands`; `review_with_commands_ignores_enter_and_confirms_on_y`; `x_moves_then_empties_folders_and_still_runs_commands`; `commands_run_after_the_moves`; `a_failing_command_does_not_stop_later_commands_or_undo_moves` (Review Focus 5); `finish_lists_each_command_outcome`; `command_marks_are_cleared_after_the_run`; `review_total_uses_joint_frees` (same number as `joint_frees()` for the same selection); `no_command_is_run_without_confirmation` (fake runner call count 0 after opening the review and cancelling); `command_job_runs_off_the_ui_thread` (poll returns without blocking while a slow fake runs).
- [ ] **Step 2:** FAIL. **Step 3: Implement** (dialog drawing for commands is in Task 8; this task adds the data and keys, and a minimal list so tests can render). **Step 4:** PASS. **Step 5: Commit** `feat: tool commands in the shared review, command job, finish outcomes`.

---

### Task 8: Drawing the screen

**Files:** Create `src/ui/targets.rs`; modify `src/ui/mod.rs` (match arm, footer hints, help line), `src/ui/explorer.rs` (extract `pub(super)` helpers: `Columns`/`columns(inner_width, g)`, `header_line(sort, &Columns, g)`, `number_spans(r: &Row, sort, queued, g)`, `frees_cell`, `value_style`), `src/ui/graph.rs` (take a `GraphData { rows: Vec<Row>, cursor: usize, total: u64, caption_scope: String, marks: &BTreeMap<..> }` built by the caller; Explorer builds it from `app.rows`; Targets builds it from the highlighted group's rows), `src/ui/dialogs.rs` (review shows commands in their own block with "irreversible" label and the line, and the key hint `y run and move` when commands exist).

**Layout:** left list pane (Explorer's panel and column layout, share bar when wide) and right column with the graph on top and the detail pane below (detail takes the whole column when the graph is hidden). Section lines are full-width bands (blue PROJECTS, purple TOOLS & CACHES; text `PROJECTS  <size>  frees <joint-or-standalone>`); group lines are bold lighter lines with fold marker; rows show mark glyph (`Covered` dimmed), safety dot, name, Size, On disk, Frees, share. Footer line inside the pane: `marked N · frees X now · +Y if <name> is marked · [d] review` (shows a spinner while `set_space.pending`, never a stale number as final; `+ up to Z by tools` when command rows are marked). Detail pane: path, safety word, regen text, the command line (if any), Space breakdown (frees now, kept alive by unmarked, held by snapshots, shared inside), and the `also_holds` list with mark state and `m` hint. Everything through `ui::clean`/`fit`; ASCII mode swaps the band glyphs.

- [ ] **Step 1: Failing render tests** in `tests/render.rs` style (TestBackend, throwaway sandbox): `targets_screen_shows_both_section_bands`; `targets_columns_match_the_explorer_header_order` (extract the header text of both and compare the column labels in order); `marked_row_shows_mark_and_footer_total`; `detail_pane_lists_unmarked_holders`; `ascii_mode_has_no_non_ascii` ; `minimum_size_does_not_panic` and `empty_scan_renders` (Review Focus 6); `review_dialog_shows_command_block`; `graph_pane_cycles_and_follows_group`.
- [ ] **Step 2:** FAIL. **Step 3: Implement.** **Step 4:** PASS, then run `cargo build && DUMP=1 python3 -I scripts/smoke_pty.py target/debug/maccleanup` to look at the screens (throwaway `HOME`). **Step 5: Commit** `feat: draw the Caches & Projects screen`.

---

### Task 9: Docs, smoke test, final checks

**Files:** Modify `README.md`, `CLAUDE.md`, `docs/superpowers/specs/2026-11-06-maccleanup-design.md` (status line), `scripts/smoke_pty.py`.

- [ ] Add to the smoke script: open the screen with `4`, press `space` on a row, `d`, assert the review text, cancel, quit. It must set a throwaway `HOME` with a fake project (`package.json`, `node_modules`).
- [ ] `README.md`: key table gains `4`, `m`, fold keys, and the screen description; `CLAUDE.md` code map gains `catalog/`, `targets.rs`, `ui/targets.rs`, set-wise space, command job; keep the two standing rules already added.
- [ ] Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup`. All must pass; report the actual output.
- [ ] Commit `docs: caches and projects screen, keys, smoke test`.

---

## Self-review notes

- Spec coverage: G1 sidebar/screen (T5), G2 sections (T4, T8), G3 joint Frees (T1, T6), G4 detail pane (T6, T8), G5 consistency (T5, T8 tests compare headers), G6 review with commands (T7), G7 accounting via free before/after (T7), G8 shared selection (T6). Spec sections 6 (data source banner, dedupe) are in T4 and T8; the scan-root banner is drawn in T8 when `stack[0]` is not the home folder.
- Known limitation recorded: marked **files** (from the Explorer) join the joint figure by their own `file_space`, not through clone sets with marked folders.
