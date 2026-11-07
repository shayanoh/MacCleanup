# Time Machine snapshots page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, chosen by the user) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A "Time Machine" page that lists local snapshots, estimates what each frees, deletes marked ones through the shared review, and keeps free space live while APFS releases it.

**Architecture:** A pure module `src/snapshots.rs` (parsing, command specs, size estimate, settle state) with an App child module `src/app/snapshots.rs` (state, load/measure jobs through `bg::Latest`, marks as `tool_marks` ids `snapshot:<date>`, review commands) and a drawing module `src/ui/snapshots.rs`. Deletion is `ReviewCommand`s run by the existing `CommandJob`.

**Tech Stack:** Rust 2024, ratatui 0.30, rayon (already a dependency), `tmutil`/`diskutil` through `CommandRunner` (FakeRunner in tests).

**Spec:** `docs/superpowers/specs/2026-11-07-timemachine-design.md`

## Global Constraints

- Never escalate privileges; every command runs as the user. Nothing slow on the UI thread; jobs go through `bg::Latest`.
- Dates reach a command line only after matching `YYYY-MM-DD-HHMMSS` (digits, 4-2-2-6).
- Only `com.apple.TimeMachine.<date>.local` snapshots are markable; others are listed dimmed.
- Deletion is irreversible: `y` only, no `--force`, order oldest first. Real `tmutil` is never run in tests.
- Sidebar digit = position in `SIDEBAR` (new entry "Time Machine" before "ToBeDeleted"); update README key table, CLAUDE.md, footer hint "1-8".
- Format with `cargo fmt`; `cargo clippy --all-targets -- -D warnings` clean.

## Review Focus

- A snapshot date that is not 17 chars of digits/dashes must never become a command argument.
- Zero snapshots / `tmutil` failing: the page says so, no panic, no stale rows.
- A measure cancelled or whose mount fails leaves nothing mounted by us and shows no half number.
- Marked snapshot deleted elsewhere before `y`: the command fails, the rest still run, list rereads.
- Free space polling stops on its own (stable for 30 s or 5 min).

---

### Task 1: `snapshots.rs` pure logic

**Files:** Create `src/snapshots.rs`; Modify `src/lib.rs` (`pub mod snapshots;`).

**Interfaces:**
- Produces: `valid_date(&str) -> bool`; `parse_dates(&str) -> Vec<String>` (valid dates from `tmutil listlocalsnapshotdates` output, sorted); `parse_system_names(&str) -> Vec<String>` (non-Time-Machine `Name:` lines of `diskutil apfs listSnapshots`); `delete_spec(&str) -> Option<CommandSpec>`; `struct Snapshot { name: String, date: Option<String> }` with `Snapshot::id() -> Option<String>` (`snapshot:<date>`); `load(&dyn CommandRunner) -> Result<Vec<Snapshot>, String>`; `find_mount(root: &Path, date: &str) -> Option<PathBuf>`; `estimate_held(snap: &Path, live: &Path, cancel: &AtomicBool) -> Option<u64>`; `measure(&dyn CommandRunner, date: &str, cancel: &AtomicBool) -> Result<u64, String>`; `Settle { start(tick, free), observe(tick, free), active(tick) }`.

- [ ] Tests (names, in the module): `valid_date_accepts_only_the_tmutil_shape`, `parse_dates_keeps_valid_sorted`, `parse_system_names_skip_time_machine`, `delete_spec_refuses_bad_dates`, `estimate_counts_gone_and_changed_but_not_same_files`, `estimate_stops_when_cancelled`, `find_mount_finds_the_backupdb_layout`, `load_lists_time_machine_then_system_and_reports_failure`, `settle_ends_after_stable_free_space`.
- [ ] Implement; run `cargo test snapshots`; commit.

### Task 2: App state, jobs, marks, review, settling

**Files:** Create `src/app/snapshots.rs`; Modify `src/app.rs` (`Screen::Snapshots`, `SIDEBAR` entry, `Action::SnapMeasure { all: bool }`, `Action::SnapMarkAll`, `goto`, `move_by` order, `toggle_mark`, `open_review` screen list, `review_commands` chain, `commands_done` reload + settle, `on_tick` poll + fast free poll, click), `src/input.rs` (`m`, `M`, `a`).

**Interfaces:** Consumes Task 1. Produces on `App`: `pub snaps: SnapState` (`items`, `error`, `cursor`, `offset`, `sizes: HashMap<String,u64>`, `settle: Settle`), `reload_snapshots()`, `snap_review_commands() -> Vec<ReviewCommand>`.

- [ ] Tests (unit, sandbox `App` with `FakeRunner`): `snapshots_page_lists_what_the_runner_reports`, `space_marks_a_time_machine_snapshot_and_refuses_a_system_one`, `review_lists_marked_snapshots_oldest_first_as_one_command_each`, `finishing_a_snapshot_delete_rereads_the_list_and_starts_settling`, `m_on_a_page_without_snapshots_does_nothing`, `the_digit_for_time_machine_is_its_menu_position`.
- [ ] Implement; `cargo test`; commit.

### Task 3: UI and docs

**Files:** Create `src/ui/snapshots.rs`; Modify `src/ui/mod.rs` (module, draw match, footer hints, "1-8"), `src/ui/sidebar.rs` and `src/glyphs.rs` (icon), `tests/render.rs`, `README.md`, `CLAUDE.md`.

- [ ] Test in `tests/render.rs`: `time_machine_page_shows_snapshots_total_and_marks` (TestBackend, text contains date, "about", mark box).
- [ ] Implement; `cargo fmt`, clippy, `cargo test`; run the real binary in the pty smoke test with a throwaway `HOME`; commit.
