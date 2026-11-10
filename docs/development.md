# Development

For contributors. Project rules and the code map are in [`CLAUDE.md`](../CLAUDE.md).

## Checks

```bash
cargo fmt                                   # format
cargo clippy --all-targets -- -D warnings   # lint
cargo test                                  # unit and integration tests
cargo build && python3 -I scripts/smoke_pty.py target/debug/maccleanup   # end-to-end run in a pseudo-terminal
```

The smoke script drives the real binary in a throwaway `HOME` (explorer, filter, mark, review, soft delete, restore, rescan, the Caches & Projects screen, the irreversible path, quit); set `DUMP=1` to print the screens it sees. A few slow checks only run on request, for example `cargo test --release stress_ -- --ignored --nocapture` (timing on a clone-heavy tree) and `cargo test real_uv -- --ignored --nocapture` (cancelling the real `uv` while it waits on a held cache lock, in a throwaway cache).

## Design documents

- Design: [`superpowers/specs/2026-11-06-maccleanup-design.md`](superpowers/specs/2026-11-06-maccleanup-design.md)
- Every page has its own design in [`superpowers/specs/`](superpowers/specs/) and its plan in [`superpowers/plans/`](superpowers/plans/) (Caches & Projects, Docker, Time Machine, Runtimes, Uninstall and Leftovers). Caches & Projects section 11 records what was built and what is deferred.

## Releasing

A release is an annotated version tag. Pushing `vX.Y.Z` runs `.github/workflows/release.yml`:

1. **verify**: the tag must equal the `Cargo.toml` version, point at a commit on `main` and have a `CHANGELOG.md` entry; then fmt, clippy, tests and the smoke script run again.
2. **build**: release binaries for `aarch64-apple-darwin` and `x86_64-apple-darwin` (the Intel one is cross-compiled on the Apple Silicon runner, so it is built but never run in CI), each as a `.tar.gz` with a `.sha256` file.
3. **publish**: `cargo publish` to crates.io. This job uses the `release` environment, so it can wait for your approval. A crates.io version can be yanked but never reused, which is why it comes after everything that can fail.
4. **release**: a GitHub release whose notes are the version's `CHANGELOG.md` section, with the binaries attached.

Each job needs the one before it, so a failure stops everything after it.

### The changelog

`CHANGELOG.md` follows Keep a Changelog. Add a line under **Unreleased** with every user-visible change (Added, Changed, Fixed, Removed). `cargo release` moves it under the new version heading with today's date. `scripts/release-notes.sh X.Y.Z` prints one version's section; the tag message and the GitHub release both come from it, so it must not be empty.

### Steps for a new version

[`cargo-release`](https://github.com/crate-ci/cargo-release) (`cargo install cargo-release`) does the version bump; `release.toml` turns off its publishing, pushing and tagging, so it only edits and commits.

```bash
# 1. preview, then bump: Cargo.toml, Cargo.lock, CHANGELOG.md and one commit
cargo release minor              # dry run (use patch, minor or major)
cargo release minor --execute
# 2. push the commit and wait for CI on main to go green
git push <remote> main
# 3. tag exactly that version; the message is the changelog
git tag -a vX.Y.Z -m "MacCleanup X.Y.Z" -m "$(scripts/release-notes.sh X.Y.Z)"
git push <remote> vX.Y.Z
# 4. approve the "publish" job on the Actions page (if the environment requires it)
```

`git show vX.Y.Z` shows the tag message. Check the preview before `--execute`: it lists the version change and the changelog replacement.

### One-time setup

- Repository secret `CARGO_REGISTRY_TOKEN`: a crates.io API token (scope `publish-update` for this crate is enough once it exists).
- Optional approval gate: Settings > Environments > New environment `release` > Required reviewers. Without reviewers the job runs straight away.

### When something fails

- **Before `publish`** (guard, tests or build): nothing is public. Fix it, then delete the tag and tag the fixed commit again:

  ```bash
  git tag -d vX.Y.Z
  git push <remote> --delete vX.Y.Z
  ```

- **After `publish`**: the version is used for good. For a bad release, bump to the next patch version and run the steps again; `cargo yank --version X.Y.Z` hides the bad one from new installs.
- **`release` failed after `publish`**: re-run only that job from the Actions page.
- A tag with a hyphen (`v0.11.0-rc.1`) is published as a pre-release; give it its own changelog section.
