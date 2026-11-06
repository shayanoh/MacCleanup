# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MacCleanup is a Rust TUI (colorful, animated, mouse-supported) that finds and reports disk usage on macOS and cleans up extra files safely. Milestone 1 is implemented: scanner, explorer, safety layer, soft delete, graphs, accounting, finish screen. Leftovers, uninstaller, caches, project artifacts, Docker and the other candidates are planned (design doc) and appear dimmed in the sidebar.

## Commands

```bash
cargo run --release                         # run (add -- --ascii, --no-animation, --root DIR, --holding-dir DIR)
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
- `ui/` drawing only (`mod.rs` helpers, `sidebar`, `home`, `explorer`, `graph`, `holding`, `dialogs`); `glyphs.rs` Nerd/ASCII symbols; `fmt.rs` sizes and bars; `cli.rs` flags; `main.rs` terminal setup and event loop.
- Tests: unit tests sit beside the code; `tests/render.rs` (TestBackend rendering), `tests/cli_smoke.rs` (binary flags/exit codes); `testutil.rs` builds a sandbox `App` for unit tests.

## Rules and gotchas

- Deletion is a final step: list every file with its size on the review screen, confirm, then delete in the background with a progress bar. Never delete without that review.
- Soft delete is the default everywhere; permanent delete only through "move, then empty now" (`x` in the review dialog) or the ToBeDeleted screen, both behind an explicit `y` (Enter never confirms an irreversible action).
- Never escalate privileges or ask for a password. For actions needing admin rights, show the exact command with a copy-to-clipboard button plus "Cancel" and "I ran the command" buttons; after the user confirms, rescan and continue. (Not built yet.)
- Free space is the volume's free space before vs after; soft-deleted data is "pending", never "freed".
- Caches for docker and pnpm should be cleaned through their own commands where possible, not by removing files directly. Multiple npm/pnpm/node installs may exist (fnm and others); don't assume one.
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
