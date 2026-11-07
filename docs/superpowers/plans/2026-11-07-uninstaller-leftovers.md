# Uninstaller and Leftovers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two new pages (Uninstall, Leftovers) that list apps with their related folders, mark and remove them through the shared review, with a minimal elevated batch for root-owned items.

**Architecture:** Both pages are the existing Caches & Projects list (`Targets`, `Group`, `TRow`) with two new `SectionKind`s and `Page`s. A new `catalog/apps.rs` builds an installed-app index; `catalog/appfiles.rs` finds each app's related folders; `catalog/leftovers.rs` applies the evidence rules. Outside-home paths found by these rules are unlocked in `Policy`. A root-owned item that fails a normal move goes into one batch run through an `Elevator` (osascript password prompt) after the holding manifest entry is written.

**Tech Stack:** Rust 2024, ratatui 0.30, `plist` (add with `cargo add`; check usage with context7 first), existing `CommandRunner`/`FakeRunner`, `bg::Latest`, `bg::walk_pool`.

**Spec:** `docs/superpowers/specs/2026-11-07-uninstaller-leftovers-design.md` (read its "Calls made after the builder check" too).

## Global Constraints

- Rust code formatted with `cargo fmt`; `cargo clippy --all-targets -- -D warnings` clean; add dependencies only with `cargo add`.
- Never delete without the review; soft delete default; `y` only when the review holds commands or items that may ask for the password; Enter never confirms an irreversible action.
- The app never asks for, reads or stores a password. Elevation is only `osascript ... with administrator privileges`; no `sudo`.
- Nothing slow on the UI thread; list builds via `bg::Latest`; sizes via `bg::walk_pool()`.
- Real commands (`osascript`, `brew`, `port`, `launchctl`, `mdls`) are never run in tests: use `FakeRunner` / a `FakeElevator`. Tests that run the binary set a throwaway `HOME`.
- Never list a leftover on a guess; unsure means not listed.
- Menu digits come from `SIDEBAR` positions (Leftovers = 3, Uninstall = 4 stay as they are).
- Sort cycle follows header order: Name, Size, On disk, Last used (Uninstall) / Modified (Leftovers).
- All names shown go through `ui::clean/fit`.

## Review Focus

- An app bundle whose `Info.plist` is missing, binary or malformed: skipped, never a panic.
- A name with spaces, quotes, `$`, backticks or a newline reaching the elevated batch: passed as separate `argv` items, never spliced.
- A symlink in `~/Applications` (Homebrew Caskroom) pointing outside the scan roots: counted once, never deleted through the link target.
- The user cancels the macOS password prompt: nothing is half-moved, the manifest holds no stale entry, the copy-command dialog is offered.
- Spotlight disabled or `mdls` failing: Leftovers shows nothing and says why; Uninstall still lists apps with "never".
- An app that is running (including the terminal MacCleanup runs in): whole group not markable, rechecked when the review opens.

---

### Task 1: Branch and app-location policy

**Files:**
- Modify: `src/safety.rs` (the outside-home branch of `Policy::classify`)
- Create: `src/catalog/apppolicy.rs` (declared in `src/catalog/mod.rs`)
- Test: unit tests in `apppolicy.rs`

**Interfaces:**
- Produces: `pub fn app_location(path: &Path) -> Option<Safety>` in `apppolicy.rs`: `Some(Caution)` for a path that this feature may offer (a direct child of `/Applications` or `/Applications/<Subfolder>/` that ends in `.app`, a direct child of `/Library/{Application Support,LaunchAgents,LaunchDaemons,PrivilegedHelperTools,Caches,Preferences,Logs}`), `Some(Protected)` for anything Apple/system, `None` for everything else (stay protected by the old rules). `pub fn is_apple_id(id: &str) -> bool` (`com.apple.` prefix, case-insensitive). `pub fn is_apple_path(path: &Path) -> bool`.
- Consumes: `Safety` from `safety.rs`.

