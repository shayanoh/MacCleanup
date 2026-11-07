//! What the Uninstall and Leftovers pages need, gathered once on a thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::appfiles::{AppPart, parts_of, wrappers};
use super::apppolicy::is_apple_id;
use super::apps::{AppBundle, build_index, running_executables};
use super::command::CommandRunner;
use super::leftovers::{LeftoversState, find_leftovers};

/// One installed app and the places that belong to it.
#[derive(Clone, Debug)]
pub struct AppGroup {
    pub app: AppBundle,
    pub parts: Vec<AppPart>,
}

#[derive(Clone, Debug)]
pub struct AppsInput {
    pub installed: Vec<AppGroup>,
    pub leftovers: LeftoversState,
    /// Executable paths of running processes.
    pub running: Vec<PathBuf>,
}

/// The folders that are searched for apps.
pub fn default_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications"),
        home.join("Applications"),
        PathBuf::from("/System/Applications"),
    ]
}

/// Read the apps, their folders and the leftovers. Slow (it asks Spotlight and
/// walks `/Applications`): call it on a thread.
pub fn discover_apps(
    home: &Path,
    lib: &Path,
    roots: &[PathBuf],
    runner: &dyn CommandRunner,
) -> AppsInput {
    let index = build_index(roots, runner);
    let running = running_executables(runner);
    let running_ids: BTreeSet<String> = index
        .apps
        .iter()
        .filter(|a| running.iter().any(|r| r.starts_with(&a.path)))
        .map(|a| a.id.clone())
        .collect();
    let absorbed: BTreeSet<PathBuf> = index
        .apps
        .iter()
        .flat_map(|a| wrappers(a, &index.apps).into_iter().map(|w| w.path.clone()))
        .collect();
    let installed = index
        .apps
        .iter()
        // macOS's own apps stay out of the list; wrapper bundles are shown
        // inside the app they belong to.
        .filter(|a| !a.path.starts_with("/System") && !absorbed.contains(&a.path))
        .filter(|a| !is_apple_id(&a.id) || a.path.starts_with("/Applications"))
        .map(|a| AppGroup {
            parts: parts_of(a, &index.apps, home, lib),
            app: a.clone(),
        })
        .collect();
    let leftovers = find_leftovers(&index, home, lib, &running_ids);
    AppsInput {
        installed,
        leftovers,
        running,
    }
}
