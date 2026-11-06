# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: milestone 1 is implemented.** The disk explorer, soft delete (ToBeDeleted folder), graphs and free-space accounting work today. The remaining features below are planned; see the design document for the full scope.

## What works today

- **Home:** live free-space gauge, pending (soft-deleted) space, scan activity, and the biggest folders.
- **Disk explorer:** a multithreaded scan of your home folder (or `--root`) with live sizes. Each row shows logical size, real on-disk size and a share bar. Browse with vim-style keys or the mouse, sort by size or name, mark items, then review and delete.
- **Soft delete:** "delete" moves items to `~/ToBeDeleted` (or `--holding-dir`). Browse that folder, restore items to where they came from, delete items for good, or empty it. Moved files count as **pending**, not freed, because nothing is freed until the folder is emptied.
- **Graphs:** a bar graph and a treemap (box area equals size) that follow your selection. Cycle bars, treemap and hidden with `g`; resize the panel with `[` and `]`.
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
| `Home` / `End` or `G`, `PageUp` / `PageDown` | Jump and page |
| `space` | Mark or unmark the selected item |
| `d` | Review marked items, then delete |
| `g` | Cycle graph: bars, treemap, hidden |
| `[` / `]` | Shrink / grow the graph panel |
| `s` | Sort by size or name |
| `Tab` | Focus the sidebar (then `j`/`k` switch screens) |
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