- [ ] **Step 1:** `git checkout -b uninstaller-leftovers`.
- [ ] **Step 2: Write failing tests** in `apppolicy.rs`: `app_in_applications_is_caution` (`/Applications/Foo.app` → `Some(Caution)`), `apple_app_in_applications_is_protected` (`/Applications/Safari.app`, `/Applications/Xcode.app`, `/Applications/GarageBand.app` → `Some(Protected)`), `system_paths_are_protected` (`/System/Applications/Mail.app`, `/Library/Apple/x`, `/usr/bin/ls` → `Some(Protected)` or `None` but never `Caution`), `library_child_is_caution` (`/Library/LaunchDaemons/com.docker.socket.plist`), `library_root_is_none` (`/Library`, `/Library/LaunchDaemons` themselves → `None`), `is_apple_id_cases` (`com.apple.Safari` true, `COM.APPLE.x` true, `com.applesauce.x` false). Apple detection by path alone uses a hard-coded name list for Apple apps (`Safari`, `Xcode`, `GarageBand`, `Final Cut Pro`, `Logic Pro`, `Keynote`, `Numbers`, `Pages`, `iMovie`, `TextEdit`, `Mail`, `Music`, `TV`, `Photos`, `Preview`, `Notes`, `Terminal`, `Utilities`); id-based Apple detection happens in Task 2.
- [ ] **Step 3:** Run `cargo test apppolicy` → FAIL (module missing).
- [ ] **Step 4: Implement** the three functions; use a `const APPLE_APP_NAMES: &[&str]`.
- [ ] **Step 5:** In `Policy::classify`, where `strip_prefix(&self.home)` fails, return `app_location(path).unwrap_or(Safety::Protected)` (keep the holding-folder check first and the `has_parent_dir` check). Add a safety.rs test `policy_allows_app_locations_only` (`/Applications/Foo.app` → Caution, `/etc/hosts` → Protected, `/Applications/Safari.app` → Protected, `/Applications/Foo.app/../Safari.app` → Protected).
- [ ] **Step 6:** `cargo test` → PASS. Commit `feat: policy for app locations outside home`.

### Task 2: Installed app index

**Files:**
- Create: `src/catalog/apps.rs`; Modify: `src/catalog/mod.rs`; `Cargo.toml` via `cargo add plist`
- Test: unit tests in `apps.rs` with `.app` fixtures under `tempfile`

**Interfaces:**
- Produces:
  - `pub struct AppBundle { pub path: PathBuf, pub id: String, pub name: String, pub version: String, pub executable: PathBuf, pub owner_uid: u32, pub last_used: i64 /* unix secs, 0 = never */, pub manager: Option<Manager> }` where `Manager` is the existing `catalog::resolve::Manager` or a new `AppManager { Brew(String), Port(String) }` defined here.
  - `pub struct InstalledIndex { pub apps: Vec<AppBundle>, pub nested_ids: BTreeSet<String>, pub spotlight_ok: bool }` with `pub fn has_id(&self, id: &str) -> bool` (exact id, nested id, or a longer id in the same two-segment vendor sharing the prefix).
  - `pub fn read_bundle(path: &Path) -> Option<AppBundle>` (reads `Contents/Info.plist`: `CFBundleIdentifier`, `CFBundleName`/`CFBundleDisplayName`, `CFBundleShortVersionString`, `CFBundleExecutable`; `None` on any parse problem).
  - `pub fn build_index(home: &Path, roots: &[PathBuf], runner: &dyn CommandRunner) -> InstalledIndex` (roots: `/Applications`, `~/Applications`, `/System/Applications`, descending into non-`.app` subfolders; nested `*.app`, `*.xpc`, `*.appex` under `Contents` contribute ids to `nested_ids`; last used and the Spotlight completeness query go through `runner`).
  - `pub fn running_bundles(runner: &dyn CommandRunner) -> Vec<PathBuf>` (executable paths from `ps -axo comm=`; used to lock groups).
- Consumes: `CommandRunner::query`, `CommandSpec::new`.

