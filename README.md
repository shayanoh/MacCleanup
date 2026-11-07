# MacCleanup

A colorful, animated terminal app (Rust TUI) for macOS that shows where your disk space went and helps you free it safely. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

> **Status: milestones 1 to 4 (explorer, Caches & Projects, Docker, Time Machine) are implemented.** The disk explorer, soft delete (ToBeDeleted folder), graphs, free-space accounting, and the Caches & Projects screen (project build output and tool caches, with products kept and tool commands) work today. The remaining features under *Planned* are not built yet; see the design documents for the full scope.

## What works today

### Explorer, soft delete and accounting

- **Home:** live free-space gauge, pending (soft-deleted) space, scan activity, and the biggest folders. Every size is shown both as logical size and as space actually used on disk (for example `1.5 KB (4.1 KB on disk)`), because on-disk size is what a delete frees.
- **Disk explorer:** a multithreaded scan of your home folder (or the folder you pass, e.g. `maccleanup .`) with live sizes. Each row shows logical size, real on-disk size and a share bar. Browse with vim-style keys or the mouse, sort by on-disk size, what a delete frees, when something inside last changed (**Modified**), size or name, mark items, then review and delete.
- **Filter:** `/` opens a filter box above the list and typing narrows the current folder's list as you type, fzf-style: space-separated terms must all match, each term is a fuzzy match that ignores case unless you type a capital, and `'exact`, `^prefix`, `suffix$` and `!exclude` work too. Matched letters are highlighted, the sort order is kept, and the box shows how many rows match. `Enter` returns to the list with the filter kept, `/` edits it again, `Esc` clears it. Going into or out of a folder clears it.
- **ToBeDeleted indicator and quit warning:** the menu shows when ToBeDeleted is empty and, when it holds something, lights its icon and shows how many items are waiting. Quitting with items still in it asks first (`e` opens it, `q` quits anyway, `Esc` stays).
- **Progress you can see:** the scan shows a moving bar with its rising counters; the tool lookup and the Caches & Projects list build show what they are doing, and the build shows a filled bar by stage (for example `list: reading space numbers 2/3`).
- **Always responsive:** every move and every permanent delete runs in the background with a live progress bar (by bytes, so one huge folder still moves it). Big trees are deleted in parallel. Quitting while a delete runs waits for it to finish.
- **Soft delete:** "delete" moves items to `~/ToBeDeleted` (or `--holding-dir`). Browse that folder, restore items to where they came from, delete items for good, or empty it. Moved files count as **pending**, not freed, because nothing is freed until the folder is emptied.
- **Where the space would go:** under the list, the selected folder or file shows what deleting it would really do, because on a Mac sizes lie. APFS clones (what pnpm uses for `node_modules`) look full-size but cost nothing extra, and Time Machine local snapshots keep deleted files' blocks alive. Three lines: **frees now** and **held by snapshots** (freed only when those hourly snapshots expire), **shared with other places** (clones or hard links with files outside the folder, which stay) and **shared inside**, and what **cloning inside** already saves. A folder's **On disk** is its real footprint (clone data and hard links counted once), so the sort order and graphs use real space. The review dialog sums it for everything you marked. The numbers are final once the scan has finished; the scan is about 2.5 times slower for this (one extra system call per file).
- **Folder totals and position:** a line above the scan totals sums the folder you are viewing (items, size, on disk, what deleting it frees); the bottom-right of the list shows your position (`12/340`) and a scrollbar appears when the list is longer than the pane. A **Frees** column shows what deleting each row really frees (a clone or hard link frees nothing); it is `…` until the scan finishes. A **Modified** column shows how long ago the newest file inside changed (`today`, `3d ago`, `5w ago`, `3mo ago`; `-` for an empty folder), and the Share bar is dropped first when the pane is narrow. Folders still waiting for their turn in the scan show `…` and `queued` instead of a misleading 0 B.
- **Snapshots, exactly:** a file's shortfall is blamed on a Time Machine snapshot only if a snapshot exists, the file already existed when the newest one was taken, and the missing bytes lie inside the file's real data. Compressed files and blocks preallocated past the end of a file are never counted as snapshot-held.
- **Rescan:** `r` rescans the folder you are viewing and updates the totals above it; `R` rescans everything from the scan root.
- **Graphs:** a treemap (box area equals size, the default) and a bar graph that follow your selection. Cycle treemap, hidden and bars with `t`; resize the panel with `[` (bigger) and `]` (smaller). Graphs and share bars follow the sorted size (on-disk by default). A large treemap box shows what is inside the folder as boxes within it, as deep as there is room. Rest the mouse on a box or bar to highlight it and, after a moment, see a tooltip with its full name, sizes and share.
- **Motion:** windows grow in, and the congratulations window rains confetti. `--no-animation` turns all of it off.
- **Quit summary:** when you quit, the app prints what the session did (moved, deleted for good, restored, still pending) and tool commands (Docker, caches), free space at start and now, and how much space was truly freed. The bottom line of every page shows the free space now and at start.
- **Honest accounting:** the finish screen reports space truly freed (the volume's free space now minus at the start), not just what was moved.
- **Safety:** a hard-coded protected list, a review screen before every delete, and an extra explicit confirmation for anything irreversible (see below).

### Caches & Projects

Press `5` (or choose it in the menu). It uses the Explorer's columns, sort, filter, graph, keys and mouse, and shares its marks with the Explorer: mark in either place and `d` reviews everything.

- **What it lists:** one list of things that come back by themselves. A blue **PROJECTS** section groups build output by project (`node_modules`, `.next`, `.turbo`, Rust `target`, `.dart_tool`, Flutter and Xcode `build`, `Pods`; only when the project's `package.json`, `Cargo.toml`, `pubspec.yaml`, `Podfile` or `.xcodeproj` sits next to it) and a purple **TOOLS & CACHES** section groups tool caches (pnpm (store, dlx cache and metadata cache are separate rows; the store row estimates how much of it no project uses), npm, Yarn, Go, uv, pip, Homebrew, Cargo, Dart/Flutter pub, Xcode DerivedData, Playwright, and every other folder in `~/Library/Caches` and `~/.cache`). It has the Explorer's columns, sort, filter, graph and keys. Because pnpm clones files into every project, one row's size lies: the footer shows **what everything you marked really frees, as a set**, recomputed in the background on every mark. The details pane names the other folders that hold the same data (with their mark state) and `m` marks them all, so you can keep one project's `node_modules` and clear the rest. Marks are shared with the Explorer: mark in either place, and `d` reviews everything.
- **Products are kept:** build folders hold intermediate files and the things you actually want (a built program, an APK, a bundle). The list splits them: Rust `target` becomes `debug/deps`, `release/incremental` and so on, with each built program and Tauri `bundle` as its own **(kept)** row; Xcode `build` and DerivedData keep `Build/Products`; Flutter keeps `build/app/outputs` and the web build; Gradle keeps `outputs` and `libs`; Maven keeps the packaged `.jar`/`.war`; SwiftPM keeps executables; a Python `dist` folder is one kept row. Marking a project or a section marks the intermediate folders only; a kept row is deleted only when you mark that row yourself. A build folder the app does not recognise is still shown as one row.
- **Projects and workspaces:** a monorepo's packages are grouped under its root project, found the way the node tools find it (`pnpm-workspace.yaml`, `"workspaces"` in `package.json`, lerna, nx, Cargo `[workspace]`, ...), so `apps/web/node_modules` and a Tauri `src-tauri/target` appear in one group named after the project, with root-relative names. SDKs and toolchains (a Flutter SDK, a Node or Rust install) are recognised by their layout and never listed, wherever they are installed.
- **Filtering projects and tools too:** on Caches & Projects, `/` matches the name of a project or tool as well as the items. Typing `uv` shows the uv group with all its caches, and a project's folder name (say `MacCleanup`) shows that project with every cache in it, even if it was folded; the matching letters of the name are highlighted. Otherwise only the items that match are shown, as before. Several words may be spread over the two: `mac node_modules` is "node_modules in projects matching mac", and `shop !next` is shop without its `.next`. Only the group's *name* is matched, not its path, so `dev` does not pull in every project under `~/dev`.
- **Separate commands, separate items:** when a tool needs more than one command to clean its caches, each command is its own item with its own size, so you count and select them separately. npm is the example: its **package cache** (`npm cache clean --force`) and its **npx cache** (`npm cache npx rm --force`, what `npx` downloaded to run programs, often the bigger one) are two items. pnpm has one item per store version, and Go has a build cache and a module cache. An npm too old for the npx command gets a plain folder item for it.
- **Numbers follow the cleanup:** after a command runs, the folder it cleaned is scanned again, and after a move the moved folders leave the scan, so rows, totals and the graph show what is really left instead of the old sizes.
- **Stuck or slow commands:** while a tool command runs, the Working window shows its live output, how long it has run, and `Esc` stops it (it kills the command and anything it started, and skips the commands after it; folder moves already made stay). `q` or `Ctrl-C` stop it and quit. `uv cache clean` waits up to 5 minutes when another uv process holds the cache (a `uv run` server, for example), then fails; the window and the result screen say so in plain words. For commands that have a force form (`uv cache clean --force`), the review has an `f` key to switch to it (drawn in red, with a warning that it skips uv's in-use check), and after a lock failure the result screen offers `f` to retry with `--force` through the review, which still needs `y`.
- **Tool commands:** caches that have their own clean command (pnpm, npm, Yarn, Go, uv, pip, Homebrew) are cleaned by that command, listed in the review with the exact install it runs. Each pnpm store version (`store/v10`, `store/v11`) is pruned by the pnpm that owns it; installs under fnm, nvm and volta, including corepack shims, are found, and commands run in your home folder with downloads and prompts disabled, so they cannot hang. A review that runs commands needs an explicit `y` (never `Enter`), and a failing command does not stop the others or undo the moves.

### Docker

Press `6` (needs Docker Desktop). One list, drawn like Caches & Projects, of everything Docker holds, with what is connected to what: a **VM DISK** row, then **IMAGES** (an image holds the containers made from it, and a container holds the volumes it mounts), **VOLUMES** nobody uses, and the **BUILD CACHE**. Columns: Size, Frees, Last used, Created (the last two give way on a narrow terminal so names stay readable).

- **Exact numbers, worked out here:** Docker's own "unique size" can be wrong (it can even be negative), so shared and unique sizes come from the layers themselves. **Frees** is what removing just that item frees inside Docker; for several items marked together it is worked out as a set (a layer two images share is freed only when both are marked).
- **Layers and build steps:** open an image to see its layers, each with the command that made it, its size, whether other images share it, and the biggest one. The details pane shows the **whole** command (wrapped; `J` / `K` scroll it when it does not fit).
- **Last used** is Docker's own for build cache. For images and volumes Docker records nothing, so it is worked out from their containers and shown with a `~`; the details pane says so.
- **Look inside:** `b` (or `Enter` on an image or volume with nothing to unfold) lists the files of an image or volume read-only, with sizes (folders count everything inside, like the Explorer) and dates; it works like the Explorer: `s` sorts by name, size or modified, the graph sits at the side (`t`, `[`, `]`), `/` filters, `l` opens a folder, `h` goes up, `Esc` goes back. A volume is mounted read-only (no network) in a tiny helper image of ours (the first time, the app builds it, and Docker pulls the 2.5 MB `busybox:musl` once if it is not there); an image runs only its own shell to list a folder (no network, read-only root), so images without a shell cannot be browsed. Containers cannot be listed without starting them, so they are not browsable.
- **Removing:** marks go into the same selection and review as everything else (`d` on any screen reviews all of it). Docker's own commands do the removal, in the order they need: containers, images, volumes, build cache. Docker removals cannot be undone, so they need `y`, bulk marks (a section) take only dangling images and unused build cache, and an image or volume held by stopped containers needs those containers marked too (`m` marks them); the review says so and refuses to run until they are.
- **Giving the space back to the Mac:** deleting things inside Docker does not shrink `Docker.raw`. After any removal the app runs a trim inside Docker's VM (the `Shrink VM disk` row is marked for you; unmark it to skip it, or mark that row alone any time), and the finish screen shows what `Docker.raw` took on your Mac before and after. The trim runs from a 2.5 MB helper image the app builds itself (`maccleanup/vm-tools`, from `busybox:musl`).
- **Safe by construction:** every Docker command uses the Docker Desktop context only, the app never uses `--force`, and `Docker.raw` and Docker Desktop's data folders are protected from being deleted as files in the Explorer and on Caches & Projects.

### Time Machine

- **What it does:** the Time Machine page lists the local snapshots (`tmutil listlocalsnapshotdates /`), the total the scan says they hold, and the free space now and at start. Snapshots macOS keeps for itself (updates, boot) are listed dimmed and cannot be marked.
- **Size of each snapshot:** macOS prints none without root, so `m` (selected) or `M` (all) estimates it: the app compares the mounted snapshot with the live files (a minute or two on a big disk; a status line with a spinner and moving bar shows which snapshot, the files and bytes found so far and the elapsed time, like the Explorer's scan line). Pressing `m` or `M` while one is running queues the others: they show "queued" and start by themselves when the running one finishes and counts what was deleted or changed since. Files renamed or moved since the snapshot (compilers do this all the time) are recognised by inode and not counted. What another snapshot also holds is not counted (that is Disk Utility's Private Size). It shows a range, from "renames excluded" to "renames counted as deleted", because a renamed file may also have been rewritten. Against Disk Utility: 138.1 MB against 137.9 MB, 805-1017 MB against 1.01 GB, 2.50-2.67 GB against 2.24 GB (read a few minutes earlier, while the snapshot was still growing). It counts file data only, not the file system's own records. It is labelled "about"; the total on top is the scan's exact figure. Files are matched across snapshots by path, inode, size and modification time.
- **Deleting:** the heading bar at the top of the list is the select-all row (space on it marks or unmarks every snapshot), space marks one, `a` marks all, `d` reviews, and one `y` runs `tmutil deletelocalsnapshots <date>` per snapshot, oldest first. No password is needed. It cannot be undone.
- **Free space after a delete:** APFS gives the space back gradually, so for a while the app reads free space several times a second: the bottom line on every page (`free X (start Y)`) and the result screen follow it until it has stopped moving for 30 seconds.

### Runtimes

- **What it does:** the Runtimes page (menu entry 8) lists what is installed and can be removed, with sizes, in sections: Node versions (fnm, nvm) and the global npm packages of each; pnpm global packages (one row per install group, because pnpm removes a whole group when you remove one of its packages); Corepack package manager versions (pnpm, yarn); uv Pythons and uv tools; Rust toolchains (rustup); cargo binaries; Android SDK components (platforms, build-tools, NDK, CMake, sources, system images); iOS simulator devices. The same list, colors, filter, graph and keys as Caches & Projects.
- **Flags:** a Node version is marked "the default version", "used by this shell" (the one fnm gave the shell that started the app), "older than the default" and "asked for by N project(s)" with every project named (read from `.nvmrc`, `.node-version`, `package.json` `engines.node` / `volta.node` of the projects the scan found, by major or exact version). nvm's default is followed through its aliases (`lts/*`, `node`, `20`). An older patch of a uv Python says "older"; the default Rust toolchain and unavailable simulators say so too. Removing the default Node, the shell's Node, a version projects ask for, or the default Rust toolchain is allowed, and the review shows a warning line under that command saying what it may disturb.
- **Removal, always by the tool's own command:** `fnm uninstall`, `nvm uninstall`, `npm uninstall -g` (run by that version's own npm), `pnpm remove -g`, `uv python uninstall`, `uv tool uninstall`, `rustup toolchain uninstall`, `cargo uninstall`, `sdkmanager --uninstall` (the `sdkmanager` your `PATH` finds inside the SDK, so an older one that works with your Java wins over `cmdline-tools/latest`), `xcrun simctl delete`. Corepack has no per-version command, so its versions are folder moves (to ToBeDeleted); an Android component or Node row whose tool was not found is a folder move too, and its note says so. A version's global packages go with it, so marking a version drops their marks. `space` on a section or group header (an iOS version, "Node (fnm)", "Rust (rustup)") marks every row beneath it that can be marked, and again unmarks them: a running simulator is skipped, and the packages of a marked Node version are skipped because they go with it. The Caches & Projects page does not do this for commands. All of it goes through the one review, irreversible, `y` only.
- **Leftovers:** a row whose command ran but whose folder is still there turns into a folder move (its note says "left behind by ..."), so you can finish the job by hand through the same review.
- **Running simulators** are listed but cannot be marked (shut them down first). Simulator runtimes, Go, Deno and JDKs (outside the home folder, protected in this release) and Homebrew (its own page later) are left out.
- **Known limits:** projects are found by the scan's build folders (a project with only an `.nvmrc` and no `node_modules` is not seen); global npm packages of a Node version are listed only when that version has its own `npm`.

### Uninstall and Leftovers

- **Uninstall (menu entry 4):** every app in `/Applications`, `~/Applications` and their subfolders (not macOS's own), each as a group with the folders that belong to it: the app and its wrapper bundles (Fusion's "Service Utility" and "Remove ..." bundles), `Caches`, `Containers`, `Group Containers` (team id ignored), `Application Support`, preferences, saved state, `HTTPStorages`, `WebKit`, launch agents, the matching items in `/Library` (daemons, helper tools), a launcher script's real payload (Autodesk Fusion's `destfolder=` folder under `Application Support`), and the vendor folder. Columns: Name, Size, On disk, Opened (the app's last use from Spotlight, `never` when it has none; `never` sorts as the oldest). A vendor folder that other installed apps use too is shown under each of them and can be marked only when every installed app of that vendor is marked. Space on an app's header marks the app and its folders in your home folder; outside-home parts (daemons, helper tools, `/Library` folders) are marked one by one.
- **Leftovers (menu entry 3):** folders whose name is a bundle id (or `<id>.plist`, `<id>.savedState`, ...) of an app that is not installed anywhere (scan roots, helpers nested inside apps, and everything Spotlight lists), plus launch items whose program is gone. Name-only folders (`Application Support/Some App`), Apple ids and vendors that still have an installed app are never listed (big vendors, Google/Microsoft/Adobe/Mozilla/JetBrains, are matched by full id only). If Spotlight is off the page lists nothing and says why. Every group says why it is listed.
- **Never removable:** Apple's own apps (`com.apple.*`, Safari, Xcode, ...), anything under `/System`, and any app that is running (the terminal MacCleanup runs in is running, so it is covered). The running check is repeated when the review opens.
- **Removing:** the usual review and soft delete. A root-owned item that a normal move is not allowed to touch is tried normally first; what fails goes into one batch behind macOS's own password prompt (`osascript ... with administrator privileges`, one prompt for all). The app never sees the password. The review marks such rows "may ask for your password" and needs `y`. If you cancel the prompt (or it cannot run), a dialog shows the exact `sudo mv` and `chown` commands against a booked ToBeDeleted slot: `c` copies them, `r` ("I ran the commands") rescans, `Esc` gives the slot back.
- **Package-manager apps** (Homebrew casks found in a Caskroom, MacPorts apps) are removed with the manager's own command (`brew uninstall --cask`, `port uninstall`); their other folders stay ordinary moves. `port uninstall` normally needs root and may report a failure.
- **Known limits:** `launchctl bootout` is not run (a moved daemon keeps running until the next logout or reboot); sizes outside home are plain allocated bytes (no clone accounting); Frees is not shown (the column reads `...`); the vendor's own uninstaller is never run.

## Planned

Next are the other candidates: a Homebrew page, simulators and runtimes, and large and duplicate files. The sidebar lists what is not built yet dimmed.

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
| `/` | Filter the list (`Enter` keeps it, `/` edits it, `Esc` clears it). On Caches & Projects it also matches project and tool names |
| `space` | Mark or unmark the selected item |
| `d` | Review marked items, then delete |
| `t` | Cycle graph type: treemap, hidden, bars |
| `[` / `]` | Grow / shrink the graph panel |
| `r` / `R` | Rescan the folder you are viewing / rescan everything (refused while a scan is running; on the ToBeDeleted screen `r` is restore) |
| `s` | Cycle sort in header order: name, size, on-disk size (the default), frees, modified. The sorted column is white with an arrow in the header |
| `Tab` | Switch focus between the menu and the list (the menu has focus at startup: `j`/`k` choose a screen, `Enter` opens it) |
| `1` to `9` | Open the page at that position in the menu (the number is shown beside each entry; a planned page says it is coming) |
| `h` / `l` (Caches & Projects, Runtimes, Uninstall, Leftovers) | Fold / open the group; `l` or `Enter` on a row shows it in the Explorer |
| `m` (Caches & Projects) | Mark the selected folder and every folder that shares its data; press again to unmark them |
| `m` (Docker) | Mark the stopped containers the selected image or volume needs removed first; press again to unmark them |
| `b` (Docker) | Look inside the selected image or volume, read-only (`l` / `Enter` open a folder, `h` up, `Esc` back, `s` sorts, `/` filters) |
| `J` / `K` (Docker) | Scroll the details pane (the whole command line of a layer or build step) |
| `m` / `M` / `a` (Time Machine) | Measure the selected snapshot / every snapshot / mark all snapshots |
| `r` / `D` / `E` (ToBeDeleted screen) | Restore / delete for good / empty all |
| `q`, `Esc` | Quit (`Esc` first clears an active filter, then goes up a folder; quitting asks first if ToBeDeleted still holds items). While a tool command runs, `Esc` cancels it and `q` / `Ctrl-C` cancel it and quit |
| mouse | Click rows, menu entries, and treemap boxes or bars (a click on the graph selects that row in the list); scroll with the wheel; hover a graph box for a tooltip |

In the review dialog the list is one scrolling list: the folders first, then the tool commands at its end. It scrolls with `j`/`k`, the arrows, `PageUp`/`PageDown`, `Ctrl-F`/`Ctrl-B`, `Home`/`End` or the mouse wheel (a line under the list says which lines you are looking at, e.g. `lines 11-20 of 57`). Nothing is ever cut for a short terminal. When the list scrolls and contains commands, `y` stays off until you have scrolled to the end (`End` jumps there), so a command you never saw cannot run. `Enter` moves items to ToBeDeleted, `x` offers "move, then empty now" (irreversible), `f` switches a command that has a force form (`uv cache clean`) to `--force`, `Esc` cancels. Irreversible actions need an explicit `y`; `Enter` never confirms them. When the review contains tool commands, only `y` runs it.

## Safety

- Nothing is deleted without a review screen and a confirmation. The default action is always the reversible one.
- System paths, your top-level user folders, keychains, mail, secrets (`.ssh`, `.gnupg`, `.aws`), Photos libraries and disk images, and the ToBeDeleted folder itself are protected and cannot be marked.
- A folder that contains a protected item (for example `~/.config` holding `gcloud`) cannot be moved or deleted as a whole; the app reports which protected item blocked it.
- Quitting while a delete is running waits for it to finish, and a too-small terminal accepts only `q` / Ctrl-C, so a stray key never acts on a screen you cannot see.
- Tool commands run only after you confirm with `y` (never `Enter`). The app never adds a `--force` on its own: you choose it with `f` in the review (shown in red with a warning), or retry with it after a lock failure, and it still goes through the review. A running command can always be cancelled, and everything it started is stopped with it.
- Build products (a built program, an APK, a bundle, a `dist` folder) are listed as separate rows marked *(kept)*; marking a project or a section never selects them. Caches that may hold offline downloads or saved media (Spotify, Telegram, model caches, sync clients...) are drawn as caution, never as safe, but you can still clear them.
- SDKs and toolchain installs (a Flutter SDK, Node or Rust installs, editor extensions, app bundles) are recognised by their layout and never listed as projects, wherever they live.
- Docker removals (`docker image rm`, `container rm`, `volume rm`, build-cache prune) cannot be undone: they run only after `y` in the review, volumes (which can hold databases) are never taken by a bulk mark, and anything a running container holds cannot be marked.
- The app never asks for, reads or stores your password. The only admin step is the Uninstall/Leftovers batch described above: macOS's own prompt, after the review, minimal `mv` + `chown` commands built from separate arguments and checked against the protected list just before they run; if it fails the exact commands are shown for you to run.

## Development

```bash
cargo fmt                                   # format
cargo clippy --all-targets -- -D warnings   # lint
cargo test                                  # unit and integration tests
cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup   # end-to-end run in a pseudo-terminal
```

The smoke script drives the real binary in a throwaway `HOME` (explorer, filter, mark, review, soft delete, restore, rescan, the Caches & Projects screen, the irreversible path, quit); set `DUMP=1` to print the screens it sees. A few slow checks only run on request, for example `cargo test --release stress_ -- --ignored --nocapture` (timing on a clone-heavy tree) and `cargo test real_uv -- --ignored --nocapture` (cancelling the real `uv` while it waits on a held cache lock, in a throwaway cache).

## Documentation

- Design: [`docs/superpowers/specs/2026-11-06-maccleanup-design.md`](docs/superpowers/specs/2026-11-06-maccleanup-design.md)
- Caches & Projects design (section 11 records what was built and what is deferred): [`docs/superpowers/specs/2026-11-06-caches-projects-design.md`](docs/superpowers/specs/2026-11-06-caches-projects-design.md) and plan: [`docs/superpowers/plans/2026-11-06-caches-projects.md`](docs/superpowers/plans/2026-11-06-caches-projects.md)
- Milestone 1 plan: [`docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md`](docs/superpowers/plans/2026-11-06-maccleanup-milestone-1.md)
- Contributor and AI-assistant guidance: [`CLAUDE.md`](CLAUDE.md)
