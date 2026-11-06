# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: milestone 1 is implemented.** The disk explorer, soft delete (ToBeDeleted folder), graphs and free-space accounting work today. The remaining features below are planned; see the design document for the full scope.

## What works today

- **Home:** live free-space gauge, pending (soft-deleted) space, scan activity, and the biggest folders. Every size is shown both as logical size and as space actually used on disk (for example `1.5 KB (4.1 KB on disk)`), because on-disk size is what a delete frees.
- **Disk explorer:** a multithreaded scan of your home folder (or `--root`) with live sizes. Each row shows logical size, real on-disk size and a share bar. Browse with vim-style keys or the mouse, sort by on-disk size, size or name, mark items, then review and delete.
- **Always responsive:** every move and every permanent delete runs in the background with a live progress bar (by bytes, so one huge folder still moves it). Big trees are deleted in parallel. Quitting while a delete runs waits for it to finish.
- **Soft delete:** "delete" moves items to `~/ToBeDeleted` (or `--holding-dir`). Browse that folder, restore items to where they came from, delete items for good, or empty it. Moved files count as **pending**, not freed, because nothing is freed until the folder is emptied.
- **Unique vs shared space:** under the list, the selected folder or file shows what deleting it actually **frees** (every hard link to those files is inside it) and what it **shares** with hard links elsewhere, which stays on disk. This is what to look at for pnpm's `node_modules`, which hard-links into the pnpm store. The review dialog sums it for everything you marked ("frees about X; Y is shared and stays"). It does not depend on which folder the scanner happened to credit first.
- **Rescan:** `r` rescans the folder you are viewing and updates the totals above it; `R` rescans everything from the scan root.
- **Graphs:** a bar graph and a treemap (box area equals size) that follow your selection. Cycle bars, treemap and hidden with `t`; resize the panel with `[` (bigger) and `]` (smaller). Graphs and share bars follow the sorted size (on-disk by default).
- **Motion:** windows grow in, and the congratulations window rains confetti. `--no-animation` turns all of it off.
- **Quit summary:** when you quit, the app prints what the session did (moved, deleted for good, restored, still pending) and how much space was truly freed.
- **Honest accounting:** the finish screen reports space truly freed (the volume's free space now minus at the start), not just what was moved.
- **Safety:** a hard-coded protected list, a review screen before every delete, and an extra explicit confirmation for anything irreversible (see below).

## Planned

App leftovers, app uninstaller, caches (npm, pnpm, cargo, Xcode, Playwright and more, cleaned through each tool's own command), project artifacts (`node_modules`, `.next`, `.turbo`, Rust `target`, Xcode `DerivedData`), a Docker panel, Time Machine snapshots, simulators, large and duplicate files, and the copy-this-command flow for actions that need admin rights. The sidebar already lists these, dimmed.

## Build and run

Requires macOS and a recent Rust toolchain (developed with Rust 1.99, edition 2024).

```bash
cargo run --release                       # run the app (scans your home folder)
cargo run --release -- --root ~/Developer # explore a different folder
cargo run --release -- --help             # all options
```

| Option | What it does |
|---|---|
| `--root <DIR>` | Folder to explore (default: your home folder) |
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
| `space` | Mark or unmark the selected item |
| `d` | Review marked items, then delete |
| `t` | Cycle graph type: bars, treemap, hidden |
| `[` / `]` | Grow / shrink the graph panel |
| `r` / `R` | Rescan the folder you are viewing / rescan everything (refused while a scan is running; on the ToBeDeleted screen `r` is restore) |
| `s` | Cycle sort: on-disk size (default), size, name. The sorted column is white with an arrow in the header |
| `Tab` | Switch focus between the menu and the list (the menu has focus at startup: `j`/`k` choose a screen, `Enter` opens it) |
| `1` `2` `3` | Home, Explorer, ToBeDeleted |
| `r` / `D` / `E` (ToBeDeleted screen) | Restore / delete for good / empty all |
| `q`, `Esc` | Quit (`Esc` goes up a folder first) |
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
