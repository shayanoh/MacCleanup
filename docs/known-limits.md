# Known limits and roadmap

**Planned pages:** a Homebrew page, simulator runtimes, Go, Deno and JDKs, and large and duplicate files.

**Uninstall and Leftovers, known gaps** (none of them can delete something it should not; they leave something undone or imprecise):

- `launchctl bootout` is never run. A launch daemon or agent that is moved out keeps running until the next logout or reboot (the elevated script only does `mv` and `chown`; stopping a system daemon needs root too). Next step: add the bootout to the same single password prompt.
- A launch daemon restored from ToBeDeleted comes back owned by you (the move is followed by `chown -R`), and launchd refuses to load a daemon plist that root does not own. Restore would need to give it back to root.
- `port uninstall` normally needs root and is run as you, so it may fail with its own message. The fix is to run it through the same password prompt.
- The apps and leftovers are read once per session. An app installed or removed while the app runs is noticed on the next start (`r` rescans folders but does not re-read the app list), so a just-removed app can briefly look like a leftover.
- An app whose `Info.plist` cannot be read is not listed, and data that might be its is hidden from Leftovers by name. Reading its id with Spotlight (`kMDItemCFBundleIdentifier`) instead would list it.
- A vendor folder shared by several apps counts in each of their totals.
- Some Leftovers groups are SDK or tool data that other software still uses (`com.install4j.installations`, `com.crashlytics.data`). They pass every check (no installed id, idle for 30 days), so check what a group is before you delete it; every group says why it is listed.
- Sizes outside your home folder are plain allocated bytes (no clone accounting), and the real `osascript` prompt, a cancelled prompt and a session without a GUI have been tested only with fakes.
- Other users' folders, other volumes, system and kernel extensions, and receipts in `/var/db/receipts` are not looked at.
- The second whole-branch code review was not repeated after the last round of changes (progress display, preferences files, tags, fold all).
