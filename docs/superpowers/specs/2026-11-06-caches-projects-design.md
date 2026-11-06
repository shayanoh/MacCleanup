# MacCleanup: Caches & Projects screen (milestone 2) design

Status: draft for approval, 2026-11-06. Parent design: `2026-11-06-maccleanup-design.md` (goals G3.4 caches, G3.5 project artifacts, G5 accounting, section 12 real space). Convention as in the parent: **[User]** = decided by the user, **[My call]** = decided by the author, open to change.

## 1. Intent and why

The user's Mac is 94% full. Most of the reclaimable space is rebuildable: tool caches (pnpm, npm, cargo, Xcode, ...) and per-project build output (`node_modules`, `target`, `.next`, ...). Doing this by hand means remembering paths and commands.

The catch, and the reason this is its own milestone: on this Mac pnpm clones files (APFS copy-on-write) into every project's `node_modules`. Deleting the pnpm store alone frees almost nothing, and deleting one project's `node_modules` frees almost nothing, until every holder of the same data is gone. Per-row sizes lie. The screen must show the user the real, joint effect of what they mark, live, and name the related folders. **[User]**

Success: the user marks things, watches the "frees" number move as they mark and unmark (including keeping one project's `node_modules` while clearing everything else), sees exactly which folders and which tool commands will run, and the finish screen later confirms what was truly freed.

## 2. Goals

- G1. One new screen, **Caches & Projects**, in the sidebar (replaces the dimmed Caches and Projects entries; one screen, not two). **[User]** asked for projects and tools in a single list.
- G2. The list has two sections with clearly different header styles: **PROJECTS** (a group per project folder, containing that project's artifacts) and **TOOLS & CACHES** (a group per tool, containing its caches). **[User]**
- G3. The footer and every section header show **Frees computed for the whole marked set**, recomputed on every mark and unmark. A cloned or hard-linked file frees only when every holder is in the marked set. **[User]**
- G4. A detail pane for the highlighted row: the folders that go with it, why only part of its size frees, which unmarked folders keep data alive, and a key to mark all related folders. **[User]**
- G5. The screen behaves like the Explorer wherever it can (section 4). **[User]**: this is a standing rule, recorded in `CLAUDE.md`.
- G6. Deleting goes through the existing review screen. Tool-owned caches list the exact command that will run; the rest are soft-deleted. **[User]**
- G7. Real accounting after the run: free space before vs after (existing `Accounting`), plus the tool's own reported number where it prints one. **[User]** (parent G5)

- G8. **One shared selection across all screens** (standing rule, section 4a). **[User]**

## 3. Non-goals and anti-goals

- Not in this milestone: Docker, leftovers, uninstaller, simulators/runtimes, Time Machine snapshots, large/old files, duplicates (all stay dimmed in the sidebar).
- Anti-goal: deleting anything that is not clearly rebuildable. A folder named `node_modules` or `target` that is not next to its project marker is not offered. **[My call]**
- Anti-goal: a total that over-promises. A number shown as "frees" must be what free space will really do, or be labelled as an estimate. **[User]** (spirit of parent section 12)
- Anti-goal: running a tool command without it being listed in the review screen. **[User]**
- No privilege escalation (parent rule).

## 4. Consistency with the Explorer (standing rule) **[User]**

The Caches & Projects screen must look and behave like the Explorer as far as the content allows, and any later screen must follow the same rule. Specifically:

- Same list pane plus right pane layout; same column set, in the same order and widths: name, Size, On disk, Frees, share bar (hidden first when narrow), with the safety dot and mark glyph in the same place. `Row.disk` of a folder is its footprint, as in the Explorer.
- Same sort keys and the same header-order cycle on `s` (Name, Size, On disk, Frees), sorted arrow on the active header. Sorting applies inside each group, and groups are ordered by the same key on their totals. Sections stay in fixed order (PROJECTS, then TOOLS & CACHES).
- Same graph pane: `g` cycles Bars, Treemap, Hidden; `[` `]` resize; the graph highlights and follows the selection; treemap is the default. The graph shows the rows of the highlighted group (so it is comparable to the Explorer showing one folder). **[My call]**
- Same filter box (`/`, fzf-style `Query`), same Esc behavior, same scrollbar and `a/b` position, same totals line above the list (here: section totals plus marked totals).
- Same keys: `j/k`, `space` mark, `d` review, `r` rescan, `?` help, same mouse behavior. `h`/`l` fold and unfold the group under the cursor (the Explorer's out/into have no direct meaning here); `Enter` on a row opens that path in the Explorer. **[My call]**
- Same animation rules (`--no-animation`), same ASCII fallback, same `ui::clean`/`fit` for every file name.
- Same safety dots (green safe, amber caution, red protected). Rebuildable project artifacts are amber with "how it comes back" text; known-safe caches green. **[My call]**

When a column or key changes in the Explorer, the same change is made here in the same commit, and the tests and README key table are updated together (extends the existing "sort order follows headers" rule).

## 4a. One shared selection (standing rule) **[User]**

Viable and decided: there is **one** selection for the whole app, not one per screen. A folder marked in the Explorer is marked here, and the other way round. `d` on any screen opens the review of **everything** marked, wherever it was marked. A future screen that cannot share (it must say so, here, before it is built) would need its own list that is clearly explained on screen; none is planned.

How it works:
- The existing `App::marks` (path to usage, one set for the app) stays the single source for folder marks. This screen reads and writes it. Mark state on a row here is "marked", "covered" (an ancestor folder is marked, shown dimmed with the covering path in the detail pane), or unmarked.
- The existing nesting rule applies unchanged on every screen: marking a parent absorbs marked children; marking something inside a marked folder is refused with a notice. (So marking `~/dev/shop` in the Explorer replaces a mark on `~/dev/shop/node_modules` made here, and the notice says so.)
- Tool-owned rows (a cleanup command, no folder to move) are a second kind of selection entry, keyed by the tool's registry id. They live in the same selection, show in the review with their command, and count in the marked totals on every screen. They are only toggled from this screen, since the Explorer has no row for them. The Explorer footer says "+ N tool cleanups" when any are marked. **[My call]**
- Frees for the marked set (section 7) is always computed over the whole selection, so the Explorer's marked-space detail and this screen's footer give the same number for the same selection.
- The selection is not persisted between runs (as today). Leaving a screen never clears it; only deleting, unmarking or `Esc`-clear-all does.
- Every future milestone (Docker items, leftovers, apps, snapshots) adds its own entry kinds to this same selection and the same review. This extends the standing rules in `CLAUDE.md`.

## 5. The screen

```
 󰉋 PROJECTS                       9.4 GB   frees 7.5 GB      │ detail pane / graph
  ~/dev/shop                       Size   Frees             │
    □ node_modules                 2.9 GB  0.0 GB           │
    ■ .next                        0.4 GB  0.4 GB           │
  ~/dev/api  Rust                                           │
    ■ target                       6.9 GB  6.9 GB           │
 󰏖 TOOLS & CACHES                 22.1 GB  frees 5.6 GB      │
  pnpm   ■ store                   8.4 GB  4.0 GB           │
  npm    ■ _cacache                2.1 GB  2.1 GB           │
 marked 8 · frees 12.4 GB now · +3.0 GB if shop/node_modules is marked · [d] review
```

(Rough; the Explorer's real column layout governs. Mockups are in `.superpowers/brainstorm/`, not committed.)

- **Section headers** are full-width bands with distinct colors (blue PROJECTS, purple TOOLS & CACHES), showing the section's size and its Frees for the currently marked rows. **Group headers** (a project or a tool) are lighter bold lines. **[User]** asked that the two kinds look different enough.
- **Right column:** the graph on top (modes as in the Explorer) and the detail pane below it; when the graph is hidden the detail pane takes the whole column. **[My call]**
- **Footer line:** marked count, Frees now for the marked set, and what marking the unmarked holders of shared data would add. Marking never touches disk.
- **Detail pane for a row:** path, safety class and how it regenerates, the command that will clean it (if tool-owned), a `Space` breakdown (frees now, kept alive by unmarked folders, held by snapshots, shared inside), and a list of the other folders sharing the same data with their marked/unmarked state. A key marks all of them and says what the total then becomes. **[User]** asked for the related folders and the freed amount.
- Rows still being scanned or sized show the same queued/spinner markers as the Explorer. Frees for the marked set is only exact once the scan is idle, and shows "scanning" until then (same as `Scanner::space`).

## 6. What gets listed

**Project artifacts [User named first five; rest My call, from parent G3.5].** Found in the scan tree under the scan root (default `~`): `node_modules`, `.next`, `.turbo`, Rust `target`, Xcode `DerivedData` and build output, `.dart_tool`, `Pods`, Flutter `build`. A candidate counts only when its project marker is a sibling: `package.json` (node_modules, .next, .turbo), `Cargo.toml` (target), `pubspec.yaml` (.dart_tool, build), `Podfile` (Pods). Nested copies inside an artifact (for example `node_modules` within `node_modules`) are not listed separately. Parent rule: final binaries and app bundles are kept unless the user opts in. **[My call]** here: `target` is offered only as a whole folder (amber), and its review line says that built binaries inside go too; marking it is the opt-in. Nothing is offered inside `.app` bundles or install locations.

**Tool caches [User listed tools; paths My call].** A hard-coded registry (like `safety.rs`, tested) of known cache locations per tool: pnpm store, npm `_cacache`, yarn, cargo registry and git, go build cache and module cache, flutter/pub cache, uv and pip, brew cache, Xcode DerivedData and caches, Playwright browsers, and `~/Library/Caches` / `~/.cache` entries by app where identifiable. The registry is data, extendable. Each tool entry records: paths, safety class, regeneration note, and optional cleanup command. A tool whose binary or paths do not exist is simply not shown. Multiple installs of npm/pnpm/node must not be assumed single. **[User, 2026-11-06]** The user runs fnm with corepack-managed pnpm, and other Macs use nvm or volta. Rules: (1) every install is found (the shell's `PATH`, every fnm/nvm/volta Node version, standalone locations), the same binary reached two ways counts once; (2) each pnpm major keeps its own store folder (`store/v10`, `store/v11`), so every store folder is an entry of its own and is cleaned by the pnpm that reports it (`pnpm store path`); a store no installed pnpm reports is moved instead, with a note; the npm cache folder likewise comes from `npm config get cache` and must be under the home folder; (3) a command is pinned to one exact install (absolute path), runs in the home folder so a corepack shim uses its default and not a project's version, with `COREPACK_ENABLE_NETWORK=0` and `COREPACK_ENABLE_DOWNLOAD_PROMPT=0` so it can never download or wait for a prompt, with the matching Node first on `PATH`, no stdin, and a timeout; (4) discovery runs programs, so it runs on a background thread and the screen shows what it has until it finishes; a query that fails or times out only downgrades that entry to a folder move. **[My call]** the detailed mechanism.

**Data source and identity [My call].** The screen reads the app's existing scan tree (default root `~`); it does not start a scan of its own, and clone/link facts are only complete for scanned files. If the scan root is narrowed (`PATH`) or Full Disk Access hides locations, a banner says what is not covered and the list shows only what was scanned. While the scan runs, rows appear as found with the Explorer's queued/spinner markers, and set-wise figures show "scanning" until the scan is idle. A folder has one identity (its canonical path) and appears in exactly one group: the tool registry wins over project-artifact detection (so Xcode `DerivedData` is listed under Xcode only; a project's Rust `target` is a project artifact). A candidate inside another candidate is absorbed into it, matching the marks nesting rule. The "shares data with" list in the detail pane covers any folder in the scanned tree, not only listed candidates; marking from it goes through the shared selection.

## 7. Marking, Frees, and the set-wise calculation

- A mark applies to a row (one folder). Marking a group header marks its rows; marking a section marks all rows. Shared-data holders are linked in the detail pane only; marking one does not mark the others (the user may deliberately keep one). **[User]**
- **Row Frees column [My call]:** a row's Frees is what deleting that row **alone** frees, exactly as in the Explorer (stable, so sorting by Frees never reshuffles while the user marks). The live, set-wise effect of the marks is shown in the footer, in the section and group headers, in the detail pane ("with your marks: X"), and in the review. The mockups showed the column itself changing; it does not.
- Frees for the marked set: the existing space computation (clone sets with members = containing folders, hard-link groups, snapshot attribution; `scan.rs`) is evaluated once for the union of the marked folders, not summed per folder. A clone set frees only if all its members lie inside the marked set. Identity must hold for the set: `footprint == frees_now + snapshots + shared_inside + shared_outside` (tested, including with `cp -c` fixtures). The same call yields per-row Frees (this row's contribution given the current marks) so that the Frees column changes as marks change. **[My call on exact column semantics:]** with nothing marked, a row's Frees is what deleting it alone frees (same as the Explorer); with marks, it is what it adds on top of the current marks.
- The computation must not block the UI thread. It is recomputed in a background worker on each mark change; the UI shows the previous figure with a spinner until the new one arrives (stale figures never shown as final). The scan-time clone/link tables are read, not rebuilt. **[My call]**
- Tool-owned rows (with a cleanup command) do not take part in the exact total, because what a command frees is not knowable beforehand (for example `pnpm store prune` only removes unreferenced packages). They show "up to X", where X is the set-wise result computed as if that row's folder were also marked, and are summed separately in the footer ("+ up to X by tools"). The real number comes from the before/after measurement and the tool's own output. **[My call]**
- **pnpm case, spelled out [My call, from the user's rule that pnpm is cleaned through its own command]:** the pnpm store row is command-owned (`pnpm store prune`). The detail pane still lists the project `node_modules` that clone its data (clone relationships are computed from files, not from the command). Typical flow: mark the project `node_modules` to clear (folder soft delete), and the store row then shows a larger "up to X" that a `pnpm store prune` may reclaim afterwards. Whether prune recognizes cloned (not hard-linked) packages as unreferenced must be verified against pnpm's current docs and a real run before the row promises anything; if it cannot, the row says so and offers soft-deleting the store folder instead (safe, since clones are independent copies and pnpm re-downloads). 
- **Command-owned vs folder-owned is per registry entry [My call]:** an entry is command-owned only where the tool has a documented clean command (verified from current docs) and the user's rule applies (pnpm, docker, brew, npm, go, uv, pip, Xcode where `xcrun`/`xcodebuild` offers it); otherwise it is folder-owned and soft-deleted. The registry records the mode.

## 8. Review and delete

- `d` opens the existing review dialog. Folder rows list path and size as today and are soft-deleted into ToBeDeleted (the default). Tool-owned rows list the exact command line, are labelled irreversible, and are not soft-deletable. **[User]**
- The review shows the exact set-wise Frees for the whole selection and which marked rows are still kept alive by unmarked holders.
- **Mixed review [My call]:** one review dialog. If any tool command is in it, the whole review needs an explicit `y` (Enter never confirms an irreversible action); with only folder moves, it confirms as today. Folder moves run first, then commands in the listed order. A failed command is reported on the finish screen and does not undo moves or stop the remaining commands. `x` ("move, then empty now") applies to the folder rows only; commands run in both modes.
- **Bulk marks skip command rows [My call, safety]:** marking a group or a section marks folder rows only. A command row is marked only by marking that row itself, so one keystroke can never queue an irreversible command.
- Commands run in the existing background job machinery (a `DeleteJob`-style job), never on the UI thread, with live output/progress (counters and spinner when the total is unknown). Tool integrations sit behind a command-runner abstraction and are tested with mocks; real runs are done by the user manually. Commands run without a shell, with explicit arguments. Never escalate privileges; if a command needs admin rights, use the existing "show the command with Copy, Cancel, I ran it" flow (the flow itself is not yet built; if needed here, it is built as part of this milestone). **[User]** (parent section 10)
- Protected-path policy applies to every folder row before deletion (`Policy::protected_descendant`, checked on the delete thread). Nothing outside the home folder is offered in this release.
- Accounting: `Accounting` records pending (soft-deleted) separately from freed; tool-driven results count as freed only by measured free space before vs after, with the tool's own number alongside when it prints one.

## 9. Constraints

- Rust, ratatui 0.30, crates added only with `cargo add` (`CLAUDE.md`).
- All scanning stays multithreaded; new discovery runs inside or alongside the existing scan, not as a second single-threaded walk.
- All new safety and registry logic is tested in `tempfile` sandboxes with a throwaway `HOME`, never against real data. Command execution is tested through the runner abstraction.
- Update `CLAUDE.md` (code map, the consistency rule, key table) and `README.md` in the same change.

## 10. Left to the builder

- Module layout (a `targets` or `catalog` module for the registry and project detection; a screen module under `ui/`), data model for groups and rows, how row/group state maps onto the Explorer's `Row`.
- How to discover artifacts efficiently from the existing scan tree versus a targeted walk.
- The background worker design for the set-wise Frees.
- The exact registry contents and commands per tool, verified against each tool's current documentation before use (not from memory).
- Key choices not named above, wording, colors beyond the section bands.
- Test plan, with emphasis on: the set-wise identity with clones, the keep-one-project case, marker-based detection, registry paths, review listing of commands, and the Explorer-consistency rule where it can be asserted (column order, sort cycle).
