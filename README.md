# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: milestone 1 is implemented.** The disk explorer, soft delete (ToBeDeleted folder), graphs and free-space accounting work today. The remaining features below are planned; see the design document for the full scope.

## What works today

- **Home:** live free-space gauge, pending (soft-deleted) space, scan activity, and the biggest folders. Every size is shown both as logical size and as space actually used on disk (for example `1.5 KB (4.1 KB on disk)`), because on-disk size is what a delete frees.
- **Disk explorer:** a multithreaded scan of your home folder (or the folder you pass, e.g. `maccleanup .`) with live sizes. Each row shows logical size, real on-disk size and a share bar. Browse with vim-style keys or the mouse, sort by on-disk size, what a delete frees, size or name, mark items, then review and delete.
- **Filter:** `/` opens a filter box above the list and typing narrows the current folder's list as you type, fzf-style: space-separated terms must all match, each term is a fuzzy match that ignores case unless you type a capital, and `'exact`, `^prefix`, `suffix$` and `!exclude` work too. Matched letters are highlighted, the sort order is kept, and the box shows how many rows match. `Enter` returns to the list with the filter kept, `/` edits it again, `Esc` clears it. Going into or out of a folder clears it.
- **ToBeDeleted indicator and quit warning:** the menu shows when ToBeDeleted is empty and, when it holds something, lights its icon and shows how many items are waiting. Quitting with items still in it asks first (`e` opens it, `q` quits anyway, `Esc` stays).
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

## Planned

A Time Machine snapshots screen (list them with sizes, thin or delete them) is the next phase; for now the app only tells you how much they hold.

App leftovers, app uninstaller, caches (npm, pnpm, cargo, Xcode, Playwright and more, cleaned through each tool's own command), project artifacts (`node_modules`, `.next`, `.turbo`, Rust `target`, Xcode `DerivedData`), a Docker panel, Time Machine snapshots, simulators, large and duplicate files, and the copy-this-command flow for actions that need admin rights. The sidebar already lists these, dimmed.

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
| `g` / `G` (or `Home` / `End`), `PageUp` / `PageDown` | Jump to top / bottom, page |
| `/` | Filter the current folder's list (`Enter` keeps it, `/` edits it, `Esc` clears it) |
| `space` | Mark or unmark the selected item |
| `d` | Review marked items, then delete |
| `t` | Cycle graph type: treemap, hidden, bars |
| `[` / `]` | Grow / shrink the graph panel |
| `r` / `R` | Rescan the folder you are viewing / rescan everything (refused while a scan is running; on the ToBeDeleted screen `r` is restore) |
| `s` | Cycle sort: on-disk size (default), frees, size, name. The sorted column is white with an arrow in the header |
| `Tab` | Switch focus between the menu and the list (the menu has focus at startup: `j`/`k` choose a screen, `Enter` opens it) |
| `1` `2` `3` | Home, Explorer, ToBeDeleted |
| `r` / `D` / `E` (ToBeDeleted screen) | Restore / delete for good / empty all |
| `q`, `Esc` | Quit (`Esc` first clears an active filter, then goes up a folder; quitting asks first if ToBeDeleted still holds items) |
| mouse | Click rows and menu entries, scroll with the wheel |

In the review dialog: `Enter` moves items to ToBeDeleted, `x` offers "move, then empty now" (irreversible), `Esc` cancels. Irreversible actions need an explicit `y`; `Enter` never confirms them.

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
- Milestone 1 plan: [`docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md`](docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md)
- Contributor and AI-assistant guidance: [`CLAUDE.md`](CLAUDE.md)
