# Keys and mouse

Press `1` to `9` to open a page; `Tab` switches between the menu and the list.

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
| `h` / `l` (Caches & Projects, Runtimes, Uninstall, Leftovers, Docker) | Fold / open the group; `l` or `Enter` on a row shows it in the Explorer |
| `H` / `L` (same pages) | Fold every group / open every group (on Docker: every section and item; layer lists stay folded) |
| `m` (Caches & Projects) | Mark the selected folder and every folder that shares its data; press again to unmark them |
| `m` (Docker) | Mark the stopped containers the selected image or volume needs removed first; press again to unmark them |
| `b` (Docker) | Look inside the selected image or volume, read-only (`l` / `Enter` open a folder, `h` up, `Esc` back, `s` sorts, `/` filters) |
| `J` / `K` (Docker) | Scroll the details pane (the whole command line of a layer or build step) |
| `m` / `M` / `a` (Time Machine) | Measure the selected snapshot / every snapshot / mark all snapshots |
| `r` / `D` / `E` (ToBeDeleted screen) | Restore / delete for good / empty all |
| `q`, `Esc` | Quit (`Esc` first clears an active filter, then goes up a folder; quitting asks first if ToBeDeleted still holds items). While a tool command runs, `Esc` cancels it and `q` / `Ctrl-C` cancel it and quit |
| mouse | Click rows, menu entries, and treemap boxes or bars (a click on the graph selects that row in the list); scroll with the wheel; hover a graph box for a tooltip |

In the review dialog the list is one scrolling list: the folders first, then the tool commands at its end. It scrolls with `j`/`k`, the arrows, `PageUp`/`PageDown`, `Ctrl-F`/`Ctrl-B`, `Home`/`End` or the mouse wheel (a line under the list says which lines you are looking at, e.g. `lines 11-20 of 57`). Nothing is ever cut for a short terminal. When the list scrolls and contains commands, `y` stays off until you have scrolled to the end (`End` jumps there), so a command you never saw cannot run. `Enter` moves items to ToBeDeleted, `x` offers "move, then empty now" (irreversible), `f` switches a command that has a force form (`uv cache clean`) to `--force`, `Esc` cancels. Irreversible actions need an explicit `y`; `Enter` never confirms them. When the review contains tool commands, only `y` runs it.
