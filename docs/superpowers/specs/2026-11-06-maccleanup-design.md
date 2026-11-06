# MacCleanup: Design

Status: approved 2026-11-06. Milestone 1 (explorer, safety layer, soft delete, graphs, accounting, finish screen) is implemented; the other feature areas are planned. Date: 2026-11-06.
Convention: **[User]** = decided by the user. **[My call]** = decided by the author, open to change.

## 1. Intent and why

The user opens MacCleanup regularly, and especially when disk is low (their Mac is currently 94% full, 28 GB free of 466 GB). They want to see where the space went, free it safely, and finish on a celebratory message showing how much was actually freed. The product is a beautiful, animated, responsive, mouse-capable terminal app (Rust TUI) for macOS.

Success looks like: free space visibly increases, the user trusts it never deleted anything important, and nothing ever looked frozen.

## 2. Goals

- G1. A disk explorer that scans in parallel (default root `~`), shows logical size and real on-disk size per entry, supports vim-style navigation and mouse, and lets the user mark items and delete through a review step. **[User]**
- G2. Soft delete from day one: "delete" moves items to a holding folder (default `~/ToBeDeleted`). The user can see its size, recover items, or empty it for good. **[User]** Default location is **[My call]** only in that it follows the user's own example.
- G3. Feature set, all in scope **[User]**:
  1. Disk explorer (built first, with soft delete built in). **[User]**
  2. App leftovers: remnants of deleted apps, grouped by app, deletable per app or per file/folder.
  3. App uninstaller: all installed apps with total size and breakdown (app, data, caches, etc.), delete whole app.
  4. Caches: grouped by app/tool/type. Must include go, rust, npm, pnpm, yarn, flutter/pub, docker, node (fnm and other managers), python/pip/uv, brew, Xcode, Playwright and others found at scan time.
  5. Project artifacts: `node_modules`, `.next`, `.turbo`, Rust `target`, Xcode `DerivedData` and build output, and by my addition `.dart_tool`, `Pods`, Flutter `build`. **[User]** named the first five; the rest are **[My call]**. Final binaries and app bundles are kept unless the user opts in to deleting them. **[User]**
  6. Docker panel: list images, volumes, containers, build cache by name; show relations (which container uses which image/volume); browse inside volumes; delete a single item; run Docker's own reclaim and report the result. **[User]**
  7. Other cleanup candidates: Time Machine local snapshots, Xcode simulators (including unavailable ones) and runtimes, Android emulator images, large/old files, duplicates, Downloads and installers (`.dmg`, `.pkg`), old iOS backups, orphaned `Application Support` and `Containers` entries. **[User]** accepted all as in scope.
- G4. Two graphs, each togglable, resizable and hideable: a bar/distribution graph and a treemap (box area equals size). Both highlight and follow the selection. Treemap zooms into and out of boxes. **[User]**
- G5. Honest accounting. Any cleanup done through a tool (docker, brew, pnpm, simctl, etc.) is wrapped in a before/after scan, and the report shows what actually changed, plus the tool's own reported number where it prints one (for example `brew cleanup`). **[User]**
- G6. Finish screen: shows total truly freed with an animated count-up and a congratulations message. Free space is visible throughout. **[User]** Animated count-up is **[My call]**.

## 3. Non-goals and anti-goals

- Not a general file manager, backup tool, or malware cleaner.
- Not cross-platform: macOS only. **[My call]**
- Anti-goal: deleting anything important or breaking the system or apps, even if the feature "works". **[User]**
- Anti-goal: a screen that looks frozen, static, or delayed while work happens. **[User]**
- Anti-goal: counting soft-deleted data as freed space. **[My call, agreed]**
- Out of scope unless asked: user documents and media libraries (Photos library, sparsebundles) are shown in the explorer but never suggested for cleanup.

## 4. Experience requirements

### Layout **[User chose B]**
Persistent left sidebar (Home, Explorer, Leftovers, Uninstall, Caches, Docker, Projects, ToBeDeleted, plus other sections as built) with a "pending" figure (soft-deleted, not yet freed) under it. Right side is the active pane. The Home overview pane shows free space with a live bar and a list of suggested biggest wins, with a "clean suggested" action.

