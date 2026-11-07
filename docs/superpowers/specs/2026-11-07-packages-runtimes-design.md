# Packages & runtimes page (milestone 5)

Status: design agreed in conversation 2026-11-07, awaiting approval to plan.
Replaces the earlier stub. Calls that were mine, not the user's, are marked
**(my call)**.

## Why

After each Node upgrade the old version stays in fnm, with every global npm
package installed under it, and nobody sees it. The same goes for old pnpm
versions in the corepack cache, Python builds from uv, Rust toolchains, cargo
binaries, uv tools, Android SDK pieces and iOS simulators. Caches & Projects
is about things that come back by themselves; these are installs. The user
wants one place that lists every runtime version and global package with its
size, flags what is old, and removes it the standard way.

## Goals

- One page, "Packages & runtimes", one section per kind, looking and behaving
  like Caches & Projects (sections, groups, `TRow` rows, Size / On disk /
  Frees / Modified columns, graph pane, filter, totals line, same keys and
  mouse). **(my call)** Reuse that list machinery (`targets.rs`, `ui/targets.rs`)
  with new row sources instead of building a second list.
- List, with size: every install the user has and may not know about.
- Mark old and unwanted ones, review them in the one shared review (`tool_marks`
  and `d`), remove them irreversibly with `y` only.
- Prefer the manager's own uninstall command. Whatever a command leaves behind
  stays visible and can be moved to ToBeDeleted by hand.

## Non-goals and anti-goals

- No Homebrew (its files are under `/usr/local`; the user wants a separate
  page later). No Go, Deno or JDKs (they live in `/usr` and `/Library`, outside
  the home folder, which is protected in this release). No dotnet, bun, volta,
  pyenv: not installed here. Where a manager is cheap to detect, show it; do
  not build removal for what cannot be tried. **(my call)** nvm is supported
  because the user asked, but it is not installed here, so it is only tested
  with `FakeRunner`; volta is left out.
- Never fail by deleting something in use without a warning, never `--force`,
  never escalate privileges, never run a real tool command in tests.
- Anti-goal: a list that makes the user guess what is safe. Each row says why
  it is flagged, or why it is refused.

## Sections and what each removes

| Section | Rows | Removal |
|---|---|---|
| Node versions | each fnm install (`fnm list`, dir from `fnm env` / `FNM_DIR`; this Mac uses `~/.local/share/fnm`, not Application Support) and each nvm install (`$NVM_DIR/versions/node`) | `fnm uninstall <v>`; nvm: shell function, run as `bash -c 'source "$NVM_DIR/nvm.sh" && nvm uninstall <v>'` pinned to the real nvm dir |
| Node global packages | per Node version, the packages in its `lib/node_modules` (npm); per pnpm major, `pnpm root -g` | `npm uninstall -g <name>` / `pnpm remove -g <name>`, matching Node first on `PATH`, same pinning rule as the tool commands in `catalog/tools.rs` |
| Package managers (corepack) | each version in the corepack cache (`~/.cache/node/corepack/...`: pnpm, yarn) | a corepack command if one exists for a single version, else a folder move |
| Python (uv) | each managed Python, deduplicated by real path (the `3.12` and `3.12.13` folders may be one install) | `uv python uninstall <v>` |
| uv tools | each tool from `uv tool list` | `uv tool uninstall <name>` |
| Rust toolchains | each toolchain in `~/.rustup/toolchains` | `rustup toolchain uninstall <name>` |
| Cargo binaries | names and versions from `~/.cargo/.crates2.json`; sizes from `~/.cargo/bin` | `cargo uninstall <name>`; rustup's own proxy binaries (`cargo`, `rustc`, `rustup`...) are never listed |
| Android SDK | each component folder with its `package.xml` (platforms, build-tools, system-images, ndk, emulator) | `sdkmanager --uninstall "<path>"` when `sdkmanager` is found, else a folder move; `cmdline-tools` and `platform-tools` are listed but not markable **(my call)** |
| iOS simulators | each device from `xcrun simctl list -j` and each runtime | `xcrun simctl delete <udid>`; runtimes by `xcrun simctl runtime delete`; never a folder move (the registry would be left wrong) |

