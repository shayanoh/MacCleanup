# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MacCleanup is a Rust TUI (colorful, mouse-supported) that finds and reports disk usage on macOS and cleans up extra files. The repo is currently pre-code (README only). Add build/test/lint commands here once a `Cargo.toml` exists.

Planned features (see `README.md` for the full list): main menu, multithreaded disk explorer (default scan root `~`, vim-style navigation, real vs on-disk size), leftover-app-remnant finder grouped by app, app uninstaller, cache cleanup, and a togglable size-distribution graph that follows the selection.

## Gotchas

- Deletion is a final step: list every file to be deleted with its size, then delete with a progress bar. Never delete without that review step.
- Caches for docker and pnpm should be cleaned through their own commands where possible, not by removing files directly.
- Multiple npm/pnpm/node installs may exist (e.g. via fnm or other version managers); don't assume a single one.

## Git

- Blanket commit permission is granted for this repo: commit without asking first. This overrides the global "ask before each commit" rule for this project only.