- [ ] **Step 1:** Look up the `plist` crate with context7 (`resolve-library-id` then `query-docs`: reading a plist dictionary from a file, XML and binary); `cargo add plist`.
- [ ] **Step 2: Write failing tests:** `read_bundle_reads_id_name_version` (fixture `Foo.app/Contents/Info.plist` XML), `read_bundle_none_for_missing_or_garbage_plist`, `read_bundle_binary_plist`, `index_skips_symlink_loops_and_counts_a_symlinked_app_once`, `nested_helper_ids_are_collected` (`Foo.app/Contents/Library/LoginItems/Helper.app` with `com.foo.helper`), `has_id_vendor_prefix` (installed `com.autodesk.dls.streamer.scriptapp.Autodesk-Fusion` makes `com.autodesk.dls.streamer` true and `com.google.x` false), `last_used_never_is_zero` (FakeRunner answers `mdls` with `(null)`), `spotlight_ok_false_when_query_fails`.
- [ ] **Step 3:** Run tests → FAIL. **Step 4:** Implement. Last used: one `mdls -name kMDItemLastUsedDate -raw <paths...>` per 50 bundles, outputs NUL-separated; verify the exact output on this Mac first with the real binary (read-only, outside tests). Spotlight completeness: `mdfind kMDItemContentType == 'com.apple.application-bundle'` returns paths; ok when it returns at least one path under `/Applications`; apps it lists that are outside the scan roots are added to the index.
- [ ] **Step 5:** `cargo test apps` → PASS. Commit `feat: installed app index`.

### Task 3: Related folders of an app

**Files:**
- Create: `src/catalog/appfiles.rs`
- Test: unit tests with a `tempfile` HOME

**Interfaces:**
- Produces: `pub struct AppPart { pub path: PathBuf, pub label: String, pub shared_vendor: bool, pub needs_admin_hint: bool }`; `pub fn parts_of(app: &AppBundle, siblings: &[AppBundle], home: &Path, lib: &Path /* /Library root, injectable for tests */) -> Vec<AppPart>`; `pub fn strip_team_id(name: &str) -> &str` (drops a leading `^[A-Z0-9]{10}\.`); `pub fn launcher_target(app: &Path) -> Option<PathBuf>` (reads the executable as text, finds `destfolder="..."`, returns it only if inside `~/Library`).
- Consumes: `AppBundle`.

- [ ] **Step 1: Failing tests:** `finds_standard_library_folders` (creates `Caches/<id>`, `Containers/<id>`, `Preferences/<id>.plist`, `Saved Application State/<id>.savedState`, `HTTPStorages/<id>`, `Application Support/<name>`; all returned), `group_container_team_id_stripped` (`UBF8T346G9.com.microsoft.teams` matches id `com.microsoft.teams`; `strip_team_id` table), `launcher_script_destfolder_joins_group` (fixture script from the spec with `destfolder="<home>/Library/Application Support/Autodesk/webdeploy/production/b43a…"`; a destfolder outside `~/Library` is ignored), `vendor_folder_needs_two_segments_and_name_match` (id `com.autodesk.x`, folder `Application Support/Autodesk` → included, `shared_vendor` true when a second installed app of that vendor exists), `big_vendors_never_attach_vendor_folder` (ids under `com.google`, `com.microsoft`, `com.adobe`, `org.mozilla`, `com.jetbrains` do not attach `Application Support/Google` etc.), `wrapper_bundles_in_same_dir_join` (sibling `Autodesk Fusion Service Utility.app` sharing a vendor prefix and name stem), `never_follows_symlinks`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement per the spec section "Related locations". **Step 4:** PASS. Commit `feat: related folders of an app`.

### Task 4: Leftover detection

**Files:**
- Create: `src/catalog/leftovers.rs`
- Test: unit tests

**Interfaces:**
- Produces: `pub struct LeftoverGroup { pub id: String, pub parts: Vec<AppPart>, pub reason: String }`; `pub enum LeftoversState { Ready(Vec<LeftoverGroup>), Unavailable(String) }`; `pub fn find_leftovers(index: &InstalledIndex, home: &Path, lib: &Path, running_ids: &BTreeSet<String>) -> LeftoversState`.
- Consumes: `InstalledIndex::has_id`, `strip_team_id`, `apppolicy::is_apple_id`.

