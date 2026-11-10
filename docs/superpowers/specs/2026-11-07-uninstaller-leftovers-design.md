# Uninstaller and Leftovers pages (milestone 6)

Status: approved and built 2026-11-07 (released as 0.9.0; the remaining gaps are listed in the README, "What is left") (plan: `docs/superpowers/plans/2026-11-07-uninstaller-leftovers.md`). Deviations: the fourth column header is "Opened" ("Last used" does not fit the shared 10-cell column); `launchctl bootout` is not run (needs root; known gap); Frees is not shown; the terminal-chain lock is covered by the running-app lock.

## Why

Removing an app by dragging it to the Trash leaves its data behind, and some apps
(Autodesk Fusion) are only a thin launcher while the real program lives elsewhere. Today
nothing in MacCleanup shows which folders belong to which app, how big an app really is,
or what is left of apps that are already gone. The sidebar has two dimmed entries,
"Leftovers" (menu 3) and "Uninstall" (menu 4); this design fills both.

This is also the first feature that reads, and may change, things outside the home
folder (`/Applications`, `/Library`), so it settles how those are treated.

## Goals

- **Uninstall page**: list installed apps, each with the folders that belong to it, so
  one can see the real size of an app and remove it with its data in one review.
- **Leftovers page**: list groups of folders that belong to an app that is no longer
  installed. Shown only when there is positive evidence (see "Leftover evidence").
- Columns on both pages, sortable like the Explorer: **Name, Size, On disk, Last used**.
  An app never opened shows "never" and sorts as the oldest.
- Same look and behaviour as the Explorer / Caches & Projects pages (see "Look").
- Deletion goes through the shared selection (`App::marks`) and the existing review,
  soft delete first. Items that need admin rights are handled by one batch behind the
  system's own password prompt (see "Deleting outside home").

## Non-goals and anti-goals

- **Never list something as removable on a guess.** Wrongly hiding a leftover is
  acceptable; wrongly offering one is a failure even if the program works. When unsure,
  do not list it (optionally show a count "N folders not listed: unsure").
- Do not remove macOS itself or Apple's apps (see "Protection").
- Do **not** run an app's own vendor uninstaller (for example `Remove Autodesk Fusion.app`).
  It is treated as one more bundle in the app's group and removed with the rest.
- No private APIs, no changes to SIP-protected locations, no installing a privileged helper.
- The app never asks for, reads or stores a password.
- Not in this release: other users' homes, `/Library/Receipts` / `/var/db/receipts` edits
  (receipts may be shown as information only, see "Builder's calls"), uninstalling
  system extensions or kernel extensions, anything on another volume.

## Two pages, one finder

Both pages are the Caches & Projects list (`Screen`, sections, groups, `TRow`) with new
screens (`Screen::Uninstall`, `Screen::Leftovers`) and sections whose `page()` selects
them, as the Runtimes page does. Underneath is one shared module (working name
`catalog/apps.rs`) that:

1. builds an **installed index**: every app bundle in `/Applications`, `~/Applications`,
   `/System/Applications` and subfolders of those (for example `Chrome Apps.localized`,
   `Utilities`), with bundle id, name, version, path, owner, last used
   (`kMDItemLastUsedDate`, read with `mdls` or the Spotlight metadata API; missing = never)
   and executable. Nested bundles (`*.app`, `*.xpc`, `*.appex`, login items, helpers inside
   another app) contribute their ids too, so a helper's folders are never "orphaned";
2. finds **related locations** for an id (below) and groups them;
3. classifies each group as *installed* (Uninstall page) or *gone* (Leftovers page).

Discovery asks programs (`mdls`, package managers), so it runs on a thread through
`bg::Latest`, like `discover_tools`. Sizes come from the existing scanner machinery
(`bg::walk_pool`, clone-aware `Space`), never walked on the UI thread.

### Related locations

For a bundle id `ID` and app name `NAME` (home = `~`):

- The bundle itself, and its wrapper bundles (Fusion also has `Autodesk Fusion Service
  Utility.app` and `Remove Autodesk Fusion.app`).
