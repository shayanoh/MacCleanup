use std::ffi::OsStr;
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
}
