# Packages & runtimes page (milestone 5, after Time Machine)

Status: planned, not designed in detail. Written 2026-11-07 from the user's
request; to be brainstormed again before it is built.

## Why

Old Node versions, global npm and pnpm packages, `cargo install` binaries and
Homebrew packages pile up and are invisible in the Caches & Projects page,
which is about things that come back by themselves. These are installs, so
removing them is a different, more careful action.

## What the page lists

1. **Node installs** from fnm (`~/Library/Application Support/fnm/node-versions`),
   nvm (`~/.nvm/versions/node`) and volta, each with its size, which one is the
   default or current, and which projects pin it (`.nvmrc`, `.node-version`,
   `package.json` `engines`) when cheap to know. Removal by the manager's own
   command: `fnm uninstall <v>`, `nvm uninstall <v>` (a shell function: run via
   the shell), volta's `volta uninstall`. The resolve layer
   (`catalog/resolve.rs`) already finds every install.
2. **Global packages per Node version**: the `lib/node_modules` of each install
   (npm), and pnpm's global dir (`pnpm root -g`, one per pnpm major), with sizes.
   Removal: `npm uninstall -g <name>` / `pnpm remove -g <name>` with the matching
   Node first on `PATH` (the same pinning rule as tool commands).
3. **Cargo**: binaries from `~/.cargo/bin` using `~/.cargo/.crates.toml` /
   `.crates2.json` for names and versions; sizes of the binaries. Removal:
   `cargo uninstall <name>`. Registry and git caches are already Caches rows.
4. **Homebrew**: formulae and casks with sizes (`brew info --json=v2 --installed`
   gives install paths); "leaves" (`brew leaves`) marked as top-level. Removal:
   `brew uninstall <name>` only for formulae nothing depends on (`brew uses
   --installed`); the page says what would break otherwise.
5. Other managers later (pip `--user`, gem, uv tools, pipx) if a size can be
   measured.

## Rules (inherited)

- Items go into the shared selection (`tool_marks`, new id prefixes
  `node:`, `npm-global:`, `pnpm-global:`, `cargo:`, `brew:`) and the one review;
  irreversible, `y` only, never `--force`, each command pinned to one exact
  install, run through `CommandRunner` with a process group and cancel.
- Never remove the Node install that is the current default, or one a running
  shell uses, without a clear warning; refusals live in a `removable` check like
  Docker's `ItemFacts::removable`.
- Sizes are measured on a thread; clone-aware like the rest (`Scanner::space`).
- Planned sidebar entry already exists dimmed; give it the next digit.

## Open questions for the brainstorm

- One page with sections, or one page per manager?
- How to mark "unused" Node versions (no project pins them, not the default)
  without scanning every project: use the Caches & Projects project list.
- Whether Homebrew removals should run `brew autoremove` afterwards.