- `~/Library/`: `Application Support/<ID|NAME>`, `Caches/<ID>`, `Containers/<ID>`,
  `Group Containers/*.<ID or group id>` (strip the leading team id, `UBF8T346G9.`),
  `Preferences/<ID>.plist` (and `ByHost`), `Saved Application State/<ID>.savedState`,
  `HTTPStorages/<ID>`, `WebKit/<ID>`, `Logs/<NAME>`, `Cookies/<ID>.binarycookies`,
  `LaunchAgents/<ID>*.plist`, `Application Scripts/<ID>`.
- Outside home (readable without admin): `/Library/Application Support/<NAME|vendor>`,
  `/Library/LaunchAgents`, `/Library/LaunchDaemons`, `/Library/PrivilegedHelperTools`,
  `/Library/Preferences/<ID>.plist`, `/Library/Caches/<ID>`.
- **Indirection**: when the bundle's executable is a shell script that names another
  folder (Autodesk Fusion's `Contents/MacOS/Autodesk Fusion` sets `destfolder=...`), that
  folder joins the group if it is inside `~/Library/Application Support` or `~/Library`.
  The script is read as text, never run.
- **Vendor folders**: a folder named after a vendor (for example `Application Support/Autodesk`)
  joins a group when the vendor part of the bundle id (`com.autodesk.*`) matches it. A vendor
  folder shared by several installed apps is shown under each, but it is markable only when
  every installed app of that vendor is in the selection. On the Leftovers page vendor folders
  never form a group on their own.

Fusion's own pieces use different ids (`com.autodesk.dls.streamer.scriptapp.Autodesk-Fusion`,
`com.Autodesk.streamer`, `com.autodesk.AdskIdentityManager`), which is why matching is
prefix- and vendor-aware and follows the launcher script, not exact-id only.

## Leftover evidence

A group is a leftover only if **all** hold:

1. its folder or file name is an explicit bundle id (or `<id>.plist`, `<id>.savedState`,
   `<teamid>.<id>`, `<teamid>.group.<id>`) found in a known per-app location above;
2. no installed bundle (including nested helpers) has that id or a longer id sharing it as a
   prefix with the same vendor; and no running process uses it;
3. it is not protected (see "Protection").

Name-only matches (`Application Support/Some App` with no id) are **not** leftovers. Launch
daemons and helper tools count as leftovers only when their plist's `Program` /
`ProgramArguments` target no longer exists. Every leftover group shows why it was listed
("no app with id com.x.y is installed") in the details pane.

## Protection

Hard-coded in `safety.rs` (`Policy`), tested in `tempfile` sandboxes. Never markable, listed
dimmed with the reason; the existing rule applies that a folder containing a protected item
is refused as a whole:

- anything under `/System`, `/usr`, `/bin`, `/sbin`, `/private` (other than listed
  per-app files), `/Library/Apple`, and `/System/Applications`;
- any bundle or folder whose id starts with `com.apple.` (this includes Apple apps copied into
  `/Applications`: Safari, Xcode, GarageBand, Final Cut, iWork) and Apple's folders in
  `Library`;
- the running app, its parent terminal chain (the terminal MacCleanup runs in), MacCleanup
  itself, and any app that is running now;
- the Mac App Store and Setup Assistant style components (`com.apple.*` covers them);
- `~/Library` itself, `~/Library/Application Support` itself, any top-level folder (only
  children matching rules above are ever offered), and `Keychains`.

Everything outside the home folder stays protected **by default** and is unlocked only for
paths that this feature found through its rules above; the elevated batch re-checks each path
against `Policy` immediately before running (see below).

## Deleting

1. **Review**: the usual review screen lists every folder with its size. Rows that need admin
   are marked and their exact commands are listed. Confirmation is `y` only when the
   review contains commands.
2. **Try normal access first** for every item (move to the ToBeDeleted folder as today).
   Whatever fails with a permission error is collected.
