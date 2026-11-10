# MacCleanup

[![CI](https://github.com/shayanoh/MacCleanup/actions/workflows/ci.yml/badge.svg)](https://github.com/shayanoh/MacCleanup/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/maccleanup.svg)](https://crates.io/crates/maccleanup)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A colorful, animated terminal app for macOS that shows where your disk space went and helps you free it **safely**. Open it, see your free space, clean up, and finish with a summary of exactly how much you got back.

Disk numbers on a Mac are misleading: APFS clones (what pnpm uses for `node_modules`) look full-size but cost nothing, and Time Machine snapshots keep deleted files alive. MacCleanup works out what deleting something will *really* free, and shows it before you commit.

## What it does

- **Explore your disk.** A fast multithreaded scan of your home folder (or any folder) with live sizes, share bars, a treemap and bar graph, fzf-style filtering, and sorting by size, real on-disk size, what a delete would free, or last modified. Keyboard (vim-style) and mouse.
- **Clean caches and old project output.** One list of things that come back by themselves: `node_modules`, Rust `target`, Xcode DerivedData, Flutter builds, and the caches of pnpm, npm, Yarn, Go, uv, pip, Homebrew, Cargo and more. Built programs and bundles are kept unless you pick them yourself. Tool caches are cleaned with the tool's own command.
- **Clean up Docker.** Images, containers, volumes and build cache with exact shared/unique sizes, a look inside images and volumes, and a trim that gives the space back to your Mac.
- **Delete Time Machine local snapshots** and see about how much each one holds.
- **Remove old runtimes.** Node versions (fnm, nvm), global npm/pnpm packages, Corepack, uv Pythons and tools, Rust toolchains, cargo binaries, Android SDK parts and iOS simulators, flagged when they are the default or a project asks for them.
- **Uninstall apps completely**, with the caches, preferences and support folders they leave behind, and find **leftovers** of apps that are already gone.
- **Honest results.** When you quit, it prints how much space you truly freed: the volume's free space now against when you started.

## Safe by design

- Nothing is deleted without a review screen listing every item and its size.
- Deleting *moves* items to `~/ToBeDeleted` first. You can restore them, or empty the folder when you are sure. Moved data counts as "pending", never "freed".
- Irreversible actions (permanent delete, Docker removals, tool commands) need an explicit `y`; `Enter` never confirms them.
- System paths, keychains, mail, `.ssh`, Photos libraries and more are protected and cannot be marked.
- Your password is never asked for, read or stored. The only admin step, removing root-owned app files, uses macOS's own prompt.

Details: [docs/safety.md](docs/safety.md).

## Install

**Requirements:** macOS (installing on Linux or Windows stops with an error) and a recent [Rust toolchain](https://rustup.rs) (1.88 or newer). Docker, Time Machine and the other tools are only needed for their own pages.

### From crates.io

```bash
cargo install maccleanup
```

### From source

```bash
git clone https://github.com/shayanoh/MacCleanup.git
cd MacCleanup
cargo install --path .
```

Either way the `maccleanup` command lands in `~/.cargo/bin`, which `rustup` normally puts on your `PATH`.

## Use it

```bash
maccleanup                  # scan your home folder
maccleanup ~/Developer      # explore another folder
maccleanup --help           # all options
```

| Option | What it does |
|---|---|
| `[PATH]` | Folder to explore (default: your home folder) |
| `--holding-dir <DIR>` | Where soft-deleted items go (default: `~/ToBeDeleted`) |
| `--ascii` | ASCII-only characters, for terminals without Nerd Font / Unicode support |
| `--no-animation` | Turn off animations |

Move with `j`/`k` and `h`/`l`, mark with `space`, review and delete with `d`, filter with `/`, switch pages with `1` to `9`, quit with `q`. All keys: [docs/keys.md](docs/keys.md).

**Tips**

- The look assumes a [Nerd Font](https://www.nerdfonts.com/) in your terminal; use `--ascii` if you have none.
- Scanning some folders needs **Full Disk Access** for your terminal app (System Settings > Privacy & Security > Full Disk Access). The app tells you when it is missing and keeps working with what it can read.
- Deleting works inside your home folder, plus the app files the Uninstall and Leftovers pages find. Everything else is protected.

## Documentation

- [Features in detail](docs/features.md): every page, column and rule
- [Keys and mouse](docs/keys.md)
- [Safety](docs/safety.md)
- [Known limits and roadmap](docs/known-limits.md)
- [Development](docs/development.md): build, test, design documents

## License

[MIT](LICENSE)
