# Packages & runtimes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox syntax.

**Goal:** A "Packages & runtimes" page listing Node versions (fnm, nvm) with their global packages, then corepack, uv, Rust, cargo, Android SDK and simulators, removable through each manager's own command.

**Architecture:** The page reuses the Caches & Projects list machinery (`targets.rs`, `ui/targets.rs`, `App::targets`, `tool_marks`, `ReviewCommand`). `SectionKind` grows new kinds; each belongs to one *page* (`Page::Caches` or `Page::Runtimes`); `Targets::lines` takes the page; `Screen::Runtimes` is the same list state shown for the runtime sections. Discovery lives in `catalog/runtimes.rs` and yields `(SectionKind, ToolEntry)` pairs, built on a thread beside `discover_tools`.

**Tech Stack:** Rust 2024, ratatui 0.30, existing `CommandRunner`/`FakeRunner`, tempfile sandboxes.

**Spec:** `docs/superpowers/specs/2026-11-07-packages-runtimes-design.md`

## Global Constraints

- Add dependencies only with `cargo add`; `cargo fmt`; `cargo clippy --all-targets -- -D warnings` clean.
- Never run a real tool command in tests: `FakeRunner` or a sandbox `HOME`.
- Commands: absolute program, `cwd` = home, no stdin, never `--force`, irreversible, `y` only.
- Nothing slow on the UI thread; list builds go through `bg::Latest`.
- Menu digits follow `SIDEBAR` position; update the README key table when it changes.
- Update `CLAUDE.md`, `README.md` and the Caches design doc section 11 in the same change.

## Review Focus

- A Node dir with no `installation` (half-removed install) is listed with size 0 or skipped, never a panic.
- A project file (`.nvmrc`, `package.json`) that is empty, huge, binary or invalid JSON is ignored.
- `.nvmrc` values `lts/*`, `node`, `v22`, `22.1`, `v22.1.0` all parse or are ignored, never match the wrong version.
- The default alias is a dangling symlink: the version is simply not marked default.
- A version marked together with its own global packages runs one command, not two.

---

### Task 1: Node discovery (`catalog/runtimes.rs`)

**Files:** Create `src/catalog/runtimes.rs`; modify `src/catalog/mod.rs`.

**Interfaces:**
- Produces: `pub struct NodeInstall { pub manager: Manager, pub version: String, pub root: PathBuf /* <version>/installation (fnm) or <version> (nvm) */, pub dir: PathBuf /* what is removed: the version folder */, pub is_default: bool }`, `pub enum Manager { Fnm, Nvm }`
- `pub fn node_installs(home: &Path, places: &Places) -> Vec<NodeInstall>` (newest first; default via the `default` alias symlink: fnm `aliases/default`, nvm `alias/default` file)
- `pub fn node_globals(install: &NodeInstall) -> Vec<(String /* package name, scoped names joined */, PathBuf)>` from `<root>/lib/node_modules`, skipping `npm` and `corepack` (they ship with Node)
- `pub fn pins_for(version: &str, projects: &[PathBuf]) -> Vec<PathBuf>`: project roots whose `.nvmrc`/`.node-version`/`package.json` (`engines.node`, `volta.node`) name this version's major (or exact version)

- [ ] Write tests (tempdir home): lists two fnm versions newest first; marks the aliased one default; nvm layout; globals skip `npm`/`corepack` and join `@scope/name`; `pins_for` with each file kind and the Review Focus values.
- [ ] Run `cargo test runtimes` — fail.
- [ ] Implement the three functions; pin parsing keeps the first run of digits as the major and an exact `x.y.z` when present; ranges (`>=18`) are ignored; read at most 64 KiB per file.
- [ ] `cargo test runtimes` — pass. Commit.

### Task 2: Entries and commands

**Files:** `src/catalog/runtimes.rs`.

**Interfaces:**
- Consumes: Task 1; `tools::pinned`-style command pinning (make `pinned` `pub(crate)`).
- Produces: `pub fn discover_runtimes(home: &Path, runner: &dyn CommandRunner, places: &Places, projects: &[PathBuf], work: &Work) -> Vec<(SectionKind, ToolEntry)>`.
  - Node version: id `node:<manager>:<version>`, group `Node (fnm)` / `Node (nvm)`, label = version, `Mode::Command(fnm uninstall <v>)` (`fnm` resolved with `resolve_in`; nvm: `bash -c 'source "$NVM_DIR/nvm.sh" && nvm uninstall <v>'` with `NVM_DIR` set), `note` = "default", "pinned by N projects" and "old" parts.
  - Global package: id `node-global:<version>:<name>`, group `Node <version> packages`, `Mode::Command(npm uninstall -g <name>)` with that version's `npm` first on PATH; `path` = package folder.

- [ ] Tests with `FakeRunner`: commands are exact (`fnm uninstall v22.1.0`, `npm uninstall -g @scope/pkg`), pinned absolute program, `cwd` home, env `COREPACK_ENABLE_NETWORK=0`; the default version's note says default; old = older than the default.
- [ ] Implement; commit.

### Task 3: Page plumbing

**Files:** `src/targets.rs`, `src/app.rs`, `src/ui/mod.rs`, `src/ui/targets.rs`, `src/ui/sidebar.rs`, `src/input.rs`, `README.md`, `CLAUDE.md`.

**Interfaces:**
- `SectionKind::{NodeVersions, NodeGlobals}`; `impl SectionKind { pub fn page(self) -> Page }`, `pub enum Page { Caches, Runtimes }`
- `Targets::lines(&self, filter: &Query, page: Page) -> Vec<Line>`; `BuildSource.runtimes: Vec<(SectionKind, ToolEntry)>`; dedupe of nested paths only within one page
- `Screen::Runtimes`; `App::on_targets(&self) -> bool` (Targets or Runtimes) replaces every `self.screen == Screen::Targets`; new SIDEBAR entry "Packages & runtimes" after Time Machine

- [ ] Tests: `Targets::lines` for `Page::Runtimes` shows only runtime sections; a version row and its package rows both survive the dedupe; the sidebar has the entry and digits follow position (update existing digit tests); marking a runtime command row works and the review lists it.
- [ ] Implement; section titles `NODE VERSIONS`, `NODE GLOBAL PACKAGES`; Runtimes-page header text; discovery thread also calls `discover_runtimes` with the Caches project roots.
- [ ] Mark rule: marking a Node version removes the marks of its global packages (`node-global:<version>:*`); marking a package of a marked version shows a notice; bulk-marking a Node versions section marks only old, not-default, unpinned versions.
- [ ] `cargo test`, `cargo clippy`, smoke script; commit.

### Task 4: Corepack, uv Pythons, uv tools, Rust toolchains, cargo binaries

One `discover_*` function each in `runtimes.rs`, entries as in the spec table. Verify each command against current docs or `--help` before use. Fixtures under `tests/fixtures/runtimes/`. Commit per tool.

### Task 5: Android SDK and iOS simulators

`package.xml` parsing; `sdkmanager --uninstall`; `xcrun simctl list -j`; booted device refused; never a folder move for simulators. Commit.

### Task 6: Leftovers and docs

Row switches to a folder move when its command ran and the folder remains; final README/CLAUDE.md/spec updates.

## Self-review

Spec coverage: sections (Tasks 2, 4, 5), flags (1, 2, 3), bulk rule (3), leftovers (6), docs (3, 6). Types: `Page`, `NodeInstall`, `discover_runtimes` named identically across tasks. Proportion: tasks 4 to 6 are intentionally short; each repeats the Task 2 pattern.