3. **One elevated batch** for the collected items, run through the system's own password
   prompt (`osascript -e 'do shell script ... with administrator privileges'`). The app
   never sees the password. The batch is minimal and built from separate arguments, never a
   spliced string: per item `mv` of the exact absolute path into the holding folder and
   `chown -R <user>` of the moved copy so later steps need no root. For launch daemons/agents
   `launchctl bootout` runs first. Paths are validated (absolute, no `..`, inside an allowed
   root, not protected) immediately before the call.
4. If the user cancels the prompt, or no GUI session exists, or the batch fails, fall back
   to the existing dialog: the exact commands with a copy button, "Cancel" and "I ran the
   command"; then rescan and continue.
5. **Package-manager apps** (MacPorts, Homebrew casks) are removed with the manager's own
   command when it has one (`brew uninstall --cask <name>`, `port uninstall <name>`), pinned
   to an exact program path and run like the other tool commands (`CommandSpec`, no
   `--force`, cancellable, order after folder moves). The app's remaining folders stay
   markable as ordinary moves, like Runtimes leftovers. Detection is by manager metadata
   (owner `macports`, `brew list --cask` / Caskroom), not by name.
6. After a delete: `Scanner::forget_dir` / `rescan_dir`, free-space accounting as for other
   pages (soft-deleted data is "pending", never "freed").

`CLAUDE.md` currently says "Never escalate privileges or ask for a password". It must be
rewritten when this is built: the app still never asks for or reads a password; the system
prompt is used only for this batch, and only after the review.

## Look

Same as Explorer / Caches & Projects (`ui/explorer.rs` helpers `cols`, `header_line`,
`number_spans`; `ui/targets.rs` for sections and groups):

- Columns in order: Name, Size, On disk, Last used. The sort cycle (`s`) follows that order;
  README key table, tests and cycle change together. Last used gives way before names get
  narrower than 16 cells; the share bar goes first.
- Last used: from the app (newest of its bundles); on Leftovers there is no app, so the
  column is "Modified" (newest modification time inside the group).
- Group header = app name (or bundle id on Leftovers) with total size; rows beneath are the
  related folders with their own size and mark box. Marking a header marks every markable row
  beneath it (as the Runtimes page does); protected, refusing and shared-vendor rows are
  skipped.
- Size of an app row is the sum of its bundle and related folders; the bundle's own size is
  its own row. Filter (`Query`), graph pane (`g`, `[`, `]`), totals line, scrollbar, mouse and
  footer behave as on the other pages. File names go through `ui::clean/fit`.

## Safety summary (the review of any change must check these)

- A leftover needs positive evidence; unsure means not listed.
- Apple, system, running and self are never removable.
- Elevated commands are exact, argument-separated and re-validated just before running.
- Nothing is deleted without the review; soft delete is the default; Enter never confirms
  an irreversible action.
- Real commands (brew, port, osascript, launchctl) are never run in tests: use `FakeRunner`
  or a no-op `CommandRunner`. Tests of discovery use sandbox `.app` fixtures under a
  throwaway `HOME`.

## Builder's calls (open to change)

- Mac App Store apps are treated like other apps; they are plain folders owned by root or
  the user. Moving them works the same way.
- Login items and `/var/db/receipts` are shown as information in the details pane when they
  name the app, not as removable rows.
- Running detection uses the process list (executable path under the bundle), not
  `NSWorkspace`.
- Group containers are matched after stripping a leading 10-character team id.
- Last used comes from Spotlight; if Spotlight is off or has no date, the cell says "never"
  (a reindex can make an old app look never-opened; the details pane says so).

## Calls made after the builder check (open to change)

- **Elevation is only the osascript prompt.** `sudo` and in-terminal prompts are not used.
  If the prompt cannot be used, the copy-the-command dialog is the fallback (decided with the user).
- **Review is honest about it.** Before the review opens, each outside-home item gets a
  write check (`access(W_OK)` on the item and its parent). Rows that probably need admin are
  labelled "may ask for your password" in the review, and the review needs `y`. The move is
  still tried with normal access first; the password prompt only appears for what then fails,
  and only after the user confirmed a review that said so. The prompt never appears without a
  review that mentioned it.
