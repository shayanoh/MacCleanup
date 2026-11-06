# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: milestones 1 and 2 are implemented.** The disk explorer, soft delete (ToBeDeleted folder), graphs, free-space accounting, and the Caches & Projects screen work today. The remaining features below are planned; see the design document for the full scope.

## What works today

- **Home:** live free-space gauge, pending (soft-deleted) space, scan activity, and the biggest folders. Every size is shown both as logical size and as space actually used on disk (for example `1.5 KB (4.1 KB on disk)`), because on-disk size is what a delete frees.
- **Disk explorer:** a multithreaded scan of your home folder (or the folder you pass, e.g. `maccleanup .`) with live sizes. Each row shows logical size, real on-disk size and a share bar. Browse with vim-style keys or the mouse, sort by on-disk size, what a delete frees, size or name, mark items, then review and delete.
- **Filter:** `/` opens a filter box above the list and typing narrows the current folder's list as you type, fzf-style: space-separated terms must all match, each term is a fuzzy match that ignores case unless you type a capital, and `'exact`, `^prefix`, `suffix$` and `!exclude` work too. Matched letters are highlighted, the sort order is kept, and the box shows how many rows match. `Enter` returns to the list with the filter kept, `/` edits it again, `Esc` clears it. Going into or out of a folder clears it.
- **ToBeDeleted indicator and quit warning:** the menu shows when ToBeDeleted is empty and, when it holds something, lights its icon and shows how many items are waiting. Quitting with items still in it asks first (`e` opens it, `q` quits anyway, `Esc` stays).
- **Progress you can see:** the scan shows a moving bar with its rising counters; the tool lookup and the Caches & Projects list build show what they are doing, and the build shows a filled bar by stage (for example `list: reading space numbers 2/3`).
- **Always responsive:** every move and every permanent delete runs in the background with a live progress bar (by bytes, so one huge folder still moves it). Big trees are deleted in parallel. Quitting while a delete runs waits for it to finish.
- **Soft delete:** "delete" moves items to `~/ToBeDeleted` (or `--holding-dir`). Browse that folder, restore items to where they came from, delete items for good, or empty it. Moved files count as **pending**, not freed, because nothing is freed until the folder is emptied.
- **Where the space would go:** under the list, the selected folder or file shows what deleting it would really do, because on a Mac sizes lie. APFS clones (what pnpm uses for `node_modules`) look full-size but cost nothing extra, and Time Machine local snapshots keep deleted files' blocks alive. Three lines: **frees now** and **held by snapshots** (freed only when those hourly snapshots expire), **shared with other places** (clones or hard links with files outside the folder, which stay) and **shared inside**, and what **cloning inside** already saves. A folder's **On disk** is its real footprint (clone data and hard links counted once), so the sort order and graphs use real space. The review dialog sums it for everything you marked. The numbers are final once the scan has finished; the scan is about 2.5 times slower for this (one extra system call per file).
- **Folder totals and position:** a line above the scan totals sums the folder you are viewing (items, size, on disk, what deleting it frees); the bottom-right of the list shows your position (`12/340`) and a scrollbar appears when the list is longer than the pane. A **Frees** column shows what deleting each row really frees (a clone or hard link frees nothing); it is `…` until the scan finishes, and the Share bar is dropped first when the pane is narrow. Folders still waiting for their turn in the scan show `…` and `queued` instead of a misleading 0 B.
- **Snapshots, exactly:** a file's shortfall is blamed on a Time Machine snapshot only if a snapshot exists, the file already existed when the newest one was taken, and the missing bytes lie inside the file's real data. Compressed files and blocks preallocated past the end of a file are never counted as snapshot-held.
- **Rescan:** `r` rescans the folder you are viewing and updates the totals above it; `R` rescans everything from the scan root.
- **Graphs:** a treemap (box area equals size, the default) and a bar graph that follow your selection. Cycle treemap, hidden and bars with `t`; resize the panel with `[` (bigger) and `]` (smaller). Graphs and share bars follow the sorted size (on-disk by default).
- **Motion:** windows grow in, and the congratulations window rains confetti. `--no-animation` turns all of it off.
- **Quit summary:** when you quit, the app prints what the session did (moved, deleted for good, restored, still pending) and how much space was truly freed.
- **Honest accounting:** the finish screen reports space truly freed (the volume's free space now minus at the start), not just what was moved.
- **Safety:** a hard-coded protected list, a review screen before every delete, and an extra explicit confirmation for anything irreversible (see below).

- **Caches & Projects** (`5`): one list of things that come back by themselves. A blue **PROJECTS** section groups build output by project (`node_modules`, `.next`, `.turbo`, Rust `target`, `.dart_tool`, Flutter and Xcode `build`, `Pods`; only when the project's `package.json`, `Cargo.toml`, `pubspec.yaml`, `Podfile` or `.xcodeproj` sits next to it) and a purple **TOOLS & CACHES** section groups tool caches (pnpm, npm, Yarn, Go, uv, pip, Homebrew, Cargo, Dart/Flutter pub, Xcode DerivedData, Playwright, and every other folder in `~/Library/Caches` and `~/.cache`). It has the Explorer's columns, sort, filter, graph and keys. Because pnpm clones files into every project, one row's size lies: the footer shows **what everything you marked really frees, as a set**, recomputed in the background on every mark. The details pane names the other folders that hold the same data (with their mark state) and `m` marks them all, so you can keep one project's `node_modules` and clear the rest. Marks are shared with the Explorer: mark in either place, and `d` reviews everything.
- **Products are kept:** build folders hold intermediate files and the things you actually want (a built program, an APK, a bundle). The list splits them: Rust `target` becomes `debug/deps`, `release/incremental` and so on, with each built program and Tauri `bundle` as its own **(kept)** row; Xcode `build` and DerivedData keep `Build/Products`; Flutter keeps `build/app/outputs` and the web build; Gradle keeps `outputs` and `libs`; Maven keeps the packaged `.jar`/`.war`; SwiftPM keeps executables; a Python `dist` folder is one kept row. Marking a project or a section marks the intermediate folders only; a kept row is deleted only when you mark that row yourself. A build folder the app does not recognise is still shown as one row.
- **Projects and workspaces:** a monorepo's packages are grouped under its root project, found the way the node tools find it (`pnpm-workspace.yaml`, `"workspaces"` in `package.json`, lerna, nx, Cargo `[workspace]`, ...), so `apps/web/node_modules` and a Tauri `src-tauri/target` appear in one group named after the project, with root-relative names. SDKs and toolchains (a Flutter SDK, a Node or Rust install) are recognised by their layout and never listed, wherever they are installed.
- **Stuck or slow commands:** while a tool command runs, the Working window shows its live output, how long it has run, and `Esc` stops it (it kills the command and anything it started, and skips the commands after it; folder moves already made stay). `q` or `Ctrl-C` stop it and quit. `uv cache clean` waits up to 5 minutes when another uv process holds the cache (a `uv run` server, for example), then fails; the window and the result screen say so in plain words. For commands that have a force form (`uv cache clean --force`), the review has an `f` key to switch to it (drawn in red, with a warning that it skips uv's in-use check), and after a lock failure the result screen offers `f` to retry with `--force` through the review, which still needs `y`.
- **Tool commands:** caches that have their own clean command (pnpm, npm, Yarn, Go, uv, pip, Homebrew) are cleaned by that command, listed in the review with the exact install it runs. Each pnpm store version (`store/v10`, `store/v11`) is pruned by the pnpm that owns it; installs under fnm, nvm and volta, including corepack shims, are found, and commands run in your home folder with downloads and prompts disabled, so they cannot hang. A review that runs commands needs an explicit `y` (never `Enter`), and a failing command does not stop the others or undo the moves.

## Planned

A Time Machine snapshots screen (list them with sizes, thin or delete them) is the next phase; for now the app only tells you how much they hold.

App leftovers, app uninstaller, a Docker panel, Time Machine snapshots, simulators, large and duplicate files, and the copy-this-command flow for actions that need admin rights. The sidebar already lists these, dimmed.

## Build and run

Requires macOS and a recent Rust toolchain (developed with Rust 1.99, edition 2024).

```bash
cargo run --release                       # run the app (scans your home folder)
cargo run --release -- ~/Developer        # explore a different folder (any path is the root)
cargo run --release -- --help             # all options
```

| Option | What it does |
|---|---|
| `[PATH]` | Folder to explore, e.g. `.` (default: your home folder); the old `--root DIR` still works |
| `--holding-dir <DIR>` | Where soft-deleted items go (default: `~/ToBeDeleted`) |
| `--ascii` | ASCII-only characters, for terminals without Nerd Font / Unicode support |
| `--no-animation` | Turn off animations |

The look assumes a [Nerd Font](https://www.nerdfonts.com/) in your terminal. Anything outside your home folder is protected in this release, so deletions only work inside it.

**Full Disk Access:** scanning some folders needs Full Disk Access for your terminal app (System Settings > Privacy & Security > Full Disk Access). The app tells you when it is missing and keeps working with what it can read.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, arrows | Move down / up |
| `h` / `l`, `Enter` | Go out of / into a folder |
| `g` / `G` (or `Home` / `End`), `PageUp` / `PageDown`, `Ctrl-F` / `Ctrl-B` (also `Ctrl-D` / `Ctrl-U`) | Jump to top / bottom, page down / up |
| `/` | Filter the current folder's list (`Enter` keeps it, `/` edits it, `Esc` clears it) |
| `space` | Mark or unmark the selected item |
| `d` | Review marked items, then delete |
| `t` | Cycle graph type: treemap, hidden, bars |
| `[` / `]` | Grow / shrink the graph panel |
| `r` / `R` | Rescan the folder you are viewing / rescan everything (refused while a scan is running; on the ToBeDeleted screen `r` is restore) |
| `s` | Cycle sort in header order: name, size, on-disk size (the default), frees. The sorted column is white with an arrow in the header |
| `Tab` | Switch focus between the menu and the list (the menu has focus at startup: `j`/`k` choose a screen, `Enter` opens it) |
| `1` to `7` | Open the page at that position in the menu (the number is shown beside each entry; planned pages say they are coming) |
| `h` / `l` (Caches & Projects) | Fold / open the group; `l` or `Enter` on a row shows it in the Explorer |
| `m` (Caches & Projects) | Mark the selected folder and every folder that shares its data |
| `r` / `D` / `E` (ToBeDeleted screen) | Restore / delete for good / empty all |
| `q`, `Esc` | Quit (`Esc` first clears an active filter, then goes up a folder; quitting asks first if ToBeDeleted still holds items) |
| mouse | Click rows, menu entries, and treemap boxes or bars (a click on the graph selects that row in the list); scroll with the wheel |

In the review dialog the item list scrolls when it is longer than the window: `j`/`k`, the arrows, `PageUp`/`PageDown`, `Ctrl-F`/`Ctrl-B`, `Home`/`End` or the mouse wheel (a line under the list says which items you are looking at, e.g. `11-20 of 57`). Tool commands are never scrolled away: they keep their own space. `Enter` moves items to ToBeDeleted, `x` offers "move, then empty now" (irreversible), `Esc` cancels. Irreversible actions need an explicit `y`; `Enter` never confirms them. When the review contains tool commands, only `y` runs it.

## Safety

- Nothing is deleted without a review screen and a confirmation. The default action is always the reversible one.
- System paths, your top-level user folders, keychains, mail, secrets (`.ssh`, `.gnupg`, `.aws`), Photos libraries and disk images, and the ToBeDeleted folder itself are protected and cannot be marked.
- A folder that contains a protected item (for example `~/.config` holding `gcloud`) cannot be moved or deleted as a whole; the app reports which protected item blocked it.
- Quitting while a delete is running waits for it to finish, and a too-small terminal accepts only `q` / Ctrl-C, so a stray key never acts on a screen you cannot see.
- The app never asks for your password. (Admin-only actions are planned: it will show the exact command with a copy button, you run it in another terminal, and then confirm in the app.)

## Development

```bash
cargo fmt                                   # format
cargo clippy --all-targets -- -D warnings   # lint
cargo test                                  # unit and integration tests
cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup   # end-to-end run in a pseudo-terminal
```

The smoke script drives the real binary in a throwaway `HOME` (mark, review, soft delete, restore, irreversible path, quit); set `DUMP=1` to print the screens it sees.

## Documentation

- Design: [`docs/superpowers/specs/2026-11-06-maccleanup-design.md`](docs/superpowers/specs/2026-11-06-maccleanup-design.md)
- Caches & Projects design: [`docs/superpowers/specs/2026-11-06-caches-projects-design.md`](docs/superpowers/specs/2026-11-06-caches-projects-design.md) and plan: [`docs/superpowers/plans/2026-11-06-caches-projects.md`](docs/superpowers/plans/2026-11-06-caches-projects.md)
- Milestone 1 plan: [`docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md`](docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md)
- Contributor and AI-assistant guidance: [`CLAUDE.md`](CLAUDE.md)
