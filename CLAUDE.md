# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MacCleanup is a Rust TUI (colorful, animated, mouse-supported) that finds and reports disk usage on macOS and cleans up extra files safely. Milestone 1 (scanner, explorer, safety layer, soft delete, graphs, accounting, finish screen), milestone 2 (the Caches & Projects screen) milestone 3 (the Docker screen) and milestone 4 (the Time Machine screen) are implemented. Leftovers, uninstaller, packages and runtimes and the other candidates are planned (design doc) and appear dimmed in the sidebar.

## Commands

```bash
cargo run --release                         # run (add -- --ascii, --no-animation, --holding-dir DIR; a PATH argument is the root, e.g. `-- .`)
cargo fmt                                   # always format Rust code
cargo clippy --all-targets -- -D warnings   # must be clean
cargo test                                  # unit + integration tests (use `cargo test <name>` for one)
cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup   # end-to-end run in a pty (DUMP=1 prints screens)
```

Add dependencies only with `cargo add`. Rust 2024 edition; ratatui 0.30 (use its re-exported `ratatui::crossterm`, not a separate crossterm dependency).

## Code map (`src/`)

- `safety.rs` hard-coded protected-path policy (`Policy`, `Safety`); `softdelete.rs` the ToBeDeleted folder (`Holding`); `jobs.rs` background delete (`DeleteJob`, `DeleteMode`).
- `scan.rs` multithreaded scanner (`Scanner`, `Node`, `Row`); `volume.rs` free space; `perms.rs` Full Disk Access check; `accounting.rs` pending vs freed; `treemap.rs` squarified layout.
- `app.rs` all UI state and `App::update(Action)`; `input.rs` key and mouse mapping (key bindings live here); `anim.rs` animation helpers.
- `catalog/` what can be cleaned: `artifacts.rs` (project build output found in the scan tree, only with its marker file beside it), `tools.rs` (`discover_tools`: tool caches, slow because it asks the installed tools, so it runs on a thread), `split.rs` (build folders split into intermediate folders and kept products), `resolve.rs` (every install of a program: `PATH`, fnm, nvm, volta), `command.rs` (`CommandSpec`, the `CommandRunner` trait, `SystemRunner`, test-only `FakeRunner`). `targets.rs` builds the Caches & Projects list (sections, groups, `TRow` = `scan::Row` plus meta); `bg.rs` is `Latest<T>`, background work where only the newest result counts.
- `docker/` the Docker page: `model.rs` (plain data, `ItemId` = `image:`/`container:`/`volume:`/`cache:`/`vm:trim`), `api.rs` (parses Docker Engine API JSON; exact bytes), `cli.rs` (`DockerCli::connect/load/cli/api_get`: finds `docker`, pins the `desktop-linux` context, reads the API over its unix socket with `curl` through the `CommandRunner`; `vm_disk_allocated`), `analysis.rs` (the tree, shared/unique/frees from layer ids, `frees_of_set`, `removal_order`, `blockers`), `view.rs` (`DockerView::lines`: sections, folds, sort, filter), `remove.rs` (the removal command per item), `vmtools.rs` (our `maccleanup/vm-tools` helper image: Dockerfile on stdin, trim, `LIST_SCRIPT`, `browse_spec`), `browse.rs` (read-only file listing), `state.rs` (`DockerState`). The App side is `app/dockerpage.rs` (a child module of `app`, to reach private fields); drawing is `ui/docker.rs` and `ui/docker_files.rs`.
- `snapshots.rs` Time Machine snapshots (`load`, `delete_spec`, `estimate_held`, `measure`, `Settle`); `app/snappage.rs` its App side (child module like `dockerpage`); drawing is `ui/snapshots.rs`.
- `filter.rs` fzf-style `Query` for the explorer's filter box (terms ANDed; fuzzy / `'exact` / `^prefix` / `suffix$` / `!exclude`; smart case; returns highlight positions).
- `ui/` drawing only (`mod.rs` helpers, `sidebar`, `home`, `explorer` (also the shared column helpers `cols`/`header_line`/`number_spans`), `graph` (takes a `GraphData`), `targets`, `holding`, `dialogs`); `glyphs.rs` Nerd/ASCII symbols; `fmt.rs` sizes and bars; `cli.rs` flags; `main.rs` terminal setup and event loop.
- Tests: unit tests sit beside the code; `tests/render.rs` (TestBackend rendering), `tests/cli_smoke.rs` (binary flags/exit codes); `testutil.rs` builds a sandbox `App` for unit tests.

## Rules and gotchas

### Safety and deletion

- Deletion is a final step: list every file with its size on the review screen, confirm, then delete in the background with a progress bar. Never delete without that review.
- Soft delete is the default everywhere; permanent delete only through "move, then empty now" (`x` in the review dialog) or the ToBeDeleted screen, both behind an explicit `y` (Enter never confirms an irreversible action).
- Never escalate privileges or ask for a password. For actions needing admin rights, show the exact command with a copy-to-clipboard button plus "Cancel" and "I ran the command" buttons; after the user confirms, rescan and continue. (Not built yet.)
- One shared selection: `App::marks` is the only delete list; every screen reads and writes it, and `d` on any screen reviews everything marked. A screen never keeps its own delete list. New kinds of deletable things (tool cleanup commands, Docker items, ...) become new entry kinds in the same selection and the same review. Design: `docs/superpowers/specs/2026-11-06-caches-projects-design.md` section 4a.
- Anything outside the home folder is protected in this release. A folder containing a protected item is refused as a whole (`Policy::protected_descendant`, checked in `Holding::move_in` on the delete thread; `Safe` folders are not walked).
- `Holding::move_in` writes the manifest entry BEFORE the rename (an interrupted move leaves a deletable stale entry, never an untracked payload); `App` defers quitting until a running delete job finishes; `App::too_small` (set by `ui::draw`) limits input to quit.
- Every quit goes through `App::request_quit`: deferred while a delete job runs, a warning dialog while ToBeDeleted holds items (a second quit/Ctrl-C confirms), immediate otherwise.

### Tool commands (`catalog/`)

- Caches for docker and pnpm should be cleaned through their own commands where possible, not by removing files directly. Multiple npm/pnpm/node installs may exist (fnm and others); don't assume one.
- Tool cleanup commands (`catalog/tools.rs`): a command is pinned to one exact install (absolute program), runs with `cwd` = the home folder, `COREPACK_ENABLE_NETWORK=0`, `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`, the matching Node first on `PATH`, no stdin and a timeout. pnpm keeps one store per major (`store/vN`), so each store folder is its own row, pruned by the pnpm that reports it with `pnpm store path`; its note estimates the unused part (`tools::unused_in_store`: files with one link and no clone). pnpm's own cache folder is split into `pnpm-dlx` (`pnpm dlx`) and one metadata row per `vN`, folder moves because pnpm has no command for them; npm's cache folder comes from `npm config get cache`. Discovery and commands never run on the UI thread, and a failing query only turns that row into a plain folder move. Real commands are never run in tests: use `FakeRunner` (unit tests) or a no-op `CommandRunner` (integration tests).
- A review that contains tool commands is confirmed only by `y`; folder moves run first, then the commands in the listed order; a failing command neither stops the rest nor undoes the moves. Marking a group or section never marks a command row.
- Running commands: `CommandRunner::run` takes a `RunControl` (live last output line, elapsed time, a cancel switch). Every command runs in its own process group so a cancel (`killpg`) reaches what it started; never run one without a way to stop it, and never add `--force`-style flags on the user's behalf: a forced variant is `CommandSpec::with_force_args`, offered by the user with `f` in the review (or on retry after a lock wait, `is_lock_wait`). uv waits `UV_LOCK_TIMEOUT` (300 s) on its cache lock, held by any running `uv run`.
- One command, one item: a tool that needs several commands to clean (npm: `_cacache` and `_npx`; pnpm: one store per version; Go: build and module caches) lists one `ToolEntry` per command, so each is counted and selected on its own. After a cleanup `App::process_refresh` reflects it in the scan (`Scanner::forget_dir` for what is gone, `rescan_dir` for what changed, one at a time); a list build only counts as current if `scan_epoch` did not change since it started.

### Space and accounting

- `Node::mtime` / `Row.mtime` is the newest modification time inside a folder (an additive max kept by the scan; it only grows, so a rescan after deletions can leave it slightly high). The Modified column and `SortBy::Modified` use it on every Explorer-like screen.
- Show every size as both logical size and on-disk size (`fmt::both`, `usage::Usage { size, disk }`); sorting, graphs and share bars default to on-disk size. Free space is the volume's free space before vs after; soft-deleted data is "pending", never "freed".
- Space is clone- and snapshot-aware (`scan.rs`). Per file the scan calls `clone_facts` (getattrlist: private size, clone id, clone count; see memory `apfs-clone-detection`). Additive per-folder counters (`Node::private`, `Node::snapshot_held`) hold the plain files; clone sets (`CloneTable`, members = containing folders) and hard-link groups (`LinkGroup`) hold the shared ones. `Scanner::space(node)` / `Scanner::list_children` (all folder rows in one pass, only when the scan is idle) turn that into `Space { footprint, frees_now, snapshots, shared_outside, shared_inside, saved }` with the identity `footprint == frees_now + snapshots + shared_inside + shared_outside` (test it). `Row.disk` of a folder is its footprint once `Row.space` is set; `Row::live` must keep it. Hard-linked data is counted once in `disk` (credited to the first folder scanned, a thread race), so `Space` adds it back for other folders; never show the credited number as what a delete frees. Tests that need clones use `cp -c` (APFS only).
- Snapshot attribution is exact, never "any gap is a snapshot" (`scan.rs`: `snapshot_may_hold`, `held_bytes`, `refresh_local_snapshots`): blame only if a local snapshot exists (`tmutil listlocalsnapshots /`, refreshed at each scan start), the file was born before the newest one, `refcnt != 0` (macOS-compressed files read private 0, refcnt 0) and only for the shortfall inside the logical size (blocks preallocated past EOF are not snapshot data). Open or just-written files also read private 0 for a moment, hence the birth-time rule.
- Set-wise space: what a whole selection frees is `Scanner::space_of_set(..).frees_all()` (`frees_now + shared_inside`: a clone set frees only when every holder is inside the set), computed on a thread (`App::set_space`) and shown as `App::joint_frees()`. Row Frees columns stay per-row (what deleting that row alone frees), like the Explorer.
- `Row.frees` (`Option<u64>`, None while scanning) is the Frees column and a sort key (`SortBy::Frees`); `Row::queued()` marks folders the scan has not started (`Node::is_started`). The treemap is the default graph. The explorer shows a folder-totals line above the scan totals and a scrollbar plus `a/b` position.
- Time Machine snapshots: the scan reports what they hold (`snapshots`); the Time Machine page (`snapshots.rs`, `app/snappage.rs`, `ui/snapshots.rs`) lists them (`tmutil listlocalsnapshotdates /`) and deletes marked ones with `tmutil deletelocalsnapshots <date>` (no root; a date reaches a command only after `snapshots::valid_date`). Only `com.apple.TimeMachine.*` snapshots are markable (`tool_marks` id `snapshot:<date>`); the system's are listed dimmed. Per-snapshot size is `snapshots::estimate_held` (mounted snapshot vs live files, see memory `time-machine-snapshot-size`), measured on demand with `m`/`M` through `bg::Latest`; never use the private size of a file inside a snapshot (always 0). After a delete `Settle` makes `App::poll_snapshots` read free space every 5 ticks until it is stable for 30 s (max 5 min); the footer and finish screen follow `free_now`.
- On exit `main.rs` prints `Accounting::summary` (session totals, tool-command estimate via `record_commands`, free space at start/now, and the change). The footer (`ui::render_footer`) shows free now / at start on every page. `Accounting::set_pending` seeds pending from what earlier sessions left in ToBeDeleted.

### Scanning

- Rescans (`r` folder, `R` all) swap a fresh subtree into the tree (`Scanner::rescan_dir`/`rescan_all`) and are refused while any scan runs; hard-link bookkeeping remembers which folder counted each inode so a subtree rescan can forget exactly its own entries.
- Every scan is multithreaded for maximum speed: pool of `Scanner::default_threads()` (2x cores), a job per directory, no lock on the per-file hot path.

### Docker

- Docker is read from the Engine API over the Docker Desktop socket (`curl --unix-socket`, through `CommandRunner`, so tests use `FakeRunner` and fixtures in `tests/fixtures/docker/`); the CLI only prints rounded sizes. Every command pins `--context=desktop-linux`; nothing ever runs on another context. Docker's `UniqueSize` is not trusted (it can be negative): shared, unique and frees come from layer ids (`docker/analysis.rs`).
- Docker items are `ReviewCommand`s in the shared selection (`App::tool_marks`, ids from `docker::model::ItemId`), reviewed and run by the existing review and `CommandJob`: irreversible, `y` only, no `--force` ever (a build-cache record is `builder prune -a -f --filter id=..`: without `-a` Docker keeps internal/frontend records), order containers, images, volumes, build cache, then the build (when missing) and the trim of the VM disk. Bulk marks take only dangling images and unused build cache; volumes are never bulk-marked; refusals live in `ItemFacts::removable`, stopped-container dependencies in `ItemFacts::needs` / `analysis::blockers` (the review's `problems`).
- The VM-disk trim is added automatically when a removal is marked (`DockerState::trim_auto`; unmarking it with removals marked sets `trim_declined`), runs from `maccleanup/vm-tools` (FROM `busybox:musl`, 2.5 MB; bump `vmtools::VERSION` when the Dockerfile changes), and the finish screen shows `Docker.raw`'s allocated size before and after (`FinishInfo::vm_disk`). `fstrim -v`'s own number is never shown as a result.
- `Docker.raw` and Docker Desktop's data folders are protected in `safety.rs`. Browsing is read-only (volume mounted `:ro`, `--network none`, images run only `sh` with `LIST_SCRIPT`); containers are not browsable. Everything we create is labelled `com.maccleanup=1`.
- The details pane wraps every line to its width and scrolls (`Action::DetailScroll`, `DockerState::detail_scroll/detail_max`); the list truncates a long command but the details never do. Optional columns (Modified on Explorer-like screens; Last used and Created on Docker) give way before names do (`MIN_NAME_*`).

### Responsiveness and background work

- Nothing slow may run on the UI thread. Moves and permanent deletes are `DeleteJob`s (`jobs.rs`): `start` for review deletes, `empty_held` for ToBeDeleted. A permanent delete is `Holding::delete_forever_with`, which uses the parallel `remove_tree` and publishes `DeleteCounters`; the job thread turns those into `Progress` events (fraction = bytes done / `of`). The manifest lock is never held while files are removed.
- Background work: any request that can repeat (list rebuilds, set-wise space) must go through `bg::Latest`, and a caller that can fire every tick must not start a new job while one is `pending` (remember the request and rerun once: see `App::rebuild_targets`). Starting a job per tick makes every result stale before it lands and hammers the clone-table locks the UI thread also needs. Show what a job is doing with `bg::Work` and a bar (`fmt::eighths_bar` for known steps, `fmt::marquee` for unknown).

### Caches & Projects list

- Products vs intermediates: `catalog/split.rs` splits a build folder (Rust `target`, SwiftPM `.build`, Xcode `build`/DerivedData, Flutter, Gradle, Maven) into rows; products get `TRow::kept`, which every bulk mark skips (like command rows). A new build system that makes binaries or dist output must come with a splitter and a test that its products are kept. Project roots follow workspace markers (`artifacts::workspace_root`), and toolchain installs are recognised by layout (`TOOLCHAIN_SIGNATURES`), never by path.
- The Caches & Projects filter (`Targets::lines`, `Query::matches_fields`) matches each term against the group's short name (`Group::label`) or the item's name, never a mix of letters from both; a group whose name matches therefore shows all its items. Keep it that way when adding fields.

### UI conventions

- Dialogs grow in over `GROW_TICKS` (`App::dialog_since`, restarted per dialog kind; content appears only once fully grown) and the congratulations window shows confetti (`ui/confetti.rs`, pure function of the tick). Both are off with `--no-animation`.
- The filter belongs to one folder's list: `App::all_rows` is the full listing and `App::rows` what is shown (all code works on `rows`). It is cleared when the folder shown changes (Into/Out/rescan-all from a subfolder), kept on a refresh of the same folder, and `Esc` clears an active filter before it goes out or quits. While `filter.editing`, every printable key is text (`input::filter_key`).
- Graph clicks: `graph::render_data` returns the clickable area of every box or bar (`GraphHit`), `ui::draw` clears `App::graph_hits` each frame and the screens store what they drew; `App::click` maps a hit to the list selection. A new graph must return its hits the same way.
- Treemap nesting and hover: `graph::render_data` returns a `GraphOut` (`hits` plus `tips`, stored with `GraphOut::store`). A treemap box whose row has a scan `Node` and is at least `NEST_MIN_W x NEST_MIN_H` cells keeps one cell of border (the top one carries the name) and draws its sub-folders plus one "(files in X)" box inside, recursively to `NEST_MAX_DEPTH`; sub-boxes are weighed like the row (`GraphData::by_size`). `fill_color(safety, selected, index, depth)` uses a darker shade set on odd depths so a box never matches the one holding it (tested). Every box and bar adds a `GraphTip` (parents before children); `Action::Hover` (mouse move) sets `App::hover`, any other action restarts `hover_since`, and `graph::tooltip` (last thing `ui::draw` paints, no dialog open) shows the innermost tip after `HOVER_TICKS`. Clicks on a nested box still select its top-level row.
- The review list scrolls (`App::review_scroll`, reset by `open_review`; `layout.review_rows` is how many items fit, written by `ui::draw` from `dialogs::review_layout`). While a review is open the navigation actions scroll it and nothing else moves; tool commands are laid out first and never scroll.
- Menu digits: a digit key is always the page's position in `SIDEBAR` (`Action::Menu(n)`, 1-based; planned pages show their "coming" notice). Never map a digit to a screen by hand; adding or moving a menu entry changes the numbers, so update the README key table.
- Screens look and behave like the Explorer wherever the content allows: same columns in the same order, same sort cycle, same graph pane and keys (`g`, `[`, `]`), same filter, totals line, scrollbar, keys and mouse. A change to the Explorer's columns, keys or sort is made on every such screen in the same commit.
- Sort order (`s`, `Action::CycleSort`) must always follow the order the column headers are drawn in the explorer (now Name, Size, On disk, Frees, Modified). Whenever a column is added, moved or removed, change the cycle, the tests and the README key table together.
- All file names shown on screen go through `ui::clean`/`fit`/`fit_start` (control characters and, in ASCII mode, non-ASCII become `?`); paths keep their tail, names keep their head.

### Testing

- Safety logic (protected paths, review/confirm, soft-delete recovery) is tested only in `tempfile` sandboxes, never against real user data. Anything that runs the binary must set a throwaway `HOME`.

## Design

Time Machine snapshots page: `docs/superpowers/specs/2026-11-07-timemachine-design.md` (built; plan in `docs/superpowers/plans/2026-11-07-timemachine.md`). Planned next: the Packages & runtimes page (`docs/superpowers/specs/2026-11-07-packages-runtimes-design.md`).

The approved design is `docs/superpowers/specs/2026-11-06-maccleanup-design.md`. Milestone 2 (Caches & Projects) has its own design, `docs/superpowers/specs/2026-11-06-caches-projects-design.md`, whose section 11 records what was built and what is deferred. The plans are in `docs/superpowers/plans/`. Follow them and update them when decisions change.

## Documentation upkeep

Always keep `CLAUDE.md` and `README.md` up to date. Whenever you add or change a command, feature, rule, key binding, build step, or convention, update both (and the design doc if a decision changed) in the same change. Commands in `README.md` must reflect what actually works.

## Git

- Blanket commit permission is granted for this repo: commit without asking first. This overrides the global "ask before each commit" rule for this project only.