- **Holding folder.** Elevated moves use the same ToBeDeleted folder. `/Applications` and
  the home folder are on the same data volume, so `mv` is a rename; any item on another
  volume or SIP-protected is not offered at all. The manifest entry is written before the
  elevated move, as for normal moves, and `chown -R` follows it; an interrupted run leaves
  a deletable stale entry.
- **Bulk marks.** Marking an app header marks its bundle (or, for a package-manager app,
  its manager command) and the folders beneath it inside the home folder. It skips outside-home
  items other than the bundle (launch daemons, helper tools, `/Library` folders), shared vendor
  folders, protected and refusing rows; those are marked one by one. The review still gates
  everything with `y`.
- **Running.** An app is running if a process's executable lies under any of its bundles. A
  running app's whole group is non-markable (not only the bundle row), and the check is run
  again when the review opens and just before the delete starts. The terminal chain is the
  ppid chain of MacCleanup up to the first non-shell process; tmux and ssh are covered by
  simply protecting whatever is in that chain.
- **Launch items.** `launchctl bootout` uses `system` for `/Library/LaunchDaemons`,
  `gui/<uid>` for agents. If bootout fails the item is still listed, still markable and
  the failure is shown in the review result; the file is moved anyway only if the target
  process is not running. `PrivilegedHelperTools` entries are removed with their matching
  `LaunchDaemons` plist, as one group.
- **Vendor matching starts strict.** A vendor folder joins a group only when at least the
  first two id segments match (`com.autodesk`) **and** the folder name equals the vendor name
  case-insensitively. A built-in list of vendors that span unrelated apps (`com.google`,
  `com.microsoft`, `com.adobe`, `org.mozilla`, `com.jetbrains`) never attaches a vendor-wide
  folder; their per-app folders still match by full id. A vendor folder never joins a
  Leftovers group.
- **Installed index must be complete before leftovers show.** Besides the scan roots, the
  index asks Spotlight (`kMDItemContentType == com.apple.application-bundle`) and the
  LaunchServices database for registered apps, and reads Homebrew's Caskroom and MacPorts'
  application folders when present. If Spotlight is disabled or the query fails, the Leftovers
  page lists nothing and says why. Apps in other places (external volumes, Setapp, JetBrains
  Toolbox, the Trash) are found by these queries; one missing from them could appear as a
  leftover, which is why every leftover shows its reason and goes through the review.
- **Package managers.** For a cask or port, the bundle in `/Applications` is the bundle; the
  Caskroom or MacPorts copy is not counted twice. `brew` and `port` are looked up with
  `catalog/resolve.rs`; a missing manager means "not a manager app". Manager queries run once
  on the discovery thread and are cached until the next refresh.
- **Fourth column.** Deliberately different: Uninstall has "Last used", Leftovers has
  "Modified" (newest modification time inside the group, like the Explorer's `Node::mtime`).
  Both sort cycles follow their own header order.
- **Sizes outside home** use the same walk code as the scanner (clone-aware sizes), run on
  `bg::walk_pool()`. A "Frees" figure is not shown on these pages, so there is no
  outside-home Frees to get wrong; soft-moved data counts as pending as usual. A folder that
  cannot be read (no Full Disk Access) shows its size as unknown and is not markable.

## Left to the builder

- File names and module layout (`catalog/apps.rs`, `app/apppage.rs`, `ui/apps.rs` or reuse of
  `ui/targets.rs`), and how many unit and integration tests each rule needs.
- The exact `osascript` quoting and error mapping; whether Touch ID appears is up to the OS.
- Time budgets, thread use and caching of the installed index.

## Done when

Both sidebar entries open working pages with the columns, sorting, filter and graph above;
a marked app (including Fusion with its payload) reviews, moves and frees space correctly;
a root-owned item is removed through one password prompt; protected items cannot be marked;
`CLAUDE.md`, `README.md` (key table, menu numbers) and this document are updated.
