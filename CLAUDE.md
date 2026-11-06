# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MacCleanup is a Rust TUI (colorful, animated, mouse-supported) that finds and reports disk usage on macOS and cleans up extra files safely. Milestone 1 (scanner, explorer, safety layer, soft delete, graphs, accounting, finish screen) and milestone 2 (the Caches & Projects screen) are implemented. Leftovers, uninstaller, Docker and the other candidates are planned (design doc) and appear dimmed in the sidebar.

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
- `catalog/` what can be cleaned: `artifacts.rs` (project build output found in the scan tree, only with its marker file beside it), `tools.rs` (`discover_tools`: tool caches, slow because it asks the installed tools, so it runs on a thread), `resolve.rs` (every install of a program: `PATH`, fnm, nvm, volta), `command.rs` (`CommandSpec`, the `CommandRunner` trait, `SystemRunner`, test-only `FakeRunner`). `targets.rs` builds the Caches & Projects list (sections, groups, `TRow` = `scan::Row` plus meta); `bg.rs` is `Latest<T>`, background work where only the newest result counts.
- `filter.rs` fzf-style `Query` for the explorer's filter box (terms ANDed; fuzzy / `'exact` / `^prefix` / `suffix$` / `!exclude`; smart case; returns highlight positions).
- `ui/` drawing only (`mod.rs` helpers, `sidebar`, `home`, `explorer` (also the shared column helpers `cols`/`header_line`/`number_spans`), `graph` (takes a `GraphData`), `targets`, `holding`, `dialogs`); `glyphs.rs` Nerd/ASCII symbols; `fmt.rs` sizes and bars; `cli.rs` flags; `main.rs` terminal setup and event loop.
- Tests: unit tests sit beside the code; `tests/render.rs` (TestBackend rendering), `tests/cli_smoke.rs` (binary flags/exit codes); `testutil.rs` builds a sandbox `App` for unit tests.

## Rules and gotchas

- Deletion is a final step: list every file with its size on the review screen, confirm, then delete in the background with a progress bar. Never delete without that review.
- Soft delete is the default everywhere; permanent delete only through "move, then empty now" (`x` in the review dialog) or the ToBeDeleted screen, both behind an explicit `y` (Enter never confirms an irreversible action).
- Nothing slow may run on the UI thread. Moves and permanent deletes are `DeleteJob`s (`jobs.rs`): `start` for review deletes, `empty_held` for ToBeDeleted. A permanent delete is `Holding::delete_forever_with`, which uses the parallel `remove_tree` and publishes `DeleteCounters`; the job thread turns those into `Progress` events (fraction = bytes done / `of`). The manifest lock is never held while files are removed.
- Dialogs grow in over `GROW_TICKS` (`App::dialog_since`, restarted per dialog kind; content appears only once fully grown) and the congratulations window shows confetti (`ui/confetti.rs`, pure function of the tick). Both are off with `--no-animation`.
- On exit `main.rs` prints `Accounting::summary` (session totals plus space truly freed). `Accounting::set_pending` seeds pending from what earlier sessions left in ToBeDeleted.
- The filter belongs to one folder's list: `App::all_rows` is the full listing and `App::rows` what is shown (all code works on `rows`). It is cleared when the folder shown changes (Into/Out/rescan-all from a subfolder), kept on a refresh of the same folder, and `Esc` clears an active filter before it goes out or quits. While `filter.editing`, every printable key is text (`input::filter_key`).
- Every quit goes through `App::request_quit`: deferred while a delete job runs, a warning dialog while ToBeDeleted holds items (a second quit/Ctrl-C confirms), immediate otherwise.
- Never escalate privileges or ask for a password. For actions needing admin rights, show the exact command with a copy-to-clipboard button plus "Cancel" and "I ran the command" buttons; after the user confirms, rescan and continue. (Not built yet.)
- Show every size as both logical size and on-disk size (`fmt::both`, `usage::Usage { size, disk }`); sorting, graphs and share bars default to on-disk size. Free space is the volume's free space before vs after; soft-deleted data is "pending", never "freed".
- Caches for docker and pnpm should be cleaned through their own commands where possible, not by removing files directly. Multiple npm/pnpm/node installs may exist (fnm and others); don't assume one.
- Space is clone- and snapshot-aware (`scan.rs`). Per file the scan calls `clone_facts` (getattrlist: private size, clone id, clone count; see memory `apfs-clone-detection`). Additive per-folder counters (`Node::private`, `Node::snapshot_held`) hold the plain files; clone sets (`CloneTable`, members = containing folders) and hard-link groups (`LinkGroup`) hold the shared ones. `Scanner::space(node)` / `Scanner::list_children` (all folder rows in one pass, only when the scan is idle) turn that into `Space { footprint, frees_now, snapshots, shared_outside, shared_inside, saved }` with the identity `footprint == frees_now + snapshots + shared_inside + shared_outside` (test it). `Row.disk` of a folder is its footprint once `Row.space` is set; `Row::live` must keep it. Hard-linked data is counted once in `disk` (credited to the first folder scanned, a thread race), so `Space` adds it back for other folders; never show the credited number as what a delete frees. Tests that need clones use `cp -c` (APFS only).
- Snapshot attribution is exact, never "any gap is a snapshot" (`scan.rs`: `snapshot_may_hold`, `held_bytes`, `refresh_local_snapshots`): blame only if a local snapshot exists (`tmutil listlocalsnapshots /`, refreshed at each scan start), the file was born before the newest one, `refcnt != 0` (macOS-compressed files read private 0, refcnt 0) and only for the shortfall inside the logical size (blocks preallocated past EOF are not snapshot data). Open or just-written files also read private 0 for a moment, hence the birth-time rule.
- Tool cleanup commands (`catalog/tools.rs`): a command is pinned to one exact install (absolute program), runs with `cwd` = the home folder, `COREPACK_ENABLE_NETWORK=0`, `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`, the matching Node first on `PATH`, no stdin and a timeout. pnpm keeps one store per major (`store/vN`), so each store folder is its own row, pruned by the pnpm that reports it with `pnpm store path`; npm's cache folder comes from `npm config get cache`. Discovery and commands never run on the UI thread, and a failing query only turns that row into a plain folder move. Real commands are never run in tests: use `FakeRunner` (unit tests) or a no-op `CommandRunner` (integration tests).
- Set-wise space: what a whole selection frees is `Scanner::space_of_set(..).frees_all()` (`frees_now + shared_inside`: a clone set frees only when every holder is inside the set), computed on a thread (`App::set_space`) and shown as `App::joint_frees()`. Row Frees columns stay per-row (what deleting that row alone frees), like the Explorer.
- A review that contains tool commands is confirmed only by `y`; folder moves run first, then the commands in the listed order; a failing command neither stops the rest nor undoes the moves. Marking a group or section never marks a command row.
- Graph clicks: `graph::render_data` returns the clickable area of every box or bar (`GraphHit`), `ui::draw` clears `App::graph_hits` each frame and the screens store what they drew; `App::click` maps a hit to the list selection. A new graph must return its hits the same way.
- Products vs intermediates: `catalog/split.rs` splits a build folder (Rust `target`, SwiftPM `.build`, Xcode `build`/DerivedData, Flutter, Gradle, Maven) into rows; products get `TRow::kept`, which every bulk mark skips (like command rows). A new build system that makes binaries or dist output must come with a splitter and a test that its products are kept. Project roots follow workspace markers (`artifacts::workspace_root`), and toolchain installs are recognised by layout (`TOOLCHAIN_SIGNATURES`), never by path.
- Menu digits: a digit key is always the page's position in `SIDEBAR` (`Action::Menu(n)`, 1-based; planned pages show their "coming" notice). Never map a digit to a screen by hand; adding or moving a menu entry changes the numbers, so update the README key table.
- Background work: any request that can repeat (list rebuilds, set-wise space) must go through `bg::Latest`, and a caller that can fire every tick must not start a new job while one is `pending` (remember the request and rerun once: see `App::rebuild_targets`). Starting a job per tick makes every result stale before it lands and hammers the clone-table locks the UI thread also needs. Show what a job is doing with `bg::Work` and a bar (`fmt::eighths_bar` for known steps, `fmt::marquee` for unknown).
- One shared selection: `App::marks` is the only delete list; every screen reads and writes it, and `d` on any screen reviews everything marked. A screen never keeps its own delete list. New kinds of deletable things (tool cleanup commands, Docker items, ...) become new entry kinds in the same selection and the same review. Design: `docs/superpowers/specs/2026-11-06-caches-projects-design.md` section 4a.
- Screens look and behave like the Explorer wherever the content allows: same columns in the same order, same sort cycle, same graph pane and keys (`g`, `[`, `]`), same filter, totals line, scrollbar, keys and mouse. A change to the Explorer's columns, keys or sort is made on every such screen in the same commit.
- Sort order (`s`, `Action::CycleSort`) must always follow the order the column headers are drawn in the explorer (now Name, Size, On disk, Frees). Whenever a column is added, moved or removed, change the cycle, the tests and the README key table together.
- `Row.frees` (`Option<u64>`, None while scanning) is the Frees column and a sort key (`SortBy::Frees`); `Row::queued()` marks folders the scan has not started (`Node::is_started`). The treemap is the default graph. The explorer shows a folder-totals line above the scan totals and a scrollbar plus `a/b` position.
- Time Machine snapshots are only reported (as `snapshots`), never touched. A screen to list/thin/delete them is a separate planned phase.
- Rescans (`r` folder, `R` all) swap a fresh subtree into the tree (`Scanner::rescan_dir`/`rescan_all`) and are refused while any scan runs; hard-link bookkeeping remembers which folder counted each inode so a subtree rescan can forget exactly its own entries.
- Every scan is multithreaded for maximum speed: pool of `Scanner::default_threads()` (2x cores), a job per directory, no lock on the per-file hot path.
- All file names shown on screen go through `ui::clean`/`fit`/`fit_start` (control characters and, in ASCII mode, non-ASCII become `?`); paths keep their tail, names keep their head.
- Safety logic (protected paths, review/confirm, soft-delete recovery) is tested only in `tempfile` sandboxes, never against real user data. Anything that runs the binary must set a throwaway `HOME`.
- Anything outside the home folder is protected in this release. A folder containing a protected item is refused as a whole (`Policy::protected_descendant`, checked in `Holding::move_in` on the delete thread; `Safe` folders are not walked).
- `Holding::move_in` writes the manifest entry BEFORE the rename (an interrupted move leaves a deletable stale entry, never an untracked payload); `App` defers quitting until a running delete job finishes; `App::too_small` (set by `ui::draw`) limits input to quit.

## Design

The approved design is `docs/superpowers/specs/2026-11-06-maccleanup-design.md`; the milestone 1 plan is in `docs/superpowers/plans/`. Follow them and update them when decisions change.

## Documentation upkeep

Always keep `CLAUDE.md` and `README.md` up to date. Whenever you add or change a command, feature, rule, key binding, build step, or convention, update both (and the design doc if a decision changed) in the same change. Commands in `README.md` must reflect what actually works.

## Git

- Blanket commit permission is granted for this repo: commit without asking first. This overrides the global "ask before each commit" rule for this project only.
