# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MacCleanup is a Rust TUI (colorful, mouse-supported) that finds and reports disk usage on macOS and cleans up extra files. The repo is currently pre-code (README, CLAUDE.md and a design doc only). Add build/test/lint commands here once a `Cargo.toml` exists.

Planned features (see `README.md` for the full list): main menu, multithreaded disk explorer (default scan root `~`, vim-style navigation, real vs on-disk size), leftover-app-remnant finder grouped by app, app uninstaller, cache cleanup, and a togglable size-distribution graph that follows the selection.

## Gotchas

- Deletion is a final step: list every file to be deleted with its size, then delete with a progress bar. Never delete without that review step.
- Caches for docker and pnpm should be cleaned through their own commands where possible, not by removing files directly.
- Multiple npm/pnpm/node installs may exist (e.g. via fnm or other version managers); don't assume a single one.

- Never escalate privileges or ask for a password. For actions needing admin rights, show the exact command with a copy-to-clipboard button plus "Cancel" and "I ran the command" buttons; after the user confirms, rescan and continue.
- Free space is measured by the volume's free space before vs after; soft-deleted data is "pending", never "freed".
- Safety logic (protected paths, review/confirm, soft-delete recovery) must be tested in temp sandbox directories only, never against real user data.

## Design

The approved-in-progress design is `docs/superpowers/specs/2026-11-06-maccleanup-design.md`. Follow it; update it when decisions change.

## Documentation upkeep

Always keep `CLAUDE.md` and `README.md` up to date. Whenever you add or change a command, feature, rule, build step, or convention, update both in the same change. Commands in `README.md` must reflect what actually works.

## Git

- Blanket commit permission is granted for this repo: commit without asking first. This overrides the global "ask before each commit" rule for this project only.