- [ ] **Step 1: Failing tests:** `unavailable_when_spotlight_not_ok` (returns `Unavailable` with a reason mentioning Spotlight), `lists_folder_named_by_uninstalled_id` (`Caches/com.gone.app` listed, grouped with `Preferences/com.gone.app.plist` and `Saved Application State/com.gone.app.savedState`), `ignores_ids_of_installed_or_nested_helpers`, `name_only_folders_are_never_leftovers` (`Application Support/Some App` absent), `apple_ids_are_never_leftovers`, `vendor_folders_never_form_a_group`, `launch_daemon_listed_only_when_program_missing` (plist in a fake `lib/LaunchDaemons` whose `ProgramArguments[0]` does not exist; a plist whose program exists is not listed), `reason_text_names_the_id`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement per "Leftover evidence" (one group per bundle id; id extracted from file names by stripping `.plist`, `.savedState`, `.binarycookies`, team prefix). **Step 4:** PASS. Commit `feat: leftover detection`.

### Task 5: Pages in the list (Uninstall and Leftovers)

**Files:**
- Modify: `src/targets.rs` (`SectionKind::{Apps, Leftovers}`, `Page::{Uninstall, Leftovers}`, `ALL`, `page()`, `BuildSource.apps: Option<AppsInput>`, `build`), `src/app.rs` (`Screen::{Uninstall, Leftovers}`, `SIDEBAR` screens, `goto`, `on_targets`, `page`, discovery thread wiring next to `discover_slow`), `src/input.rs`, `src/ui/mod.rs`, `src/ui/targets.rs` (titles, Last used header)
- Test: `tests/render.rs` (TestBackend), unit tests in `targets.rs`

**Interfaces:**
- Produces: `pub struct AppsInput { pub index: Arc<InstalledIndex>, pub leftovers: LeftoversState, pub running: Vec<PathBuf> }` gathered on the discovery thread (`discover_apps(home, runner) -> AppsInput`); rows for outside-home folders are built with `Row { node: None, is_dir: true, size/disk from a walk on bg::walk_pool(), mtime }`; for Uninstall rows `Row.mtime` is the app's `last_used` (0 = never) and the group row's mtime is the newest of its bundles; `TRow.note` carries the "why listed" text; `Group.title` is the app name (Uninstall) or bundle id (Leftovers).
- Consumes: Tasks 2-4, `Policy::classify` (rows with `Protected` are shown dimmed, not dropped, on these pages only).

- [ ] **Step 1: Failing tests:** `uninstall_page_lists_app_group_with_parts` (sandbox HOME + fake `/Applications`; group title = app name, bundle row plus Caches row), `leftovers_page_lists_groups_by_id`, `apple_app_row_is_protected_and_not_dropped`, `page_for_screen` (`Screen::Uninstall` → `Page::Uninstall`), render test `uninstall_header_shows_last_used_and_never` (a never-opened app shows `never`), `sorting_by_modified_puts_never_first_ascending` (App `SortBy::Modified` reversed order is oldest-first only via the existing direction toggle; assert `never` rows sort as oldest).
- [ ] **Step 2:** FAIL. **Step 3:** Implement: sidebar `Leftovers`/`Uninstall` get screens; `Targets::lines(filter, page)` already selects by `SectionKind::page()`; header label "Last used" on `Page::Uninstall`, "Modified" otherwise; cell text `never` when mtime is 0 on `Page::Uninstall`. The build keeps `Protected` rows only for these two sections. Loading state shows a `bg::Work` marquee.
- [ ] **Step 4:** `cargo test` → PASS. Commit `feat: Uninstall and Leftovers pages list apps and leftovers`.

### Task 6: Marking rules

**Files:**
- Modify: `src/app.rs` (`toggle_target_mark`, `mark_bulk_commands` sibling `mark_bulk_apps`, `open_review`), `src/targets.rs` (`TRow` fields `locked: Option<String>`, `shared: bool`)
- Test: unit tests in `app.rs` using `testutil.rs` sandbox