### Visual style **[User]**
Rich color, icons, special Unicode and Nerd Font glyphs (assume Nerd Font installed), elegant and "alive". A command-line flag switches to ASCII-only for terminals without that support. Animations using characters, motion and morph/transitions are wanted: screen transitions, treemap zoom, scan activity, final count-up. **[My call]**: an option to reduce or disable animations; animation never delays input or the real work.

### Color semantics **[My call]**
Green = safe, amber = caution (rebuildable or needs care), red = irreversible or protected. Colors carry safety meaning, not only decoration.

### Responsiveness **[User]**
Every long operation (scan, sizing, delete, tool command) shows live activity: progress bar when the total is known, otherwise counters that keep rising (files seen, bytes found), with spinners. **[My call]**: the UI thread never blocks; scanning and cleanup run in background workers; partial results are browsable while scanning continues.

### Scan speed **[User, added 2026-11-06]**
All scans are multi-threaded and run at maximum speed: a worker pool sized above the core count (directory walking is I/O-bound), one job per directory, and no shared lock on the per-file hot path. This applies to every scanning feature, not only the explorer.

### Input **[User]**
Vim-style navigation (`j/k/h/l`, `space` to mark, `d` to review and delete) plus mouse support. Key map shown in a footer and help screen. **[My call]** extra keys: `/` filter, `s` sort, `g` cycle graph, `[`/`]` resize graph, `?` help.

### Disk explorer screen **[User approved mockup]**
Rows show icon, name, logical size, on-disk size, share bar. Folders still scanning show spinner and live count. Header shows overall scan progress. Footer shows "marked: N items, X GB (not deleted yet)".

## 5. Safety model

