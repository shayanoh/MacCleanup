# MacCleanup

A rust project with beautiful and colorful TUI to find and report disk usage on the mac, and cleanup the extra files. Features are roughly like this:

- A main menu giving the options.
- A disk explorer, that scans in multiple threads a given folder (default ~), and shows a file explorer displaying each file and folders size, and real disk size, which you can browse into and select files for deletion (space selects/deselects, support vim style navigation, d to mark for deletion). Deleting is a final step which lists all files that are going to be deleted, their sizes, and shows a progress-bar while deleting.
- Look for remnants of deleted mac appliciations, group them by application and show their size (real/on disk), allow deleting by whole app, and selecting specific file/folders inside app
- App uninstaller (same as previous line, but show all the apps installed, their total sizes [with split info by app size, data size, ...]) and allow deleting entire app
- Cache deletion (searches for caches, groups by app/type, should include caches for go, rust, npm, pnpm, flutter, docker, node, python, uv, ... and anything else that is useful.)
- Cleanup for some caches like docker and pnpm will need to go through their own commands if possible. Multiple pnpm and npm and node installations might be there using fnm or other managers.
- On each page a togglable graph could show the size distribution, that highlights and follows the selection.
- Mouse support is a plus.
- Other cleanup ideas are welcome.

