# Time Machine snapshots page (milestone 4)

Status: proposed, waiting for approval. Written 2026-11-07.

## Why

Local Time Machine snapshots hold space that no folder shows. The scan already
reports a total (`snapshots`, see `scan.rs`) and the Explorer says "held by
snapshots", but there is no place to see each snapshot, what deleting it would
free, or to delete it. The sidebar entry is dimmed "coming".

## Goals

- List local Time Machine snapshots (`com.apple.TimeMachine.<date>.local`) with
  date, age and an estimate of what deleting each one frees.
- Show the total held by all of them.
- Delete one, several or all, through the shared selection and review, with a
  single `y` (irreversible: snapshots cannot be restored by this app).
- After a delete, keep the free-space figure live while APFS releases space
  gradually: the footer ("free X (start Y)") and the finish screen update on
  their own until the number settles.

## Non-goals and anti-goals

- Never touch snapshots that are not Time Machine's (`com.apple.os.update-*`,
  `com.apple.bless.*`): listed dimmed, with a note, never markable.
- No root, no password prompt, no private Apple APIs. If a step needs admin
  rights the page shows the exact command (copy button, Cancel, "I ran the
  command"); nothing here currently does.
- No thinning (`thinlocalsnapshots`) and no creating snapshots in this release.
- Do not present an estimate as exact: the size column says "about".

## Commands (verified on macOS 15.7, no root)

| Need | Command |
|---|---|
| List | `tmutil listlocalsnapshotdates /` (dates `YYYY-MM-DD-HHMMSS`), names from `tmutil listlocalsnapshots /` |
| Detail | `diskutil apfs listSnapshots -plist /System/Volumes/Data` (XID, purgeable flag, UUID; no size) |
| Delete one | `tmutil deletelocalsnapshots <date>` |
| Delete all | the same once per date (a result and a progress step per snapshot); `/` also works but gives no per-snapshot result |

Dates are validated against `^\d{4}-\d{2}-\d{2}-\d{6}$` before they reach a
command line. The commands run through `CommandRunner` with a no-op or
`FakeRunner` in tests; real `tmutil` is never run in tests.

## Size of a snapshot (the hard part)

No command prints it (see memory `time-machine-snapshot-size`: `diskutil` has no
size, `tmutil uniquesize/compare` need a Time Machine disk, `sizelocalsnapshotsof`
needs root). Disk Utility shows "Private Size" from private APIs. Our estimate,
verified against it (138.1 MB vs 137.9 MB):

1. Find the snapshot's mount (the system mounts it under
   `/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/<Host>/<date>/Macintosh HD - Data`;
   if not mounted, `mount_apfs -o rdonly,nobrowse -s <name> /System/Volumes/Data <our dir>`,
   unmounted afterwards, only what we mounted).
2. Walk it in parallel (about 50 s for 3 million files). For each file compare with
   `/System/Volumes/Data/<same path>`:
   - missing live: adds `blocks * 512`;
   - size or mtime differs: adds `max(0, snap_blocks - (live_blocks - live_private))`
     when the live file has no clones (`clone_facts(live).private`);
   - the same: adds nothing.
3. A file missing at the same path is looked up by inode on the live volume (`fsgetpath(2)`, declared by hand: `libc` has no binding): if it exists elsewhere it was renamed or moved and is compared like any other file. Without this a build folder (rustc renames its output) inflated one estimate from about 726 MB to 1169 MB.
4. The result is a range `Held { low, high }`: `low` excludes renamed files, `high` counts them as deleted. A file identical (inode, size, mtime) in another snapshot is private to none and is skipped, so the figure follows Disk Utility's Private Size. File-system records are in neither number.
5. Never use `private` of a file inside a snapshot: it reads 0 for every file.

It is measured on demand (key `m`), on a thread through `bg::Latest`, with a
progress marquee and cancel, because it takes a minute. It is not run at start.

Open edge: with two or more snapshots, a block shared only by two snapshots is
counted for both. The first version shows each figure as "about X (vs now)"; a
test with two snapshots (create one with `tmutil localsnapshot`) decides whether
to subtract the previous snapshot's figure. Until then the total on the page is
the scan's exact `snapshots` figure, not the sum of the rows.

## Screen

Same shape as the Explorer family: columns Name (date and age), Size (about),
Kind (Time Machine / system), the graph pane, sort cycle in header order,
filter, totals line, mouse, keys. Space marks, `d` reviews, `m` measures the
selected snapshot (`M` all), `r` rereads. Marks go into the shared selection as
a new entry kind (like Docker items: `tool_marks`, id `snapshot:<date>`),
reviewed and run by the existing review and `CommandJob`, `y` only, order oldest
first.

## Live free space

- `App::free_now` is refreshed on a slow timer on every screen (every 2 s while
  idle), not only after jobs.
- After any snapshot delete the app enters a settling state: poll every second;
  leave it after 30 s with no change, or 5 minutes.
- The finish screen's "freed" figure is computed from the live value on every
  frame, so it counts up while the dialog is open.
- The quit summary and the footer already show start, now and change.

## Constraints from the project rules

Nothing slow on the UI thread; every job through `bg::Latest`; safety rules
(snapshots outside our own kind are protected); update `README.md` key table and
`CLAUDE.md` in the same change; sidebar digit stays the page's position.

## Left to the builder

Column widths, graph choice (bar of sizes), exact wording, how the review lists a
snapshot (name, age, about-size).
