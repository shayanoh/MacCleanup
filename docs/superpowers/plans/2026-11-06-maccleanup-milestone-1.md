# MacCleanup Milestone 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. (User instruction for this project: always execute inline with superpowers:executing-plans; never ask which method.)

**Goal:** Ship a runnable macOS TUI with a live parallel disk explorer, a hard-coded safety layer, soft delete to `~/ToBeDeleted` (browse, recover, empty), free-space accounting, two graphs (bars, treemap), animation, and a finish screen.

**Architecture:** A library crate (`maccleanup`) of small pure modules (safety, scan, softdelete, treemap, accounting, app state machine) plus a thin `ui` layer (ratatui) and `main.rs` (terminal setup, event loop). Scanning and deleting run on background threads and publish progress through atomics and `mpsc` channels, so the UI thread never blocks. All UI state changes go through `App::update(Action)`, which is unit-tested without a terminal.

**Tech Stack:** Rust 2024 (cargo 1.99), ratatui 0.30 (uses its re-exported `ratatui::crossterm`), clap 4 (derive), rayon, serde + serde_json, libc (statvfs), thiserror, anyhow, unicode-width; dev: tempfile.

**Spec:** `docs/superpowers/specs/2026-11-06-maccleanup-design.md` (read it; it also holds the requirements in sections 5, 6, 7 and 10).

## Global Constraints

- Language Rust; macOS only; Nerd Font glyphs by default, `--ascii` flag for ASCII-only; animations can be disabled (`--no-animation`). (Spec 4, 8)
- Soft delete is the default everywhere; permanent delete only via the review screen's clearly marked "move, then empty now" option with an irreversible warning. Default holding folder `~/ToBeDeleted`. (Spec 5, 6, 10)
- The app never asks for a password or escalates privileges. (Spec 10; no admin features exist in this milestone)
- "Freed" = volume free space now minus free space at session start (`statvfs`); soft-deleted bytes are "pending", never "freed". (Spec 6, 10)
- Safety logic is tested only in `tempfile` sandbox directories, never against real user data. (Spec 9)
- Protected paths cannot be marked; the deny-list is hard-coded and tested before any delete code exists. (Spec 5, 10)
- Every long operation shows live activity (spinner, counters or bar); the UI thread never blocks. (Spec 4)
- Scans are multi-threaded for maximum speed (user requirement, added 2026-11-06): the scanner pool is sized by `Scanner::default_threads()` = `available_parallelism() * 2` (directory walking is I/O-bound), one job per subdirectory, and no lock sits on the per-file hot path (the hard-link set is locked only for files with `nlink > 1`). The same applies to every later scan feature.
- Format with `cargo fmt`; add dependencies only with `cargo add`; commit after each task (blanket commit permission in CLAUDE.md).
- Keep `README.md` and `CLAUDE.md` updated in the same change as any command or rule change. (CLAUDE.md)
- Sizes are shown in decimal units (1 GB = 1,000,000,000 bytes) to match Finder and Docker. (Design choice)

## Review Focus

1. **Nested marks:** marking a folder and one of its children must not double count or delete twice (marking a parent drops child marks; marking a child of a marked parent is ignored). Pinned in Task 7.
2. **Symlinks:** a symlink pointing at a protected or outside location is moved/treated as the link only, never its target; a symlink whose canonical parent is protected is refused. Pinned in Tasks 2, 4 and 5.
3. **Tiny terminal:** rendering at e.g. 20x5 must not panic and shows a "terminal too small" notice. Pinned in Task 10.
4. **Restore problems:** restoring when the original path exists (conflict) or its parent folder is gone, and two held items with the same file name. Pinned in Task 5.
5. **Unreadable, empty or vanished directories** (and non-UTF-8 names): scanning/listing never panics, permission failures are collected for the Full Disk Access banner. Pinned in Task 4.

---

## File Structure

```
Cargo.toml
src/lib.rs        module declarations only
src/main.rs       CLI parse, terminal setup/teardown, event loop
src/cli.rs        Cli (clap)
src/glyphs.rs     Glyphs (Nerd vs ASCII)
src/fmt.rs        human_size, eighths_bar
src/safety.rs     Policy, Safety
src/volume.rs     free_space, total_space
src/perms.rs      full_disk_access_missing
src/accounting.rs Accounting
src/scan.rs       Node, Scanner, Row
src/softdelete.rs Holding, HeldItem, HoldError
src/treemap.rs    squarify
src/app.rs        App, Action, Screen, Dialog, GraphMode (state machine)
src/jobs.rs       background delete job (DeleteJob)
src/input.rs      key/mouse event -> Action
src/anim.rs       Smooth, ease_out_cubic, count_up, spinner_frame
src/ui/mod.rs     draw(frame, &App, &Theme); too-small guard
src/ui/{sidebar,home,explorer,graph,dialogs,holding,finish}.rs
tests/            integration tests per module where noted
```

---

### Task 1: Scaffold, CLI, glyphs, formatting helpers

**Files:**
- Create: `Cargo.toml` (via `cargo init --name maccleanup --lib`-style setup then add `src/main.rs`), `src/lib.rs`, `src/main.rs`, `src/cli.rs`, `src/glyphs.rs`, `src/fmt.rs`
- Test: unit tests inside `src/cli.rs`, `src/glyphs.rs`, `src/fmt.rs`

