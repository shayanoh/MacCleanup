use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Rebuildable or disposable (caches, logs, build artifacts).
    Safe,
    /// Inside the home folder but needs a human decision.
    Caution,
    /// Never offered for deletion.
    Protected,
}

/// The hard-coded deny-list. Everything outside the home folder is protected;
/// inside it, sensitive folders are protected and only known-disposable
/// locations are `Safe`.
#[derive(Clone, Debug)]
pub struct Policy {
    home: PathBuf,
    holding: PathBuf,
}

const PROTECTED_TOP_LEVEL: &[&str] = &[
    "Library",
    "Documents",
    "Desktop",
    "Pictures",
    "Movies",
    "Music",
    "Applications",
    "Public",
];

/// `~/Library` folders that are protected themselves (but not their children).
const PROTECTED_LIBRARY_SELF: &[&str] = &[
    "Keychains",
    "Preferences",
    "Mobile Documents",
    "Application Support",
    "Containers",
    "Group Containers",
    "Mail",
    "Messages",
    "Safari",
    "Cookies",
    "Accounts",
    "Calendars",
    "Photos",
    "Metadata",
];

/// `~/Library` folders that are protected including everything inside.
const PROTECTED_LIBRARY_TREE: &[&str] = &[
    "Keychains",
    "Preferences",
    "Mobile Documents",
    "Mail",
    "Messages",
    "Safari",
    "Cookies",
    "Accounts",
    "Calendars",
];

const PROTECTED_SECRET_DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube"];
const PROTECTED_SUFFIXES: &[&str] = &[".photoslibrary", ".sparsebundle", ".sparseimage"];
const SAFE_DIR_NAMES: &[&str] = &["node_modules", ".next", ".turbo", "DerivedData"];

/// Canonicalize as much of `path` as exists, then append the rest unchanged.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    let mut tail: Vec<&OsStr> = Vec::new();
    let mut cur = path;
    loop {
        match cur.parent() {
            Some(parent) => {
                if let Some(name) = cur.file_name() {
                    tail.push(name);
                }
                if let Ok(mut base) = parent.canonicalize() {
                    for name in tail.iter().rev() {
                        base.push(name);
                    }
                    return base;
                }
                cur = parent;
            }
            None => return path.to_path_buf(),
        }
    }
}

fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

impl Policy {
    pub fn new(home: &Path, holding: &Path) -> Policy {
        Policy {
            home: canonicalize_lenient(home),
            holding: canonicalize_lenient(holding),
        }
    }

    /// Lexical classification of an absolute path (no filesystem access).
    pub fn classify(&self, path: &Path) -> Safety {
        if !path.is_absolute() || has_parent_dir(path) {
            return Safety::Protected;
        }
        // The holding folder, anything inside it, and anything containing it.
        if path.starts_with(&self.holding) || self.holding.starts_with(path) {
            return Safety::Protected;
        }
        let Ok(rel) = path.strip_prefix(&self.home) else {
            return Safety::Protected;
        };
        let comps: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if comps.is_empty() {
            return Safety::Protected; // home itself
        }
        if comps
            .iter()
            .any(|c| PROTECTED_SUFFIXES.iter().any(|s| c.ends_with(s)))
        {
            return Safety::Protected;
        }
        let first = comps[0].as_str();
        if comps.len() == 1 && PROTECTED_TOP_LEVEL.contains(&first) {
            return Safety::Protected;
        }
        if PROTECTED_SECRET_DIRS.contains(&first) {
            return Safety::Protected;
        }
        if first == ".config" && comps.get(1).map(String::as_str) == Some("gcloud") {
            return Safety::Protected;
        }
        if first == "Library" && comps.len() >= 2 {
            let second = comps[1].as_str();
            if comps.len() == 2 && PROTECTED_LIBRARY_SELF.contains(&second) {
                return Safety::Protected;
            }
            if PROTECTED_LIBRARY_TREE.contains(&second) {
                return Safety::Protected;
            }
        }

        // Known-disposable locations.
        if first == "Library" && comps.len() >= 3 && matches!(comps[1].as_str(), "Caches" | "Logs")
        {
            return Safety::Safe;
        }
        if first == ".cache" && comps.len() >= 2 {
            return Safety::Safe;
        }
        if first == ".npm" {
            return Safety::Safe;
        }
        if comps.iter().any(|c| SAFE_DIR_NAMES.contains(&c.as_str())) {
            return Safety::Safe;
        }
        Safety::Caution
    }