**Interfaces:**
- Produces: `TRow.locked` (reason: running, Apple/system, protected) makes the row non-markable with a notice; header marks mark the bundle (or its manager command, id `app:<manager>:<name>`) and in-home parts, skipping outside-home non-bundle parts, `shared` vendor rows (markable only when every installed app of that vendor is marked), and locked rows; a running app locks its whole group; `open_review` rechecks running state.
- Consumes: Task 5 rows, `apps::running_bundles`.

- [ ] **Step 1: Failing tests:** `running_app_group_cannot_be_marked`, `header_mark_skips_outside_home_parts_and_shared_vendor_rows`, `shared_vendor_folder_markable_when_all_vendor_apps_marked`, `apple_row_refuses_with_notice`, `review_rechecks_running` (an app that starts running after marking is dropped from the review with a problem line), `terminal_chain_is_locked` (FakeRunner `ps` shows the parent chain).
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS. Commit `feat: marking rules for app groups`.

### Task 7: Package-manager apps

**Files:**
- Modify: `src/catalog/apps.rs` (manager detection), `src/targets.rs` (command row for the bundle), `src/catalog/resolve.rs` use
- Test: unit tests with `FakeRunner`

**Interfaces:**
- Produces: for an app whose bundle is owned by `macports` or listed by `brew list --cask` (queried once on the discovery thread), the bundle row is `RowKind::Command { id: "app:brew:<token>" | "app:port:<name>", spec }` with spec `<abs brew> uninstall --cask <token>` / `<abs port> uninstall <name>` (token/name only after `safe_name`), `cwd` = home; the remaining parts stay `RowKind::Folder`. Missing manager = not a manager app.
- Consumes: `resolve::resolve`, `catalog::runtimes::safe_name`, `CommandSpec`.

- [ ] **Step 1: Failing tests:** `brew_cask_app_becomes_command_row` (FakeRunner answers `brew list --cask`), `port_owned_app_becomes_command_row`, `unsafe_token_is_refused` (`foo; rm`), `missing_brew_means_plain_folder_row`, `command_row_not_marked_by_other_pages_bulk`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS. Commit `feat: package-manager apps use the manager's uninstall`.

### Task 8: Elevated batch

**Files:**
- Modify: `src/softdelete.rs` (split `move_in` into `reserve` + rename; add `unreserve`), `src/jobs.rs` (`DeleteJob::start_with(.., elevator)`, `run_move` collects `PermissionDenied`)
- Create: `src/elevate.rs` (`Elevator` trait, `OsascriptElevator`, test-only `FakeElevator`)
- Test: unit tests in each file; integration in `tests/` only with `FakeElevator`

**Interfaces:**
- Produces:
  - `Holding::reserve(&self, policy: &Policy, path: &Path, usage: Usage) -> Result<Reserved, HoldError>` with `pub struct Reserved { pub item: HeldItem, pub payload: PathBuf }` (all checks and the manifest entry of `move_in`, no rename); `move_in` becomes `reserve` + `rename` with identical behaviour and tests; `Holding::unreserve(&self, r: &Reserved)` removes the manifest entry and item dir.
  - `pub struct ElevatedMove { pub from: PathBuf, pub to: PathBuf }`; `pub trait Elevator: Send + Sync { fn move_all(&self, moves: &[ElevatedMove], owner: &str) -> Result<(), ElevateError>; }` with `ElevateError { Cancelled, Failed(String) }`.
  - `OsascriptElevator::move_all` runs `osascript` with a constant script and the paths as separate `argv` items; the script builds `mv -- <from> <to> && chown -R <owner> <to>` per move using `quoted form of`, joins them with `;`, and runs one `do shell script ... with administrator privileges`. User-cancel (AppleScript error -128) maps to `Cancelled`. Every `from` and `to` is revalidated (absolute, no `..`, `Policy::classify_real != Protected`, `to` inside the holding payload dir) before the call.
  - `DeleteJob::start_with(holding, policy, items, mode, elevator: Arc<dyn Elevator>)`; `start` keeps its signature and passes a `NoElevator` that returns `Failed("needs administrator rights")`.
