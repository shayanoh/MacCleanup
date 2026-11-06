//! Finding every install of a program. A developer Mac often has several
//! Node versions side by side (fnm, nvm, volta), each with its own pnpm or
//! corepack shim, and `PATH` only shows the one the current shell picked.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::command::is_executable;

/// Every executable called `name`: first the folders of `path_var` in
/// order (`None` means the process's own `PATH`), then the version folders
/// of fnm, nvm and volta under `home`, then the usual standalone locations.
/// The same file reached by two routes (a shell's temporary link and the
/// real install) appears once, as the route found first.
pub fn resolve(home: &Path, name: &str, path_var: Option<&OsStr>) -> Vec<PathBuf> {
    let mut places = Places::from_env();
    if path_var.is_some() {
        places.path_var = path_var.map(OsStr::to_os_string);
    }
    resolve_in(home, name, &places)
}

/// Where to look, so tests can keep the real machine out of it.
pub(crate) struct Places {
    pub path_var: Option<std::ffi::OsString>,
    pub fnm_dir: Option<PathBuf>,
    pub nvm_dir: Option<PathBuf>,
    pub system: Vec<PathBuf>,
}

impl Places {
    /// This process's `PATH`, `FNM_DIR`, `NVM_DIR` and the usual system folders.
    pub fn from_env() -> Places {
        Places {
            path_var: std::env::var_os("PATH"),
            fnm_dir: std::env::var_os("FNM_DIR").map(PathBuf::from),
            nvm_dir: std::env::var_os("NVM_DIR").map(PathBuf::from),
            system: vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
        }
    }

    /// Nothing but the given `PATH` (and the version folders under `home`).
    #[cfg(test)]
    pub fn only(path_var: &OsStr) -> Places {
        Places {
            path_var: Some(path_var.to_os_string()),
            fnm_dir: None,
            nvm_dir: None,
            system: Vec::new(),
        }
    }
}

pub(crate) fn resolve_in(home: &Path, name: &str, places: &Places) -> Vec<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Vec::new();
    }
    let mut dirs: Vec<PathBuf> = places
        .path_var
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for base in manager_roots(home, places) {
        dirs.extend(version_bins(&base));
    }
    dirs.push(home.join(".volta/bin"));
    dirs.push(home.join("Library/pnpm"));
    dirs.extend(places.system.iter().cloned());

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if !is_executable(&candidate) {
            continue;
        }
        let key = fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen.insert(key) {
            out.push(candidate);
        }
    }
    out
}

/// `(<root>, kind)` folders that hold one sub-folder per Node version.
fn manager_roots(home: &Path, places: &Places) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = &places.fnm_dir {
        roots.push(dir.join("node-versions"));
    }
    roots.push(home.join(".local/share/fnm/node-versions"));
    roots.push(home.join("Library/Application Support/fnm/node-versions"));
    if let Some(dir) = &places.nvm_dir {
        roots.push(dir.join("versions/node"));
    }
    roots.push(home.join(".nvm/versions/node"));
    roots
}

/// The `bin` folder of every Node version under `root`, in version-name
/// order (fnm keeps it in `<version>/installation/bin`, nvm in `<version>/bin`).
fn version_bins(root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions
        .into_iter()
        .flat_map(|v| [v.join("installation/bin"), v.join("bin")])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn exe(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn find(home: &Path, name: &str, path: Option<&OsStr>) -> Vec<PathBuf> {
        resolve_in(home, name, &Places::only(path.unwrap_or(OsStr::new(""))))
    }

    fn names(found: &[PathBuf], base: &Path) -> Vec<String> {
        found
            .iter()
            .map(|p| p.strip_prefix(base).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_programs_on_path_in_order() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path();
        exe(&b.join("one/pnpm"));
        exe(&b.join("two/pnpm"));
        let path = std::env::join_paths([b.join("one"), b.join("two")]).unwrap();
        let found = find(&b.join("home"), "pnpm", Some(&path));
        assert_eq!(names(&found, b), ["one/pnpm", "two/pnpm"]);
    }

    #[test]
    fn finds_every_fnm_node_version() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let fnm = home.join(".local/share/fnm/node-versions");
        exe(&fnm.join("v20.1.0/installation/bin/pnpm"));
        exe(&fnm.join("v22.2.0/installation/bin/pnpm"));
        let found = find(&home, "pnpm", Some(OsStr::new("")));
        assert_eq!(
            names(&found, &home),
            [
                ".local/share/fnm/node-versions/v20.1.0/installation/bin/pnpm",
                ".local/share/fnm/node-versions/v22.2.0/installation/bin/pnpm",
            ]
        );
    }

    #[test]
    fn finds_every_nvm_node_version_and_the_macos_fnm_location() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        exe(&home.join(".nvm/versions/node/v18.0.0/bin/pnpm"));
        exe(&home
            .join("Library/Application Support/fnm/node-versions/v21.0.0/installation/bin/pnpm"));
        let found = find(&home, "pnpm", Some(OsStr::new("")));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn skips_directories_and_files_that_are_not_executable() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path();
        fs::create_dir_all(b.join("bin/pnpm")).unwrap();
        fs::create_dir_all(b.join("bin2")).unwrap();
        fs::write(b.join("bin2/pnpm"), "x").unwrap();
        let path = std::env::join_paths([b.join("bin"), b.join("bin2")]).unwrap();
        assert!(find(&b.join("home"), "pnpm", Some(&path)).is_empty());
    }

    #[test]
    fn the_same_binary_reached_two_ways_is_listed_once() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let real = home.join(".local/share/fnm/node-versions/v22.0.0/installation/bin/pnpm");
        exe(&real);
        // A shell's temporary multishell directory links to the same install.
        let shell = d.path().join("multishell/bin");
        fs::create_dir_all(&shell).unwrap();
        symlink(&real, shell.join("pnpm")).unwrap();
        let found = find(&home, "pnpm", Some(shell.as_os_str()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], shell.join("pnpm"), "the first way found is kept");
    }

    #[test]
    fn missing_folders_and_odd_names_are_not_errors() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("nope");
        assert!(find(&home, "pnpm", Some(OsStr::new(""))).is_empty());
        assert!(find(&home, "", Some(OsStr::new(""))).is_empty());
        assert!(find(&home, "../pnpm", Some(OsStr::new(""))).is_empty());
        assert!(find(&home, "a/b", Some(OsStr::new(""))).is_empty());
    }
}