- **Review before delete [User]:** pressing `d` opens a review listing every item with size and safety color. Nothing is touched before confirmation.
- **Default action is soft delete [User].** Permanent delete is a separate, harder choice with an explicit irreversible warning. **[User]** asked for warnings on irreversible actions.
- **Protected set [My call]:** system paths and anything apps/OS need to run cannot be marked at all. The builder defines the exact list (for example `/System`, `/usr`, `/bin`, `/Library` system parts, `~/Library/Keychains`, active app bundles in use, SIP-protected locations, the user's own documents and media libraries). The design requires a conservative allow-list approach for suggestions: only known-safe categories are ever offered as one-click cleanup.
- **Caution class [My call]:** rebuildable data (caches, `node_modules`, `target`) is marked caution or safe with a note on how it regenerates.
- **Tool-driven cleanup:** where a tool owns the data (docker, pnpm, brew, simctl, Xcode, rustup, etc.), use the tool's own command rather than deleting files directly. **[User]**
- **Project artifacts:** keep final binaries and app bundles unless the user opts in. **[User]**
- **Running state [My call]:** do not delete things in use (for example volumes mounted by a running container, the active toolchain version).

## 6. Soft-delete folder behavior **[User], details [My call]**

- Default `~/ToBeDeleted`, configurable.
- The ToBeDeleted section shows total size, lists items with original location and date, and supports: recover (restore to original location, with conflict handling), delete selected for good, empty all for good, each with confirmation.
- Moving within the same volume frees no space until emptied; the UI shows this as "pending" separately from "freed". The finish screen and free-space figure count only truly freed space.
- Items from tool-driven cleanups (docker, brew, etc.) are not soft-deletable because the tool removes them; the UI labels them irreversible before running.
- Original-path metadata is stored so recovery works.

## 7. Accounting **[User]**

Before running any cleanup, snapshot the relevant measure (affected paths and overall free space). After, rescan and report: exact change, plus the tool's own number when available. The finish screen aggregates these across the session.

## 8. Constraints

- Language: Rust. **[User, from README]**
- Runs in a macOS terminal; Nerd Font assumed; ASCII fallback flag. **[User]**
- macOS permissions: scanning protected locations may need Full Disk Access. **[My call]** The app detects missing access and tells the user clearly, with instructions, rather than silently under-reporting.
- User global rules apply to the build: format Rust code with `rustfmt`, use package-manager commands (`cargo add`) rather than hand-editing manifests, and never commit without the permissions recorded in the project CLAUDE.md.

## 9. Left to the builder

- Library and architecture choices **[User handed these over]**. **[My call]** defaults: ratatui plus crossterm for the TUI, a worker-thread scanner (for example with `rayon` or `ignore`-style walkers) feeding the UI through channels, `walkdir`-style traversal that uses on-disk block counts for real size and handles hard links and sparse files. All library versions and APIs must be verified against current docs before use.
- Exact protected-path list and per-category safety classification.
- Docker integration approach (Docker CLI versus API), and how to browse inside volumes.
- Config file format and location, ToBeDeleted metadata format.
- Animation engine details.
- Build order after the explorer and soft delete: **[My call]** explorer plus soft delete first, then caches and project artifacts, then Docker, then leftovers and uninstaller, then the remaining candidates.
- Testing approach, with emphasis on safety logic (protected set, review/confirm flow, soft-delete recovery) being heavily tested and never run against real user data.

## 10. Decisions after the builder check

**[User]**
- **Soft delete and "clean suggested":** soft delete is the default everywhere, including the Home "clean suggested" action. The review screen also offers a clearly marked irreversible "move, then empty now" option in one step.
- **Admin rights:** the app never asks for a password and never escalates. For anything needing admin rights (for example `tmutil` snapshot deletion), it shows the exact command with a copy-to-clipboard button and two buttons, "Cancel" and "I ran the command". The user runs the command in another terminal, then confirms in the app, which does the after-scan and continues.
- **Docs upkeep:** `README.md` is a proper introduction (what it is, how to run and build); `CLAUDE.md` and `README.md` must always be kept updated with instructions and rules.

### Calls made after the builder check (all **[My call]**, open to change)

- **Planning scope:** plan milestone 1 (scanner, explorer, safety layer, soft delete, finish screen, free-space accounting) in full detail. Outline later milestones only, and re-plan each after the previous one ships, since they depend on how the scanner and safety layer turn out.
- **Safety first:** the protected-path deny-list and category classification are a separate, hard-coded module, written and tested before any delete code exists. The user reviews the list in the plan before delete runs on real data.
- **"Truly freed" measure:** the headline is the volume's free space before versus after (`statfs`), with "available including purgeable" shown as a secondary figure and per-path deltas as detail. If free space did not rise as expected (for example because Time Machine local snapshots or APFS clones still hold the data), the finish screen says so and points to the snapshots feature instead of claiming a win.
- **Full Disk Access:** run degraded with a visible banner when access is missing, listing what could not be scanned and how to grant access to the terminal app, and never suggest deleting what could not be seen. Do not block startup.
- **Distribution:** a personal tool installed with `cargo install` or run with `cargo run`; no signing or packaging. Target terminals: Ghostty, iTerm2, Terminal.app (the user has the first two installed). Detect true-color and glyph support and fall back gracefully.
- **Testing:** all filesystem tests run in temporary sandbox directories. Docker, brew, simctl and other tool integrations sit behind a command-runner abstraction and are tested with mocks; real runs are done manually by the user.
- **Docker:** target Docker Desktop on macOS. Volume browsing is read-only; if it needs a short-lived helper container, the app asks before starting one and mounts the volume read-only.

## 11. Reference: what exists on the user's Mac today (read-only survey, 2026-11-06)

Used only to prioritize; not a contract. Docker 31 GB (about 4.8 GB images, 4.3 GB volumes, 2.2 GB build cache reclaimable by Docker's own count), Xcode simulators 10.6 GB with 358 unavailable entries, `~/Library/Caches` 14.9 GB, `~/.cache` 6 GB, package-manager caches about 10 GB, `~/.android` 4.3 GB, Telegram 4.2 GB, 9 Time Machine local snapshots, brew cache 0.45 GB. Large items that are not cleanup targets: Photos library (32 GB), `Application Support/Claude` (13.6 GB), Autodesk (12 GB), two vault images in Pictures. Tools present: docker, brew, fnm, pnpm, npm, yarn, cargo, rustup, uv, pip, go, flutter, dart, xcodebuild, pod.