    /// Classify using the real location: the parent directory is resolved
    /// through symlinks, the final component is not (so a symlink is judged
    /// as the link itself, never as its target).
    pub fn classify_real(&self, path: &Path) -> Safety {
        if !path.is_absolute() || has_parent_dir(path) {
            return Safety::Protected;
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Safety::Protected;
        };
        match parent.canonicalize() {
            Ok(real_parent) => self.classify(&real_parent.join(name)),
            Err(_) => Safety::Protected,
        }
    }

    /// If `path` is a folder that contains something on the deny-list (for
    /// example `~/.config` holding `gcloud`), return that protected path.
    /// Moving or deleting such a folder would take the protected item with it.
    /// Known-disposable (`Safe`) folders are not walked, and symlinks inside
    /// are never followed.
    pub fn protected_descendant(&self, path: &Path) -> Option<PathBuf> {
        let (parent, name) = (path.parent()?, path.file_name()?);
        let real = parent.canonicalize().ok()?.join(name);
        if self.classify(&real) == Safety::Safe {
            return None;
        }
        if !fs::symlink_metadata(&real).ok()?.is_dir() {
            return None;
        }
        let mut stack = vec![real];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let child = entry.path();
                if self.classify(&child) == Safety::Protected {
                    return Some(child);
                }
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(child);
                }
            }
        }
        None
    }

    pub fn can_mark(&self, path: &Path) -> bool {
        self.classify_real(path) != Safety::Protected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct Sandbox {
        _dir: TempDir,
        home: PathBuf,
        holding: PathBuf,
        policy: Policy,
    }

    fn sandbox() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        fs::create_dir_all(&home).unwrap();
        let holding = home.join("ToBeDeleted");
        fs::create_dir_all(&holding).unwrap();
        let policy = Policy::new(&home, &holding);
        Sandbox {
            _dir: dir,
            home,
            holding,
            policy,
        }
    }

    #[test]
    fn outside_home_is_protected() {
        let s = sandbox();
        let sibling = s.home.parent().unwrap().join("sibling");
        for p in [
            PathBuf::from("/System/Library"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/"),
            sibling,
        ] {
            assert_eq!(s.policy.classify(&p), Safety::Protected, "{p:?}");
        }
    }

    #[test]
    fn home_and_top_level_user_folders_are_protected() {
        let s = sandbox();
        for name in ["", "Library", "Documents", "Desktop", "Pictures", "Movies"] {
            let p = s.home.join(name);
            assert_eq!(s.policy.classify(&p), Safety::Protected, "{p:?}");
        }
        assert_eq!(
            s.policy.classify(&s.home.join("Documents/notes.txt")),
            Safety::Caution
        );
    }

    #[test]
    fn library_sensitive_dirs_protected() {
        let s = sandbox();
        let lib = s.home.join("Library");
        assert_eq!(
            s.policy.classify(&lib.join("Keychains/login.keychain-db")),
            Safety::Protected
        );
        assert_eq!(s.policy.classify(&lib.join("Mail/x")), Safety::Protected);
        assert_eq!(
            s.policy.classify(&lib.join("Application Support")),
            Safety::Protected
        );
        assert_eq!(
            s.policy.classify(&lib.join("Application Support/Foo")),
            Safety::Caution
        );
    }

    #[test]
    fn secrets_protected() {
        let s = sandbox();
        assert_eq!(
            s.policy.classify(&s.home.join(".ssh/id_rsa")),
            Safety::Protected
        );
        assert_eq!(
            s.policy.classify(&s.home.join(".config/gcloud/creds")),
            Safety::Protected
        );
    }

    #[test]
    fn vault_images_protected() {
        let s = sandbox();
        for rel in [
            "Pictures/Photos Library.photoslibrary",
            "Pictures/Photos Library.photoslibrary/database/x",
            "x/Vault.sparseimage",
            "x/Library.sparsebundle",
        ] {
            let p = s.home.join(rel);
            assert_eq!(s.policy.classify(&p), Safety::Protected, "{p:?}");
        }
    }

    #[test]
    fn known_caches_are_safe() {
        let s = sandbox();
        for rel in [
            "Library/Caches/com.foo",
            "Library/Logs/x.log",
            ".cache/uv",
            ".npm",
            "proj/node_modules",
            "proj/.next",
            "proj/.turbo",
            "proj/DerivedData",
        ] {
            let p = s.home.join(rel);
            assert_eq!(s.policy.classify(&p), Safety::Safe, "{p:?}");
        }
    }

    #[test]
    fn holding_folder_protected() {
        let s = sandbox();
        assert_eq!(s.policy.classify(&s.holding), Safety::Protected);
        assert_eq!(
            s.policy.classify(&s.holding.join("items/1/a")),
            Safety::Protected
        );
    }

    #[test]
    fn ancestor_of_holding_is_protected() {
        let s = sandbox();
        let holding = s.home.join("work/hold");
        fs::create_dir_all(&holding).unwrap();
        let policy = Policy::new(&s.home, &holding);
        assert_eq!(policy.classify(&s.home.join("work")), Safety::Protected);
        assert_eq!(policy.classify(&s.home.join("other")), Safety::Caution);
    }

    #[test]
    fn relative_and_dotdot_paths_protected() {
        let s = sandbox();
        assert_eq!(s.policy.classify(&PathBuf::from("foo")), Safety::Protected);
        assert_eq!(s.policy.classify(&s.home.join("../x")), Safety::Protected);
        assert_eq!(
            s.policy.classify_real(&PathBuf::from("foo")),
            Safety::Protected
        );
        assert!(!s.policy.can_mark(&PathBuf::from("foo")));
    }

    #[test]
    fn similar_prefix_is_not_a_match() {
        let s = sandbox();
        // "Library2" must not be treated as "Library".
        assert_eq!(
            s.policy.classify(&s.home.join("Library2/x")),
            Safety::Caution
        );
    }

    #[test]
    fn symlink_into_protected_parent_refused() {
        let s = sandbox();
        let caches = s.home.join("Library/Caches");
        fs::create_dir_all(&caches).unwrap();
        fs::create_dir_all(s.home.join("Documents")).unwrap();
        symlink(s.home.join("Documents"), caches.join("link")).unwrap();

        // The link itself is judged as a link inside Caches.
        assert_eq!(s.policy.classify_real(&caches.join("link")), Safety::Safe);
        // Going through the link lands in Documents, which is not Safe.
        assert_eq!(
            s.policy.classify_real(&caches.join("link/notes.txt")),
            Safety::Caution
        );

        // A symlink that escapes home is protected.
        let outside = s.home.parent().unwrap().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(s.home.join("proj")).unwrap();
        symlink(&outside, s.home.join("proj/escape")).unwrap();
        assert_eq!(
            s.policy.classify_real(&s.home.join("proj/escape/file")),
            Safety::Protected
        );
        assert!(!s.policy.can_mark(&s.home.join("proj/escape/file")));
    }

    #[test]
    fn protected_descendant_found_in_caution_folder() {
        let s = sandbox();
        let creds = s.home.join(".config/gcloud/creds");
        fs::create_dir_all(creds.parent().unwrap()).unwrap();
        fs::write(&creds, "secret").unwrap();
        assert_eq!(
            s.policy.protected_descendant(&s.home.join(".config")),
            Some(s.home.join(".config/gcloud"))
        );
    }

    #[test]
    fn vault_inside_a_project_is_found() {
        let s = sandbox();
        let vault = s.home.join("proj/Photos.photoslibrary");
        fs::create_dir_all(vault.join("database")).unwrap();
        assert_eq!(
            s.policy.protected_descendant(&s.home.join("proj")),
            Some(vault)
        );
    }

    #[test]
    fn plain_folder_has_no_protected_descendant() {
        let s = sandbox();
        fs::create_dir_all(s.home.join("proj/a/b")).unwrap();
        fs::write(s.home.join("proj/a/b/c.txt"), "x").unwrap();
        assert_eq!(s.policy.protected_descendant(&s.home.join("proj")), None);
        // A plain file has no descendants.
        assert_eq!(
            s.policy
                .protected_descendant(&s.home.join("proj/a/b/c.txt")),
            None
        );
    }

    #[test]
    fn safe_folders_skip_the_walk() {
        let s = sandbox();
        let nested = s.home.join("proj/node_modules/pkg/.ssh");
        fs::create_dir_all(&nested).unwrap();
        // node_modules is known-disposable (Safe); the walk is skipped.
        assert_eq!(
            s.policy
                .protected_descendant(&s.home.join("proj/node_modules")),
            None
        );
    }

    #[test]
    fn symlinks_inside_are_not_followed() {
        let s = sandbox();
        fs::create_dir_all(s.home.join(".ssh")).unwrap();
        fs::create_dir_all(s.home.join("proj")).unwrap();
        symlink(s.home.join(".ssh"), s.home.join("proj/link")).unwrap();
        assert_eq!(s.policy.protected_descendant(&s.home.join("proj")), None);
    }

    #[test]
    fn missing_path_has_no_protected_descendant() {
        let s = sandbox();
        assert_eq!(s.policy.protected_descendant(&s.home.join("nope")), None);
    }
}