- Consumes: `HoldError::Io` with `ErrorKind::PermissionDenied`.

- [ ] **Step 1: Failing tests:** softdelete: `reserve_writes_manifest_before_any_move_and_unreserve_removes_it`; `move_in_still_behaves_as_before` (existing tests stay). elevate: `script_receives_paths_as_argv` (spy on the command line the elevator would build: paths with spaces, quotes, `$(x)`, newlines appear only as separate args), `revalidation_refuses_protected_or_relative_paths`, `cancel_maps_to_cancelled`. jobs: `permission_denied_items_go_to_one_elevated_batch` (a read-only parent dir in a tempdir; `FakeElevator` records one call with all failed items and performs the rename), `cancelled_elevation_leaves_no_manifest_entry_and_reports_failures`, `normal_items_do_not_reach_the_elevator`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS. Commit `feat: one elevated batch for root-owned items`.

### Task 9: Review wording, `y`, fallback dialog, launch items

**Files:**
- Modify: `src/app.rs` (`open_review`, `ReviewItem.needs_admin: bool`), `src/ui/dialogs.rs` (review label, fallback dialog reuse), `src/jobs.rs` (launchctl step), `src/catalog/appfiles.rs` (launch-item parts carry domain)
- Test: unit tests in `app.rs`, render tests in `tests/render.rs`

**Interfaces:**
- Produces: `ReviewItem.needs_admin` (set by `access(W_OK)` on item and parent for outside-home paths); the review shows "may ask for your password" on those rows and requires `y`; on `ElevateError::Cancelled/Failed` the existing "show the exact command + copy + Cancel + I ran the command" dialog is opened with `mv`/`chown` lines; launch items get a `launchctl bootout system|gui/<uid> <plist>` command run first through `CommandRunner` (on failure the move proceeds only if the target process is not running).
- Consumes: Task 8 `Elevator`, `ReviewCommand`.

- [ ] **Step 1: Failing tests:** `review_marks_admin_rows_and_needs_y`, `enter_does_not_confirm_a_review_with_admin_rows`, `cancelled_prompt_opens_copy_command_dialog_with_exact_commands`, `daemon_part_runs_bootout_before_move` (FakeRunner order), `bootout_failure_with_running_process_skips_the_move`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement; render test snapshots the review text. **Step 4:** PASS. Commit `feat: review shows admin rows, copy-command fallback, launch items`.

### Task 10: Docs and end-to-end check

**Files:**
- Modify: `CLAUDE.md` (project line, code map, rules: rewrite "Never escalate privileges" as the new rule; Uninstall/Leftovers section), `README.md` (key table, pages, commands), spec status line; `scripts/smoke_pty.py` if the smoke script lists pages
- Test: `cargo test`, `cargo clippy --all-targets -- -D warnings`, smoke script

- [ ] **Step 1:** Update the docs listed above (new rule text: the app still never asks for or reads a password; the system prompt is used only for one batch after the review).
- [ ] **Step 2:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` all clean.
- [ ] **Step 3:** `cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup` with a throwaway `HOME`; confirm both pages open (keys `3`, `4`) without panics.
- [ ] **Step 4:** Read-only check on this Mac in the real TUI (do not mark anything): the Uninstall page shows Autodesk Fusion as one group including its `webdeploy` payload; Apple apps are dimmed; the Leftovers page lists only id-based groups.
- [ ] **Step 5:** Commit `docs: Uninstaller and Leftovers pages`.

---

## Self-review

- **Spec coverage:** goals/pages (5), protection (1, 6), related locations and Fusion indirection (3), leftover evidence and Spotlight gate (2, 4), deletion flow, elevation, fallback (8, 9), package managers (7), look/columns/never (5), docs (10). Left out on purpose, as in the spec: receipts/login items beyond the details text, other volumes.
- **Types:** `AppBundle`, `InstalledIndex`, `AppPart`, `LeftoverGroup`, `Elevator`, `Reserved` are used with the same names in every task.
- **Proportion:** ten tasks for a ten-section spec; code appears only as signatures and tests.
