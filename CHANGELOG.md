# Changelog

All notable changes to MacCleanup. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow [Semantic Versioning](https://semver.org/). Add an entry under **Unreleased** with every user-visible change; `cargo release` turns it into the next version's section (see `docs/development.md`).

## [Unreleased]

## [0.10.0] - 2026-11-10

### Added

- Runtimes: each Rust toolchain's extra targets (for example `wasm32-unknown-unknown`) are listed in a new **RUST TARGETS** section, one row per target with its own size, and can be removed one by one with `rustup target remove`. Marking a toolchain removes it with all its targets.
- Prebuilt binaries for Apple Silicon and Intel on each GitHub release.

### Changed

- The minimum supported Rust version is 1.95.
- The crate refuses to build on systems other than macOS, with a clear message.
- `make install` now runs `cargo install --path .`.

## [0.9.0] - 2026-11-10

First public release.

### Added

- Disk explorer: a multithreaded scan with logical and on-disk sizes, what a delete really frees (APFS clones, hard links and Time Machine snapshots taken into account), treemap and bar graphs, fzf-style filter, sorting and mouse support.
- Safe deletion: a review screen before every delete, soft delete to `~/ToBeDeleted` with restore, and honest free-space accounting with a summary on exit.
- Caches & Projects: project build output and tool caches (pnpm, npm, Yarn, Go, uv, pip, Homebrew, Cargo, Xcode and more), with built products kept and tool-owned cleanup commands.
- Docker: images, containers, volumes and build cache with exact sizes, a read-only file browser and a VM disk trim.
- Time Machine: local snapshots with estimated sizes, and deletion.
- Runtimes: Node versions (fnm, nvm) and their global packages, pnpm globals, Corepack, uv Pythons and tools, Rust toolchains, cargo binaries, Android SDK components and iOS simulators.
- Uninstall and Leftovers: installed apps with the folders they leave behind, and leftovers of apps that are already gone.