**Interfaces:**
- Produces:
  - `cli::Cli { pub root: Option<PathBuf>, pub holding_dir: Option<PathBuf>, pub ascii: bool, pub no_animation: bool }` (clap derive; `Cli::parse_from` usable in tests)
  - `glyphs::GlyphMode { Nerd, Ascii }`, `glyphs::Glyphs::new(GlyphMode)`, methods returning `&'static str`: `folder, file, cursor, marked, unmarked, safe, caution, protected, pending`; `spinner() -> &'static [&'static str]`; `eighths() -> &'static [char]` (index 0 = empty .. last = full; ASCII mode uses `[' ', '#']`)
  - `fmt::human_size(u64) -> String` (e.g. `0 B`, `999 B`, `1.0 KB`, `21.0 GB`)
  - `fmt::eighths_bar(frac: f64, width: usize, g: &Glyphs) -> String` (always exactly `width` chars; frac clamped to 0..=1; NaN treated as 0)

- [ ] **Step 1:** Run `cargo init --name maccleanup` in the repo root, then `cargo add ratatui clap --features clap/derive anyhow thiserror rayon libc unicode-width` and `cargo add serde --features derive`, `cargo add serde_json`, `cargo add --dev tempfile`. Create `src/lib.rs` declaring the modules listed above as they are added (start with `cli`, `glyphs`, `fmt`).
- [ ] **Step 2: Write failing tests**
  - `cli`: `parses_flags` (`["maccleanup","--ascii","--no-animation","--root","/tmp/x"]` gives `ascii && no_animation && root==Some("/tmp/x")`), `defaults_are_off` (all `false`/`None`).
  - `glyphs`: `ascii_mode_is_ascii_only` (every string from every method and every spinner/eighths char is ASCII), `nerd_mode_has_spinner_frames` (`spinner().len() >= 4`).
  - `fmt`: `human_size_cases` (`0`->`"0 B"`, `999`->`"999 B"`, `1000`->`"1.0 KB"`, `1_500_000`->`"1.5 MB"`, `21_000_000_000`->`"21.0 GB"`), `bar_width_is_exact` (for widths 0, 1, 10, frac -1.0, 0.0, 0.33, 1.0, 5.0, NaN: `chars().count() == width`), `bar_full_and_empty` (frac 1.0 all full char, 0.0 all empty char).
- [ ] **Step 3:** Run `cargo test` and confirm failures (items not defined).
- [ ] **Step 4:** Implement the items above with the signatures given. `human_size` uses powers of 1000 with units B, KB, MB, GB, TB and one decimal (except bytes).
- [ ] **Step 5:** Run `cargo fmt && cargo test`; expect all pass. `src/main.rs` can be a stub that parses `Cli` and prints nothing yet.
- [ ] **Step 6:** Commit `feat: scaffold crate, CLI flags, glyphs, size formatting`.

---

### Task 2: Safety policy (hard-coded deny-list)

**Files:**
- Create: `src/safety.rs`
- Test: unit tests in `src/safety.rs` using `tempfile`

**Interfaces:**
- Produces:
  - `safety::Safety { Safe, Caution, Protected }` (derive `Copy, Eq, Debug`)
  - `safety::Policy::new(home: &Path, holding: &Path) -> Policy` (canonicalizes `home`; keeps `holding` as given, canonicalizing it if it exists)
  - `Policy::classify(&self, path: &Path) -> Safety` (lexical, absolute paths only; relative paths or paths containing `..` components are `Protected`)
  - `Policy::classify_real(&self, path: &Path) -> Safety` (canonicalizes the **parent** directory and joins the final component without following a final symlink, then classifies; returns `Protected` when the parent cannot be canonicalized)
  - `Policy::can_mark(&self, path: &Path) -> bool` (= `classify_real != Protected`)

