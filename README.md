# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: design phase.** The design is written and awaiting approval; no application code exists yet. The "Build and run" section below describes the intended workflow and will be updated as soon as there is something to run.

## What it will do

- **Disk explorer:** multithreaded scan of a folder (default `~`) showing each file and folder's logical size and real on-disk size. Browse with vim-style keys or the mouse, mark items, then review and delete.
- **Soft delete:** "delete" moves items to `~/ToBeDeleted` by default. See how much is in there, recover items, or empty it for good. Moved files are shown as "pending", not as freed space, because nothing is freed until the folder is emptied.
- **Caches:** go, rust, npm, pnpm, yarn, flutter/pub, python/pip/uv, brew, Xcode, Playwright and more, grouped by tool. Cleanup goes through each tool's own command where one exists.
- **Project artifacts:** `node_modules`, `.next`, `.turbo`, Rust `target`, Xcode `DerivedData` and build output, and similar. Final binaries and app bundles are kept unless you opt in to deleting them.
- **Docker:** list images, volumes, containers and build cache with their relations, browse inside volumes, delete single items, or run Docker's own reclaim.
- **App leftovers and uninstaller:** find remnants of apps you already removed, or uninstall installed apps with a size breakdown.
- **Other candidates:** Time Machine local snapshots, Xcode simulators, Android emulator images, large or old files, duplicates, installers, old iOS backups.
- **Graphs:** a bar graph and a treemap (box area equals size) that follow your selection. Both can be resized or hidden.
- **Honest accounting:** every tool-driven cleanup is measured with a before/after scan, plus the tool's own reported number where it prints one. The final screen shows what was truly freed.

## Safety

- Nothing is deleted without a review screen and a confirmation.
- System paths and anything apps need to run are protected and cannot be marked.
- Irreversible actions carry a clear warning. The default action is always the reversible one.
- The app never asks for your password. When something needs admin rights, it shows the exact command with a copy button, you run it in another terminal, and then confirm in the app so it can rescan and continue.

## Look and feel

Rich color, icons and animation, assuming a [Nerd Font](https://www.nerdfonts.com/). A command-line option switches to ASCII-only for terminals without glyph support, and animations can be reduced or turned off.

## Build and run

Planned workflow (not yet available):

```bash
cargo run --release           # run the app
cargo run --release -- --help # list options, including the ASCII-only mode
```

macOS only. Scanning protected locations may need Full Disk Access for your terminal app; the app will tell you when it is missing.

## Documentation

- Design: [`docs/superpowers/specs/2026-11-06-maccleanup-design.md`](docs/superpowers/specs/2026-11-06-maccleanup-design.md)
- Contributor and AI-assistant guidance: [`CLAUDE.md`](CLAUDE.md)
