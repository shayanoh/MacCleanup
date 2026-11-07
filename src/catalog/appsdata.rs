//! What the Uninstall and Leftovers pages need, gathered once on a thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::appfiles::{AppPart, parts_of, wrappers};
use super::appmgr::{ManagedApp, Managers, detect};
use super::apppolicy::is_apple_id;
use super::apps::{AppBundle, build_index_with, running_executables};
use super::command::CommandRunner;
use super::leftovers::{LeftoversState, find_leftovers_with};
use crate::bg::Work;

/// One installed app and the places that belong to it.
#[derive(Clone, Debug)]
pub struct AppGroup {
    pub app: AppBundle,
    pub parts: Vec<AppPart>,
    /// Installed by a package manager: removed with its own command.
    pub managed: Option<ManagedApp>,
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
    managers: &Managers,
    runner: &dyn CommandRunner,
    work: &Work,
) -> AppsInput {
    let index = build_index_with(roots, runner, work);
    work.begin("checking which programs are running", 0);
    let running = running_executables(runner);
    let absorbed: BTreeSet<PathBuf> = index
        .apps
        .iter()
        .flat_map(|a| wrappers(a, &index.apps).into_iter().map(|w| w.path.clone()))
        .collect();
    let listed: Vec<&AppBundle> = index
        .apps
        .iter()
        // macOS's own apps stay out of the list; wrapper bundles are shown
        // inside the app they belong to.
        .filter(|a| !a.path.starts_with("/System") && !absorbed.contains(&a.path))
        .filter(|a| !is_apple_id(&a.id) || a.path.starts_with("/Applications"))
        .collect();
    let total = listed.len();
    let installed: Vec<AppGroup> = listed
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            work.stage(i, total, &format!("finding the folders of {}", a.name));
            AppGroup {
                parts: parts_of(a, &index.apps, home, lib),
                managed: detect(a, home, managers, runner),
                app: a.clone(),
            }
        })
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let leftovers = find_leftovers_with(&index, home, lib, &running, now, work);
    AppsInput {
        installed,
        leftovers,
        running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::SystemRunner;

    /// Read-only report on this machine: `cargo test real_machine_report --
    /// --ignored --nocapture`. Prints what the Uninstall and Leftovers pages
    /// would list; changes nothing.
    #[test]
    #[ignore = "reads the real /Applications, ~/Library and Spotlight"]
    fn real_machine_report() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let input = discover_apps(
            &home,
            Path::new("/Library"),
            &default_roots(&home),
            &Managers::system(),
            &SystemRunner,
            &Work::new(),
        );
        println!("{} installed apps", input.installed.len());
        let filter = std::env::var("APP").unwrap_or_else(|_| "Fusion".into());
        for g in input
            .installed
            .iter()
            .filter(|g| g.app.name.contains(&filter))
        {
            println!(
                "== {} ({}) {:?}",
                g.app.name,
                g.app.id,
                g.managed.as_ref().map(|m| &m.id)
            );
            for p in &g.parts {
                println!(
                    "   {:<18} shared={} outside={}  {}",
                    p.label,
                    p.shared_vendor,
                    p.outside_home,
                    p.path.display()
                );
            }
        }
        match &input.leftovers {
            LeftoversState::Ready(found) => {
                println!("{} leftover groups", found.len());
                for g in found {
                    println!("== {} ({} parts)", g.id, g.parts.len());
                    for p in g.parts.iter().take(4) {
                        println!("   {:<18} {}", p.label, p.path.display());
                    }
                }
            }
            LeftoversState::Unavailable(w) => println!("leftovers unavailable: {w}"),
        }
    }
}
