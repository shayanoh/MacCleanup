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
