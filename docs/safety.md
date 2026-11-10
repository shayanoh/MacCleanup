# Safety

How MacCleanup keeps you from deleting something you need.

- Nothing is deleted without a review screen and a confirmation. The default action is always the reversible one.
- System paths, your top-level user folders, keychains, mail, secrets (`.ssh`, `.gnupg`, `.aws`), Photos libraries and disk images, and the ToBeDeleted folder itself are protected and cannot be marked.
- A folder that contains a protected item (for example `~/.config` holding `gcloud`) cannot be moved or deleted as a whole; the app reports which protected item blocked it.
- Quitting while a delete is running waits for it to finish, and a too-small terminal accepts only `q` / Ctrl-C, so a stray key never acts on a screen you cannot see.
- Tool commands run only after you confirm with `y` (never `Enter`). The app never adds a `--force` on its own: you choose it with `f` in the review (shown in red with a warning), or retry with it after a lock failure, and it still goes through the review. A running command can always be cancelled, and everything it started is stopped with it.
- Build products (a built program, an APK, a bundle, a `dist` folder) are listed as separate rows marked *(kept)*; marking a project or a section never selects them. Caches that may hold offline downloads or saved media (Spotify, Telegram, model caches, sync clients...) are drawn as caution, never as safe, but you can still clear them.
- SDKs and toolchain installs (a Flutter SDK, Node or Rust installs, editor extensions, app bundles) are recognised by their layout and never listed as projects, wherever they live.
- Docker removals (`docker image rm`, `container rm`, `volume rm`, build-cache prune) cannot be undone: they run only after `y` in the review, volumes (which can hold databases) are never taken by a bulk mark, and anything a running container holds cannot be marked.
- The app never asks for, reads or stores your password. Nothing outside your home folder can be marked except what the Uninstall and Leftovers pages found for an app, and never Apple's or the system's. The only admin step is the Uninstall/Leftovers batch described above: macOS's own prompt, after the review, minimal `mv` + `chown` commands built from separate arguments and checked against the protected list just before they run; if it fails the exact commands are shown for you to run.