Rules (values are the spec's "conservative" set; the implementer encodes exactly these):
- `Protected`: any path outside `home`; `home` itself; the holding folder and everything inside it; direct children of `home` named `Library, Documents, Desktop, Pictures, Movies, Music, Applications, Public` (the folders themselves, not their contents); under `~/Library`: the folders `Keychains, Preferences, Mobile Documents, Application Support, Containers, Group Containers, Mail, Messages, Safari, Cookies, Accounts, Calendars, Photos, Metadata` themselves and everything under `Keychains, Preferences, Mobile Documents, Mail, Messages, Safari, Cookies, Accounts, Calendars`; `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`, `~/.config/gcloud` and everything under them; any path whose final name ends in `.photoslibrary`, `.sparsebundle` or `.sparseimage`.
- `Safe`: children of `~/Library/Caches`, `~/Library/Logs`, `~/.cache`; the directory `~/.npm`; any directory named `node_modules`, `.next`, `.turbo`, `DerivedData` (anywhere inside `home` and not otherwise protected).
- `Caution`: everything else inside `home`.

- [ ] **Step 1: Write failing tests** (sandbox `home` = `tempdir()`; create dirs as needed):
  - `outside_home_is_protected` (`/System/Library`, `/usr/bin`, a sibling tempdir all `Protected`)
  - `home_and_top_level_user_folders_are_protected` (home, `home/Documents`, `home/Library` `Protected`; `home/Documents/notes.txt` is `Caution`)
  - `library_sensitive_dirs_protected` (`home/Library/Keychains/login.keychain-db`, `home/Library/Mail/x` `Protected`; `home/Library/Application Support/Foo` is `Caution`; `home/Library/Application Support` itself `Protected`)
  - `secrets_protected` (`home/.ssh/id_rsa` `Protected`)
  - `vault_images_protected` (`home/Pictures/Photos Library.photoslibrary`, `home/x/Vault.sparseimage` `Protected`)
  - `known_caches_are_safe` (`home/Library/Caches/com.foo`, `home/.cache/uv`, `home/proj/node_modules`, `home/proj/.next` `Safe`)
  - `holding_folder_protected` (holding path and `holding/items/1/a` `Protected`)
  - `relative_and_dotdot_paths_protected` (`"foo"`, `home.join("../x")` `Protected`)
  - `symlink_into_protected_parent_refused`: create `home/safe_dir` and a symlink `home/Library/Caches/link -> home/Documents`; `classify_real(home/Library/Caches/link)` is judged on the link itself (`Safe`, the final symlink is not followed) but `classify_real(home/Library/Caches/link/notes.txt)` canonicalizes the parent to `home/Documents` and is `Caution` (not `Safe`); and a symlink whose parent resolves outside home gives `Protected`.
- [ ] **Step 2:** Run `cargo test safety` and confirm failures.
- [ ] **Step 3:** Implement `Policy` per the rules above, comparing `Path` components (never string prefixes, so `/Users/a/Library2` is not under `Library`).
- [ ] **Step 4:** `cargo fmt && cargo test safety`; expect pass.
- [ ] **Step 5:** Commit `feat: hard-coded safety policy with tests`.

---

### Task 3: Volume free space, Full Disk Access check, accounting

**Files:**
- Create: `src/volume.rs`, `src/perms.rs`, `src/accounting.rs`
- Test: unit tests in each file

**Interfaces:**
- Produces:
  - `volume::free_space(path: &Path) -> io::Result<u64>`, `volume::total_space(path: &Path) -> io::Result<u64>` (via `libc::statvfs`; free = `f_bavail * f_frsize`, total = `f_blocks * f_frsize`, widened to u64)
  - `perms::full_disk_access_missing(home: &Path) -> bool` (true only if `read_dir(home/Library/Safari)` fails with `PermissionDenied`; missing directory means `false`)
  - `accounting::Accounting::new(free_at_start: u64)`; `record_soft_delete(&mut self, bytes: u64)`, `record_restore(&mut self, bytes: u64)`, `record_emptied(&mut self, bytes: u64)` (reduces pending, saturating); `pending(&self) -> u64`; `freed(&self, free_now: u64) -> u64` (= `free_now.saturating_sub(free_at_start)`); `events(&self) -> &[Event]`; `Event { label: String, bytes: u64 }` pushed by each record call (`"moved to ToBeDeleted"`, `"restored"`, `"emptied ToBeDeleted"`); `free_at_start(&self) -> u64`

- [ ] **Step 1: Write failing tests**
  - `volume`: `free_is_positive_and_not_more_than_total` on `std::env::temp_dir()`; `error_on_missing_path` (nonexistent path -> `Err`).
  - `perms`: `absent_safari_dir_means_not_missing` (tempdir home without Library -> `false`).
  - `accounting`: `soft_delete_is_pending_not_freed` (start free 100; `record_soft_delete(40)`; `pending()==40`; `freed(100)==0`), `emptied_reduces_pending` (soft 40, emptied 40 -> pending 0; `freed(140)==40`), `freed_never_negative` (`freed(50)==0` when start is 100), `restore_reduces_pending`, `pending_saturates` (emptied more than pending -> 0).
- [ ] **Step 2:** Run `cargo test volume perms accounting` (separately if needed), confirm failures.
- [ ] **Step 3:** Implement. Use `std::ffi::CString` from `path.as_os_str().as_bytes()` (`std::os::unix::ffi::OsStrExt`) for the statvfs call; return `io::Error::last_os_error()` on failure.
- [ ] **Step 4:** `cargo fmt && cargo test`; expect pass.
- [ ] **Step 5:** Commit `feat: free-space measurement, FDA check, accounting`.

---

### Task 4: Parallel scanner

**Files:**
- Create: `src/scan.rs`
- Test: unit tests in `src/scan.rs` (sandbox trees via `tempfile`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `scan::Node` (dir node) with `pub path: PathBuf`, `pub name: OsString`; methods `size(&self) -> u64` (logical bytes in subtree), `disk(&self) -> u64` (allocated bytes: `blocks * 512`), `files(&self) -> u64`, `is_done(&self) -> bool` (whole subtree scanned), `subdir(&self, path: &Path) -> Option<Arc<Node>>`
  - `scan::Row { pub name: OsString, pub path: PathBuf, pub is_dir: bool, pub size: u64, pub disk: u64, pub files: u64, pub scanning: bool, pub node: Option<Arc<Node>> }` plus `Row::live(&self) -> Row` (re-reads size/disk/files/scanning from `node` if present, else returns a clone)
  - `scan::Scanner::default_threads() -> usize` (`available_parallelism() * 2`, minimum 2); `scan::Scanner::thread_ids_seen(&self) -> usize` (number of distinct worker thread ids that ran a directory job; test hook)
  - `scan::Scanner::new(threads: usize) -> Scanner`; `scan_root(&self, path: &Path) -> Arc<Node>` (non-blocking: returns immediately, work proceeds on a rayon pool); `list_children(&self, dir: &Arc<Node>) -> Vec<Row>` (reads that one directory; dir children get their live node or `node: None, scanning: true` if not yet discovered; files are stat'ed directly; symlinks are rows with `is_dir=false` sized as the link itself); `is_idle(&self) -> bool`; `denied(&self) -> Vec<PathBuf>` (paths that failed with `PermissionDenied`); `stats(&self) -> (u64, u64)` (files seen, bytes seen so far); `wait_idle(&self, timeout: Duration) -> bool` (for tests only; sleeps in 5 ms steps)

Behavior to implement: the job for a directory reads entries with `symlink_metadata` (never follows symlinks), skips descending into directories on a different device than the root, counts hard-linked files (`nlink > 1`) once using a `(dev, ino)` set, sums locally then adds its totals to itself and all ancestors once (parent held as `Weak<Node>`), spawns a job per subdirectory, and marks done through a pending-counter that completes the parent when children finish. `OsString` names throughout (non-UTF-8 safe).

- [ ] **Step 1: Write failing tests**
  - `sizes_sum_up`: tree `a/f1 (1000 bytes)`, `a/b/f2 (2000)`, `c/f3 (500)`; after `wait_idle`, root `size()==3500`, `files()==3`, `subdir(a).size()==3000`.
  - `disk_is_block_multiple`: root `disk() % 512 == 0` and `>= size()` for non-empty files.
  - `hardlink_counted_once`: two names for one 4000-byte file -> `size()==4000`.
  - `symlink_not_followed`: `link -> ../outside_dir` (outside has a 9000-byte file) -> root size excludes the outside file.
  - `list_children_rows`: mixed dir returns correct `is_dir`, names and file sizes; dir rows eventually have `node: Some` and `scanning==false` after idle (call `Row::live`).
  - `empty_dir_ok`: empty root -> `size()==0`, `is_done()`, `list_children` is empty.
  - `unreadable_dir_recorded`: chmod `000` a child dir -> after idle, `denied()` contains it and scan completes (restore perms at the end of the test so tempdir cleanup works; skip the test body if running as root).
  - `non_utf8_name_ok`: create a file named with bytes `[0x66, 0x80, 0x6f]` via `OsStr::from_bytes` -> counted, appears in `list_children`.
  - `vanished_dir_ok`: call `list_children` on a node whose directory was removed -> returns an empty vec, no panic.
  - `idle_flag`: `is_idle()` is `true` after `wait_idle` returns `true`.
  - `uses_multiple_threads`: a tree of 64 directories with a few small files each, scanned with `Scanner::new(4)`; after idle, `thread_ids_seen() > 1`, and totals are identical to a scan with `Scanner::new(1)` of the same tree (parallelism does not change results). Also `default_threads_is_at_least_two`.
- [ ] **Step 2:** `cargo test scan`; confirm failures.
- [ ] **Step 3:** Implement per the behavior block. Use `std::os::unix::fs::MetadataExt` (`blocks()`, `nlink()`, `dev()`, `ino()`), `rayon::ThreadPoolBuilder` with `spawn`, atomics with `Ordering::Relaxed`, `Mutex<Vec<Arc<Node>>>` for `subdirs`.
- [ ] **Step 4:** `cargo fmt && cargo test scan`; expect pass (run twice to catch flakiness).
- [ ] **Step 5:** Commit `feat: parallel scanner with live sizes and hardlink dedupe`.

---

### Task 5: Soft-delete holding folder

**Files:**
- Create: `src/softdelete.rs`
- Test: unit tests in `src/softdelete.rs` (sandbox)

**Interfaces:**
- Consumes: `safety::Policy` (`classify_real`), signatures from Task 2.
- Produces:
  - `softdelete::HoldError` (thiserror): `Protected(PathBuf)`, `CrossVolume(PathBuf)`, `Conflict(PathBuf)`, `NotFound(String)`, `Io(io::Error)`
  - `softdelete::HeldItem { pub id: String, pub original: PathBuf, pub held_name: OsString-as-String (lossy for display only), pub size: u64, pub deleted_at: u64 }` (serde); the real on-disk file lives at `dir/items/<id>/payload` so names never collide or break on odd bytes; `original` is stored as bytes-safe (serialize the path as a `Vec<u8>` via `OsStrExt`)
  - `softdelete::Holding::open(dir: PathBuf) -> io::Result<Holding>` (creates `dir`, `dir/items`, empty manifest)
  - `Holding::dir(&self) -> &Path`; `move_in(&self, policy: &Policy, path: &Path, size: u64) -> Result<HeldItem, HoldError>` (refuses `Protected`, refuses paths inside the holding dir, `rename`s into `items/<id>/payload`, maps `EXDEV` to `CrossVolume`, records manifest); `list(&self) -> io::Result<Vec<HeldItem>>`; `total_size(&self) -> io::Result<u64>` (sum of manifest sizes); `restore(&self, id: &str) -> Result<PathBuf, HoldError>` (fails with `Conflict` if the original path exists; creates missing parent directories; removes the item from the manifest); `delete_forever(&self, id: &str) -> Result<u64, HoldError>` (removes only `items/<id>`, returns its recorded size); `empty(&self) -> Result<u64, HoldError>` (deletes all items, returns total bytes)
  - The manifest is `dir/manifest.json`, rewritten atomically (write temp then rename) on each change.

- [ ] **Step 1: Write failing tests** (sandbox home with a `Policy`; holding at `home/ToBeDeleted`):
  - `move_and_list`: moving `home/proj/a.txt` removes it from its place, `list()` has one item with `original`, `total_size()==size`.
  - `protected_is_refused`: `move_in(home/Documents)` -> `Err(Protected)` and the folder still exists.
  - `cannot_move_holding_into_itself`: `move_in(holding.dir())` -> `Err(Protected)`.
  - `symlink_moves_link_not_target`: link `home/proj/l -> home/proj/real`; moving the link leaves `real` intact and removes `l`.
  - `restore_roundtrip`: move then `restore` puts the file back with the same content, manifest empty.
  - `restore_conflict`: recreate a file at the original path, then `restore` -> `Err(Conflict)`, item stays held.
  - `restore_recreates_missing_parent`: remove the original parent dir, then `restore` recreates it.
  - `same_name_items_do_not_collide`: two files `x/a.txt` and `y/a.txt` both moved; both restorable to the right places.
  - `delete_forever_and_empty`: `delete_forever` returns recorded size and frees the payload; `empty` returns total and leaves `list()` empty; neither touches anything outside `items/`.
  - `unknown_id_is_not_found`.
  - `cross_volume_error_mapping`: unit-test the helper that maps `io::ErrorKind`/raw os error 18 (`EXDEV`) to `CrossVolume`.
  - `manifest_survives_reopen`: `Holding::open` again on the same dir lists the same items.
- [ ] **Step 2:** `cargo test softdelete`; confirm failures.
- [ ] **Step 3:** Implement. Item ids are `"{unix_nanos}-{counter}"` from a process-wide `AtomicU64` counter. All destructive calls (`remove_dir_all`, `remove_file`) must be guarded by a helper that checks the target is inside `dir/items` after canonicalizing.
- [ ] **Step 4:** `cargo fmt && cargo test softdelete`; expect pass.
- [ ] **Step 5:** Commit `feat: soft-delete holding folder with restore and empty`.

---

### Task 6: Treemap layout

**Files:**
- Create: `src/treemap.rs`
- Test: unit tests in `src/treemap.rs`

**Interfaces:**
- Produces: `treemap::CellRect { pub x: u16, pub y: u16, pub w: u16, pub h: u16 }`; `treemap::squarify(weights: &[u64], w: u16, h: u16) -> Vec<Option<CellRect>>` (one entry per input weight, same order; `None` for zero weight or when the item's rectangle would be smaller than 1x1 cell). Squarified algorithm over the `w x h` area with terminal-cell aspect compensation (treat each cell as 1 wide by 2 tall when computing aspect ratios); output rects are integer cells and must tile the area without overlap.

- [ ] **Step 1: Write failing tests:**
  - `empty_and_zero_area`: `squarify(&[], 10, 10)` is empty; `squarify(&[5], 0, 5)` is `[None]`.
  - `single_item_fills_area`: `[Some(CellRect{0,0,10,5})]`.
  - `rects_inside_bounds_and_disjoint`: for weights `[60,30,20,10,5,1]` in 40x12, every rect is within bounds and no two rects overlap (check cell-by-cell with a grid).
  - `covers_whole_area_when_all_visible`: for weights `[4,3,2,1]` in 20x10, summed rect areas == 200.
  - `bigger_weight_gets_bigger_or_equal_area`: area order matches weight order (non-strict) for `[40,20,10]`.
  - `zero_weight_is_none`: `[10,0,10]` -> middle `None`.
  - `tiny_items_dropped_not_overlapping`: `[1000,1,1,1]` in 8x4 -> no overlaps, all `Some` rects in bounds.
- [ ] **Step 2:** `cargo test treemap`; confirm failures.
- [ ] **Step 3:** Implement squarify with the last-row remainder absorbing rounding so areas tile exactly.
- [ ] **Step 4:** `cargo fmt && cargo test treemap`; expect pass.
- [ ] **Step 5:** Commit `feat: squarified treemap layout`.

---

### Task 7: App state machine and input mapping

**Files:**
- Create: `src/app.rs`, `src/input.rs`
- Test: unit tests in both files

**Interfaces:**
- Consumes: `scan::{Scanner, Node, Row}`, `safety::{Policy, Safety}`, `softdelete::Holding`, `accounting::Accounting`, `glyphs::Glyphs`.
- Produces:
  - `app::Screen { Home, Explorer, Holding }`, `app::Focus { Sidebar, Main }`, `app::GraphMode { Bars, Treemap, Hidden }`, `app::SortBy { Size, Name }`
  - `app::Action { Quit, Up, Down, PageUp, PageDown, Top, Bottom, Into, Out, ToggleMark, OpenReview, CycleGraph, GrowGraph, ShrinkGraph, CycleSort, FocusToggle, GoTo(Screen), Click { x: u16, y: u16 }, Scroll(i16), Tick, Confirm, Cancel, ConfirmIrreversible, RestoreSelected, DeleteSelectedForever, EmptyAll }`
  - `app::App::new(scanner: Scanner, root: Arc<Node>, policy: Policy, holding: Holding, accounting: Accounting, glyphs: Glyphs, animations: bool) -> App`
  - Fields (pub): `screen`, `focus`, `graph`, `graph_pct: u16` (clamped 20..=70, step 5, default 35), `sort`, `rows: Vec<Row>`, `cursor: usize`, `marks: BTreeSet<PathBuf>`, `dialog: Option<Dialog>`, `should_quit: bool`, `stack: Vec<Arc<Node>>` (path of directories entered; last is current), `held: Vec<HeldItem>`, `held_cursor: usize`, `accounting: Accounting`, `free_now: u64`, `total_space: u64`, `tick: u64`, `notice: Option<String>`
  - `app::Dialog { Review(Review), Irreversible(Review), Deleting(DeleteProgress), Finish(FinishInfo), Notice(String) }`, `app::Review { items: Vec<ReviewItem>, total: u64 }`, `app::ReviewItem { path: PathBuf, size: u64, safety: Safety }`
  - `App::update(&mut self, action: Action)`; `App::refresh_rows(&mut self)` (re-lists current dir via `Scanner::list_children`, applies sort, keeps the cursor on the same path if still present); `App::current(&self) -> &Arc<Node>`; `App::selected_row(&self) -> Option<&Row>`; `App::marked_total(&self) -> u64`
  - `input::map_key(key: KeyEvent, app: &App) -> Option<Action>`: `q`/Esc-at-top quit (Esc cancels dialogs first), `j/k/Down/Up`, `h/Left` -> Out, `l/Right/Enter` -> Into (or Confirm in a dialog), `g/G` -> Top/Bottom (`gg` not required; `g` cycles graph, `Home`/`End` are Top/Bottom; use `G` for Bottom), space -> ToggleMark, `d` -> OpenReview, `s` -> CycleSort, `[` / `]` -> ShrinkGraph/GrowGraph, `Tab` -> FocusToggle, `1`/`2`/`3` -> GoTo, `x` in Review -> ConfirmIrreversible, `y` in Irreversible -> Confirm, in Holding screen `r` -> RestoreSelected, `D` -> DeleteSelectedForever, `E` -> EmptyAll; Ctrl-C quits. `input::map_mouse(ev: MouseEvent, app: &App) -> Option<Action>`: left click -> `Click{x,y}`, wheel -> `Scroll(±3)`.

Rules for `update`:
- Marking a protected path is refused with a `notice`; marking a path whose ancestor is marked is ignored; marking a path removes any marks beneath it (Review Focus 1).
- `OpenReview` with no marks sets a notice; otherwise builds `Dialog::Review` (items sorted by size desc, safety from `policy.classify_real`, total = sum).
- `Confirm` in Review starts the soft delete (hands the items to the `jobs` module in Task 8; until then it calls `Holding::move_in` synchronously inside a test seam `fn run_delete_sync`); `ConfirmIrreversible` moves to `Dialog::Irreversible`; `Confirm` in Irreversible runs move-then-empty.
- `Into` on a dir row pushes its node (if the row has a node) and refreshes; on a file it does nothing; `Out` pops (never past the scan root).
- `CycleGraph` order: `Bars -> Treemap -> Hidden -> Bars`; `GrowGraph/ShrinkGraph` change `graph_pct` by 5 within 20..=70.
- Click on a main-pane row index selects it (second click on the selected row = `Into`); click on a sidebar entry = `GoTo`; the click geometry comes from `app.layout` (a `Layout { sidebar: Rect, main: Rect, list_top: u16 }` field that `ui::draw` writes each frame, default zeros so tests set it).

- [ ] **Step 1: Write failing tests** (build `App` over a sandbox tree after `scanner.wait_idle`):
  - `down_up_clamp` (cursor stays within `0..rows.len()`).
  - `into_and_out_navigate` (enter `a`, rows show `a`'s children; `Out` returns; `Out` at root stays).
  - `mark_toggle_and_total` (`marked_total()` equals the sum of marked row sizes; second toggle unmarks).
  - `nested_marks_are_deduped`: mark child, then mark parent -> only parent in `marks`; mark child again -> ignored.
  - `protected_cannot_be_marked`: a protected row's mark attempt leaves `marks` empty and sets `notice`.
  - `open_review_requires_marks_and_sorts_by_size`.
  - `review_confirm_moves_files_to_holding`: after `Confirm` the files are gone from the sandbox, `held.len()==marked count`, `accounting.pending()==total`, `marks` empty.
  - `irreversible_flow_requires_second_confirmation`: `ConfirmIrreversible` shows `Dialog::Irreversible` and deletes nothing; `Confirm` then empties (files gone, `held` empty, pending 0).
  - `cancel_closes_dialog_and_keeps_files`.
  - `graph_cycle_and_resize_bounds` (`graph_pct` never leaves 20..=70).
  - `sort_toggle_orders_rows` (by size desc vs name asc).
  - `click_selects_then_enters`.
  - `holding_screen_restore_and_delete`: with items held, `RestoreSelected` puts the file back and lowers pending; `DeleteSelectedForever` removes the payload.
  - `input_key_mapping_table`: a table of `(KeyCode, Option<Action>)` including dialog-sensitive keys (`x` only in Review, `y` only in Irreversible).
- [ ] **Step 2:** `cargo test app input`; confirm failures.
- [ ] **Step 3:** Implement `App` and the mappers as specified. Keep `update` free of terminal or clock dependencies (time comes in only via `Tick`).
- [ ] **Step 4:** `cargo fmt && cargo test`; expect pass.
- [ ] **Step 5:** Commit `feat: app state machine and input mapping`.

---

### Task 8: Background delete job, progress and finish info

**Files:**
- Create: `src/jobs.rs`; Modify: `src/app.rs`
- Test: unit tests in `src/jobs.rs`; extend `src/app.rs` tests

**Interfaces:**
- Consumes: `softdelete::Holding`, `safety::Policy`, `app::Review`.
- Produces:
  - `jobs::DeleteMode { SoftOnly, SoftThenEmpty }`
  - `jobs::DeleteEvent { Progress { done: usize, total: usize, bytes: u64, current: PathBuf }, Failed { path: PathBuf, error: String }, Finished { moved_bytes: u64, emptied_bytes: u64, failures: usize } }`
  - `jobs::DeleteJob::start(holding: Arc<Holding>, policy: Arc<Policy>, items: Vec<(PathBuf, u64)>, mode: DeleteMode) -> DeleteJob` (spawns a thread); `DeleteJob::poll(&self) -> Vec<DeleteEvent>` (non-blocking drain of an `mpsc` receiver)
  - `app::DeleteProgress { done: usize, total: usize, bytes: u64, current: PathBuf }`, `app::FinishInfo { moved_bytes: u64, emptied_bytes: u64, free_before: u64, free_after: u64, failures: usize }`
  - `App` gains `job: Option<DeleteJob>`; `Action::Tick` polls it, updates `Dialog::Deleting`, and on `Finished` records accounting (`record_soft_delete`, `record_emptied`), re-reads `free_now`, refreshes rows and `held`, and shows `Dialog::Finish` (for `SoftThenEmpty`) or a `Dialog::Notice("Moved X to ToBeDeleted (pending, not freed yet)")` (for `SoftOnly`).
  - `Holding` must be shareable across threads (`Arc<Holding>`); `Holding` methods already take `&self`.

- [ ] **Step 1: Write failing tests:**
  - `soft_only_job_moves_items_and_reports`: collects events until `Finished`; `moved_bytes==sum`, `emptied_bytes==0`, `Progress` events are monotonic in `done`.
  - `soft_then_empty_job_removes_everything`: `emptied_bytes==moved_bytes`, `holding.list()` empty.
  - `protected_item_fails_but_job_continues`: one protected + one normal -> one `Failed` event, normal item still moved, `failures==1`.
  - `app_tick_drives_job_to_finish_dialog`: through `App::update(Confirm)` + repeated `Tick` (bounded wait loop), the dialog ends as `Notice`/`Finish` and `accounting` matches.
- [ ] **Step 2:** `cargo test jobs app`; confirm failures.
- [ ] **Step 3:** Implement. Replace Task 7's `run_delete_sync` seam with the job; keep a `#[cfg(test)]` helper to wait for the job to finish.
- [ ] **Step 4:** `cargo fmt && cargo test`; expect pass.
- [ ] **Step 5:** Commit `feat: background delete job with progress and finish accounting`.

---

### Task 9: Animation helpers

**Files:**
- Create: `src/anim.rs`
- Test: unit tests in `src/anim.rs`

**Interfaces:**
- Produces: `anim::ease_out_cubic(t: f64) -> f64` (clamped input 0..=1, output 0..=1, monotonic); `anim::spinner_frame(tick: u64, frames: usize) -> usize` (advances every 2 ticks, wraps); `anim::count_up(from: u64, to: u64, elapsed_ticks: u64, duration_ticks: u64) -> u64` (eased, reaches `to` exactly at/after duration, works when `to < from`, never overflows); `anim::Smooth { pub shown: f64 }` with `Smooth::new(v: f64)`, `step(&mut self, target: f64, rate: f64)` (moves a fraction `rate` toward target, snaps when within 0.5), `value(&self) -> f64`; `anim::wipe_columns(progress: f64, width: u16) -> u16` (how many columns of a transition are revealed, 0..=width).

- [ ] **Step 1: Write failing tests:** `ease_bounds_and_monotonic`, `spinner_wraps_and_advances_slowly` (frames 0,0,1,1,2,2,3,3,0 for 4 frames), `count_up_endpoints` (`elapsed 0 -> from`, `elapsed >= duration -> to`), `count_up_descending`, `count_up_no_overflow` (`u64::MAX` bounds), `smooth_converges_and_snaps`, `wipe_columns_bounds`.
- [ ] **Step 2:** `cargo test anim`; confirm failures.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo fmt && cargo test anim`; expect pass.
- [ ] **Step 5:** Commit `feat: animation helpers`.

---

### Task 10: UI rendering

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/sidebar.rs`, `src/ui/home.rs`, `src/ui/explorer.rs`, `src/ui/graph.rs`, `src/ui/dialogs.rs`, `src/ui/holding.rs`, `src/ui/finish.rs`
- Test: `tests/render.rs` using ratatui's `TestBackend`

**Interfaces:**
- Consumes: `App` fields and `Dialog` variants from Tasks 7-8, `Glyphs`, `fmt`, `anim`, `treemap::squarify`, `volume`.
- Produces: `ui::draw(frame: &mut Frame, app: &mut App)` (writes `app.layout` each frame; renders sidebar + active pane + footer key hints + dialogs on top); `ui::MIN_WIDTH: u16 = 60`, `ui::MIN_HEIGHT: u16 = 14` (below either, draws only a centered "Terminal too small (need 60x14)" message); one render fn per module taking `(frame, area, &App)`.

Screen content to implement (from the approved mockups):
- **Sidebar:** entries Home, Explorer, Leftovers (dim "soon"), Uninstall (dim "soon"), Caches (dim "soon"), Docker (dim "soon"), Projects (dim "soon"), ToBeDeleted; active entry highlighted; bottom shows `pending <size>`.
- **Home:** header with volume name-less label "Disk", a gauge `Free <size> / <total>` with a smoothly animated bar, the pending line, scan activity line (spinner + "files seen / bytes seen" while the scanner is not idle, a check mark when done), "Biggest folders" top 5 subdirectories of the scan root by live size, and the FDA banner when `perms::full_disk_access_missing` or `scanner.denied()` is non-empty.
- **Explorer:** header `Explorer · <path> · scanning… <files> files · <bytes>` with spinner while not idle; rows with cursor glyph, mark glyph, folder/file icon, safety dot (green/amber/red), name (truncated by display width with `unicode-width`, lossy for non-UTF-8), size, on-disk size, share bar (eighths) and a per-row spinner while `scanning`; footer line `marked: N items, X (not deleted yet)`; the graph panel on the right according to `graph` and `graph_pct`.
- **Graph panel:** `Bars` = horizontal eighths bars for the top rows with the cursor row highlighted and a `selected: <name> <pct>% of <dir>` line; `Treemap` = rects from `squarify` over the current rows' sizes, filled with safety colors, label inside when it fits, the selected rect drawn with a double border or inverted style, marked rects hatched (`░`/ASCII `/`); `Hidden` = no panel (list takes the full width).
- **Holding screen:** list of held items with original path, size, date; total; keys hint.
- **Dialogs:** Review (list with safety dots and totals, `Enter` move to ToBeDeleted, `x` move-then-empty (irreversible), `Esc` cancel), Irreversible (red warning "cannot be undone", `y` confirm, `Esc` cancel), Deleting (progress bar `done/total`, bytes, current path), Finish (count-up of freed bytes via `anim::count_up`, congratulations text, list of `accounting.events()`), Notice.
- Colors: Safe=green, Caution=amber/yellow, Protected=red; dim gray for disabled items. `--ascii` replaces every glyph via `Glyphs` (including box-drawing: use `ratatui::symbols::border::PLAIN`-style ASCII borders via `Block::border_set` with an ASCII `Set`).

- [ ] **Step 1: Write failing tests** in `tests/render.rs` (render into `TestBackend::new(w,h)` and join the buffer to a string):
  - `too_small_shows_notice_and_does_not_panic` for sizes (20,5), (59,30), (100,13), (0,0), (1,1).
  - `home_shows_free_space_and_pending` (contains `Free`, a size string, `pending`).
  - `explorer_lists_rows_with_sizes` (names and a `human_size` string present; cursor row marked).
  - `marked_footer_text` (after marking: `marked: 1 items`).
  - `graph_modes_render` (Bars shows the `selected:` line; Treemap shows a selected name inside a rect for a big enough area; Hidden shows neither and list is wider).
  - `ascii_mode_has_only_ascii_cells` for Home, Explorer (all three graph modes), Holding, and Review dialog (every cell symbol is ASCII).
  - `review_dialog_shows_total_and_keys` (contains `Enter`, `x`, `Esc`, and the total).
  - `irreversible_dialog_warns` (contains `cannot be undone`).
  - `finish_dialog_shows_congratulation` (contains the freed amount's human size after enough ticks).
  - `fda_banner_when_denied` (scanner with a denied dir, or the flag injected through `App`, shows `Full Disk Access`).
- [ ] **Step 2:** `cargo test --test render`; confirm failures.
- [ ] **Step 3:** Implement the UI modules per the content above; sidebar and main use `Layout::horizontal([Constraint::Length(16), Constraint::Fill(1)])`; popups use `Clear` before drawing; the screen-switch wipe uses `anim::wipe_columns` to blank the not-yet-revealed columns of the main pane when `animations` is on.
- [ ] **Step 4:** `cargo fmt && cargo test`; expect pass.
- [ ] **Step 5:** Commit `feat: ratatui UI for home, explorer, graphs, holding, dialogs`.

---

### Task 11: Wire up `main`, terminal lifecycle, smoke test

**Files:**
- Modify: `src/main.rs`; Create: smoke script in the scratchpad directory (not in the repo)
- Test: `tests/cli_smoke.rs` (spawns the built binary with `--help` and `--version`)

**Interfaces:**
- Consumes: everything above.
- Produces: the `maccleanup` binary. Behavior: parse `Cli`; resolve `home` from `HOME` (error with a clear message if unset); `root` = `--root` or home; `holding_dir` = `--holding-dir` or `home/ToBeDeleted`; validate that `root` exists and is a directory (print an error and exit 2 otherwise); open `Holding`, build `Policy`, `Scanner::new(Scanner::default_threads())`, `scan_root`, read initial free space for `Accounting`; install a panic hook that disables mouse capture then calls the previous hook; `ratatui::init()` and enable mouse capture; loop at ~20 fps: `terminal.draw(|f| ui::draw(f, &mut app))`, `event::poll(50 ms)`, map key presses (ignore non-`Press` kinds), mouse and resize events to actions, always send `Action::Tick`, refresh rows every 10 ticks while the scanner is busy and the current directory is not done; on exit disable mouse capture and `ratatui::restore()`.

- [ ] **Step 1: Write failing test** `tests/cli_smoke.rs`: `help_lists_flags` runs `env!("CARGO_BIN_EXE_maccleanup") --help` and asserts the output contains `--ascii`, `--no-animation`, `--root`, `--holding-dir`; `bad_root_exits_2` runs with `--root /definitely/not/here` and asserts exit code 2 and an error message on stderr.
- [ ] **Step 2:** `cargo test --test cli_smoke`; confirm failures.
- [ ] **Step 3:** Implement `main.rs` as described (no terminal setup before the argument validation so `--help` and errors work without a TTY).
- [ ] **Step 4:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`; expect all pass.
- [ ] **Step 5: Real-terminal smoke run:** write a Python script in the scratchpad directory that uses `pty.fork()` to run `target/debug/maccleanup --root <a tempdir with sample files> --holding-dir <tempdir>/hold`, sets the PTY size to 100x30, reads output for about 1.5 s, asserts that output contains `Explorer`-or-`Free`-style text and no panic message, sends `q`, and asserts the process exits with status 0 within 3 s. Run it (`python3 -I`).
- [ ] **Step 6:** Commit `feat: wire up terminal app and smoke tests`.

---

### Task 12: Documentation and wrap-up

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `docs/superpowers/specs/2026-11-06-maccleanup-design.md` (status line only)

- [ ] **Step 1:** README: replace the "design phase" status with "Milestone 1 implemented"; list what works now (explorer, soft delete, graphs, accounting) versus what is planned (leftovers, uninstaller, caches, projects, Docker, other candidates); document real commands (`cargo run --release`, flags, `cargo test`, `cargo clippy`, `cargo fmt`), key bindings, and the first-run notes (Full Disk Access).
- [ ] **Step 2:** CLAUDE.md: add the real build/test/lint commands, the module map (one line per module), the key-binding location (`src/input.rs`), and the rule that every new feature updates README, CLAUDE.md and the design doc.
- [ ] **Step 3:** Run `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`; all must pass. Delete nothing; confirm `git status` is clean after committing.
- [ ] **Step 4:** Commit `docs: update README and CLAUDE.md for milestone 1`.

---

## Self-review notes

- Spec coverage: explorer (T4, T7, T10), vim keys and mouse (T7), soft delete with recover/empty (T5, T7, T8, T10), safety deny-list (T2), review and irreversible warnings (T7, T8, T10), accounting with pending versus freed and finish screen (T3, T8, T10), two graphs resizable and hideable (T6, T7, T10), glyph/ASCII modes and animation (T1, T9, T10), responsiveness via background workers (T4, T8, T11), FDA banner (T3, T10), tiny terminal (T10). Deferred to later milestones by design: leftovers, uninstaller, caches, project artifacts, Docker, other candidates, admin-command flow, "clean suggested", before/after wrappers for external tools, treemap zoom morph animation.
- Home "Biggest folders" stands in for "suggested wins" until later milestones supply real suggestions (design choice).