Other managers (pipx, gem, bun globals) are a later addition; the section
model must make that a new source, not a rewrite. **(my call)**

Removing a Node version removes its global packages too, and the page counts
them: the version's Frees includes its `lib/node_modules`. Marking a version
makes its package rows read "goes with the version" and drops their own marks,
so nothing runs twice.

## Flags on rows

- **Default / current**: the fnm default, the Node first on `PATH`, the rustup
  default toolchain, a booted simulator. Marking one is allowed only with a
  loud warning in the review; a booted simulator is refused
  (`removable`-style check like Docker's `ItemFacts::removable`).
- **Old** **(my call)**: a Node version older than the default, a uv Python
  that is not the newest of its minor, a non-default Rust toolchain. Shown in
  a dimmer style with the word "old".
- **Pinned by projects** (Node only for now): read `.nvmrc`, `.node-version`,
  `package.json` `engines.node`, `volta.node` from the project roots the normal
  scan and Caches & Projects list already know (no extra walk of the disk).
  Matching is by major (and exact version when given); a range it cannot read
  is shown as "maybe". The details pane lists the projects. It is a note, not a
  block: the user said a pinned version may still be deleted (fnm reinstalls it).
- **Bulk marking** **[User, changed after the first build]**: marking a section or
  group header marks every markable row beneath it (an iOS version selects its
  simulators) and again unmarks them; running simulators and packages of a
  marked Node version are skipped.

## Leftovers

A row is built from what is on disk, so a folder a command did not remove is
still listed after the refresh. A row whose command ran this session and whose
folder is still there changes to a folder move (soft delete), with a note
"left behind by `<command>`".

## Rules inherited from the project

- Items are `ReviewCommand`s (or folder rows) in the shared selection
  (`App::tool_marks`, new id prefixes `node:`, `node-global:`, `corepack:`,
  `uvpy:`, `uvtool:`, `rustup:`, `cargo:`, `android:`, `sim:`), reviewed and run
  by the existing review and `CommandJob`; `y` only, process group, cancel,
  `cwd` = the home folder, no stdin, timeout.
- Discovery and sizes never run on the UI thread: `bg::Latest` for the list,
  `bg::walk_pool()` for folder sizes, a `bg::Work` bar while it works.
- Sizes: logical and on disk (`fmt::both`); clone-aware where the scan covers
  the folder, a plain walk elsewhere. Command removals feed
  `Accounting::record_commands` as estimates.
- All names on screen go through `ui::clean`/`fit`. Paths must be inside the
  home folder; the builder checks `safety.rs` for the fnm, corepack, rustup,
  cargo, Android SDK and CoreSimulator folders and makes sure none is
  protected in a way that breaks the folder-move fallback (command removal
  does not move files, so it is allowed either way).
- The sidebar entry already exists dimmed; it gets its screen and keeps its
  position (the digit follows `SIDEBAR`). README key table, `CLAUDE.md` and the
  Caches design section 11 are updated in the same change.

## Left to the builder

- Exact commands and flags per tool must be verified against current docs
  (context7 or `--help`), not assumed: `corepack` cache commands, `uv python
  uninstall`, `sdkmanager --uninstall`, `simctl runtime delete`, `fnm uninstall`.
- Parsing formats (`fnm list`, `uv tool list`, `.crates2.json`, `package.xml`,
  `simctl list -j`) with fixtures in `tests/fixtures/runtimes/`, run through
  `FakeRunner`; sandboxed `HOME` for anything that runs the binary.
- Layout of the section and group rows on narrow terminals, and how many
  columns the new sections need, following the Caches page.
- Whether the version-to-packages relation is shown as nested rows or as a
  group per version.
- Order of work: Node versions with globals and the pinned flag first (the
  user's main need), then corepack, uv, Rust, cargo, Android, simulators.
