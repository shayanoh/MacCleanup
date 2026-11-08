//! Which paths outside the home folder the Uninstall and Leftovers pages may
//! offer. Everything else outside home stays protected (see `safety.rs`).

use std::path::{Component, Path};

use crate::safety::Safety;

/// Apple's own apps that people copy into `/Applications` (matched by name;
/// bundle ids starting with `com.apple.` are matched by [`is_apple_id`]).
const APPLE_APP_NAMES: &[&str] = &[
    "Safari",
    "Xcode",
    "GarageBand",
    "Final Cut Pro",
    "Logic Pro",
    "Keynote",
    "Numbers",
    "Pages",
    "iMovie",
    "TextEdit",
    "Mail",
    "Music",
    "TV",
    "Photos",
    "Preview",
    "Notes",
    "Terminal",
    "Utilities",
];

/// `/Library` folders whose direct children belong to apps.
const LIBRARY_APP_DIRS: &[&str] = &[
    "Application Support",
    "LaunchAgents",
    "LaunchDaemons",
    "PrivilegedHelperTools",
    "Caches",
    "Preferences",
    "Logs",
];

/// Roots that are never offered, whatever else matches.
const SYSTEM_ROOTS: &[&str] = &["System", "usr", "bin", "sbin", "etc", "private", "var"];

/// Apple's own ids that do not start with `com.apple.` (Shortcuts' data,
/// macOS printing).
const APPLE_LEGACY_PREFIXES: &[&str] = &["is.workflow.", "org.cups.", "com.apple."];

pub fn is_apple_id(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let id = id
        .strip_prefix("group.")
        .or_else(|| id.strip_prefix("groups."))
        .unwrap_or(&id);
    APPLE_LEGACY_PREFIXES.iter().any(|p| id.starts_with(p))
        || id.contains(".com.apple.")
        || id.ends_with(".com.apple")
}

fn is_apple_name(name: &str) -> bool {
    let stem = name.strip_suffix(".app").unwrap_or(name);
    APPLE_APP_NAMES.contains(&stem) || is_apple_id(name)
}

/// Is this path Apple's or the system's (by name, not by reading anything)?
pub fn is_apple_path(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let Some(first) = names.first() else {
        return true;
    };
    if SYSTEM_ROOTS.contains(&first.as_str()) {
        return true;
    }
    if first == "Library" && names.get(1).is_some_and(|n| n.starts_with("Apple")) {
        return true;
    }
    names.iter().skip(1).any(|n| is_apple_name(n))
}

/// What this feature may do with an absolute path outside the home folder:
/// `Some(Caution)` when it may be offered, `Some(Protected)` when it is
/// Apple's or the system's, `None` when the old rules (protected) apply.
pub fn app_location(path: &Path) -> Option<Safety> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    if is_apple_path(path) {
        return Some(Safety::Protected);
    }
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let is_app = |n: &str| n.ends_with(".app");
    match names.as_slice() {
        [a, app] if a == "Applications" && is_app(app) => Some(Safety::Caution),
        [a, sub, app] if a == "Applications" && !is_app(sub) && is_app(app) => {
            Some(Safety::Caution)
        }
        [l, dir, _child] if l == "Library" && LIBRARY_APP_DIRS.contains(&dir.as_str()) => {
            Some(Safety::Caution)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn app_in_applications_is_caution() {
        assert_eq!(
            app_location(Path::new("/Applications/Foo.app")),
            Some(Safety::Caution)
        );
        assert_eq!(
            app_location(Path::new("/Applications/Chrome Apps.localized/Foo.app")),
            Some(Safety::Caution)
        );
    }

    #[test]
    fn apple_app_in_applications_is_protected() {
        for p in [
            "/Applications/Safari.app",
            "/Applications/Xcode.app",
            "/Applications/GarageBand.app",
            "/Applications/Utilities/Terminal.app",
        ] {
            assert_eq!(app_location(Path::new(p)), Some(Safety::Protected), "{p}");
        }
    }

    #[test]
    fn system_paths_are_never_caution() {
        for p in [
            "/System/Applications/Mail.app",
            "/Library/Apple/x",
            "/usr/bin/ls",
        ] {
            assert_ne!(app_location(Path::new(p)), Some(Safety::Caution), "{p}");
        }
    }

    #[test]
    fn library_child_is_caution() {
        assert_eq!(
            app_location(Path::new("/Library/LaunchDaemons/com.docker.socket.plist")),
            Some(Safety::Caution)
        );
        assert_eq!(
            app_location(Path::new("/Library/LaunchDaemons/com.apple.x.plist")),
            Some(Safety::Protected)
        );
    }

    #[test]
    fn library_root_is_none() {
        assert_eq!(app_location(Path::new("/Library")), None);
        assert_eq!(app_location(Path::new("/Library/LaunchDaemons")), None);
        assert_eq!(app_location(Path::new("/Applications")), None);
    }

    #[test]
    fn is_apple_id_cases() {
        assert!(is_apple_id("com.apple.Safari"));
        assert!(is_apple_id("COM.APPLE.x"));
        assert!(!is_apple_id("com.applesauce.x"));
        assert!(is_apple_id("243LU875E5.groups.com.apple.podcasts"));
        assert!(is_apple_id("group.is.workflow.shortcuts"));
        assert!(is_apple_id("org.cups.PrintingPrefs"));
    }

    #[test]
    fn parent_dir_and_relative_paths_are_refused() {
        assert_eq!(
            app_location(Path::new("/Applications/Foo.app/../Safari.app")),
            None
        );
        assert_eq!(app_location(Path::new("Applications/Foo.app")), None);
    }
}
