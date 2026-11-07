//! The folders and files that belong to one installed app.

use std::fs;
use std::path::{Path, PathBuf};

use super::apps::{AppBundle, vendor_of};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPart {
    pub path: PathBuf,
    /// What it is: `app`, `payload`, `Caches`, ...
    pub label: String,
    /// A vendor folder that other installed apps of the vendor use too.
    pub shared_vendor: bool,
    /// Lives outside the home folder (likely needs the password to move).
    pub outside_home: bool,
}

/// Vendors whose ids span unrelated apps: their vendor-wide folders are never
/// attached to one app (their per-app folders still match by full id).
pub const BIG_VENDORS: &[&str] = &[
    "com.google",
    "com.microsoft",
    "com.adobe",
    "org.mozilla",
    "com.jetbrains",
];

/// `UBF8T346G9.com.microsoft.teams` → `com.microsoft.teams`: drops a leading
/// 10-character team id.
pub fn strip_team_id(name: &str) -> &str {
    match name.split_once('.') {
        Some((team, rest))
            if team.len() == 10
                && team
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
        {
            rest
        }
        _ => name,
    }
}

/// The folder a launcher script runs the real app from (`destfolder="..."`),
/// when it is inside `~/Library`. The script is read as text, never run.
pub fn launcher_target(app: &AppBundle, home_lib: &Path) -> Option<PathBuf> {
    use std::io::Read;
    let mut text = String::new();
    fs::File::open(&app.executable)
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut text)
        .ok()?;
    let line = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("destfolder=")?;
        Some(rest.trim_matches('"').trim_matches('\'').to_string())
    })?;
    let p = PathBuf::from(line);
    (p.is_absolute() && p.starts_with(home_lib) && !p.components().any(|c| c.as_os_str() == ".."))
        .then_some(p)
}

/// How many leading dot-separated parts two ids share.
fn shared_segments(a: &str, b: &str) -> usize {
    a.split('.')
        .zip(b.split('.'))
        .take_while(|(x, y)| x.eq_ignore_ascii_case(y))
        .count()
}

/// Sibling bundles in the same folder that belong to this app (Fusion's
/// "Service Utility" and "Remove ..." bundles): their ids share at least four
/// leading parts with the app's, and their longer names contain its name.
/// (`com.google.Chrome` and `com.google.Chrome.canary` share three: separate
/// apps.)
pub fn wrappers<'a>(app: &AppBundle, siblings: &'a [AppBundle]) -> Vec<&'a AppBundle> {
    let n = app.name.to_lowercase();
    siblings
        .iter()
        .filter(|s| s.path != app.path && s.path.parent() == app.path.parent())
        .filter(|s| shared_segments(&s.id, &app.id) >= 4)
        .filter(|s| {
            let sn = s.name.to_lowercase();
            sn.len() > n.len() && sn.contains(&n)
        })
        .collect()
}

fn push_if_exists(out: &mut Vec<AppPart>, path: PathBuf, label: &str, outside_home: bool) {
    if fs::symlink_metadata(&path).is_ok() && !out.iter().any(|p| p.path == path) {
        out.push(AppPart {
            path,
            label: label.to_string(),
            shared_vendor: false,
            outside_home,
        });
    }
}

/// Entries of `dir` whose name starts with `prefix`.
fn starting_with(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Everything that belongs to `app`: its bundle, wrapper bundles, the
/// per-app folders in `~/Library` and `/Library` (`lib`), a launcher
/// script's payload, and the vendor folder.
pub fn parts_of(app: &AppBundle, siblings: &[AppBundle], home: &Path, lib: &Path) -> Vec<AppPart> {
    let hl = home.join("Library");
    let (id, name) = (app.id.as_str(), app.name.as_str());
    let mut out = Vec::new();
    push_if_exists(
        &mut out,
        app.path.clone(),
        "app",
        !app.path.starts_with(home),
    );
    for w in wrappers(app, siblings) {
        push_if_exists(&mut out, w.path.clone(), "app", !w.path.starts_with(home));
    }
    if let Some(p) = launcher_target(app, &hl) {
        push_if_exists(&mut out, p, "payload", false);
    }
    for dir in [
        "Caches",
        "Containers",
        "HTTPStorages",
        "WebKit",
        "Application Scripts",
    ] {
        push_if_exists(&mut out, hl.join(dir).join(id), dir, false);
    }
    for n in [id, name] {
        push_if_exists(
            &mut out,
            hl.join("Application Support").join(n),
            "Application Support",
            false,
        );
    }
    push_if_exists(&mut out, hl.join("Logs").join(name), "Logs", false);
    push_if_exists(
        &mut out,
        hl.join("Preferences").join(format!("{id}.plist")),
        "Preferences",
        false,
    );
    for p in starting_with(&hl.join("Preferences/ByHost"), &format!("{id}.")) {
        push_if_exists(&mut out, p, "Preferences", false);
    }
    push_if_exists(
        &mut out,
        hl.join("Saved Application State")
            .join(format!("{id}.savedState")),
        "Saved State",
        false,
    );
    push_if_exists(
        &mut out,
        hl.join("Cookies").join(format!("{id}.binarycookies")),
        "Cookies",
        false,
    );
    for p in starting_with(&hl.join("LaunchAgents"), &format!("{id}.")) {
        push_if_exists(&mut out, p, "LaunchAgents", false);
    }
    if let Ok(rd) = fs::read_dir(hl.join("Group Containers")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let bare = strip_team_id(&n);
            if bare == id || bare == format!("group.{id}") {
                push_if_exists(&mut out, e.path(), "Group Containers", false);
            }
        }
    }
    // /Library: needs the password to change.
    for n in [id, name] {
        push_if_exists(
            &mut out,
            lib.join("Application Support").join(n),
            "Application Support",
            true,
        );
    }
    push_if_exists(&mut out, lib.join("Caches").join(id), "Caches", true);
    push_if_exists(
        &mut out,
        lib.join("Preferences").join(format!("{id}.plist")),
        "Preferences",
        true,
    );
    for dir in ["LaunchAgents", "LaunchDaemons", "PrivilegedHelperTools"] {
        for p in starting_with(&lib.join(dir), &format!("{id}."))
            .into_iter()
            .chain(
                starting_with(&lib.join(dir), id)
                    .into_iter()
                    .filter(|p| p.file_name().is_some_and(|f| f == id)),
            )
        {
            push_if_exists(&mut out, p, dir, true);
        }
    }
    // The vendor folder (`Application Support/Autodesk`).
    let vendor = vendor_of(id);
    if !BIG_VENDORS.contains(&vendor.as_str())
        && let Some(vname) = vendor.split('.').nth(1)
    {
        let shared = siblings.iter().any(|s| {
            s.path != app.path
                && vendor_of(&s.id) == vendor
                && !wrappers(app, siblings).iter().any(|w| w.path == s.path)
        });
        if let Ok(rd) = fs::read_dir(hl.join("Application Support")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_lowercase() == vname
                    && !out.iter().any(|p| p.path == e.path())
                {
                    out.push(AppPart {
                        path: e.path(),
                        label: "Vendor folder".to_string(),
                        shared_vendor: shared,
                        outside_home: false,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(path: &Path, id: &str, name: &str) -> AppBundle {
        AppBundle {
            path: path.to_path_buf(),
            id: id.to_string(),
            name: name.to_string(),
            version: String::new(),
            executable: path.join("Contents/MacOS").join(name),
            owner: "u".into(),
            last_used: 0,
        }
    }

    struct Env {
        _d: tempfile::TempDir,
        home: PathBuf,
        lib: PathBuf,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap().join("home");
        let lib = d.path().canonicalize().unwrap().join("lib");
        fs::create_dir_all(home.join("Library")).unwrap();
        fs::create_dir_all(&lib).unwrap();
        Env { _d: d, home, lib }
    }

    fn mk(p: &Path) {
        fs::create_dir_all(p).unwrap();
    }

    fn labels(parts: &[AppPart]) -> Vec<String> {
        parts
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_standard_library_folders() {
        let e = env();
        let hl = e.home.join("Library");
        let a = app(&e.home.join("Applications/Foo.app"), "com.foo.app", "Foo");
        mk(&a.path);
        for d in [
            "Caches/com.foo.app",
            "Containers/com.foo.app",
            "HTTPStorages/com.foo.app",
            "Application Support/Foo",
        ] {
            mk(&hl.join(d));
        }
        for f in [
            "Preferences/com.foo.app.plist",
            "Saved Application State/com.foo.app.savedState",
        ] {
            mk(hl.join(f).parent().unwrap());
            fs::write(hl.join(f), b"x").unwrap();
        }
        let n = labels(&parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib));
        for want in [
            "Foo.app",
            "com.foo.app",
            "Foo",
            "com.foo.app.plist",
            "com.foo.app.savedState",
        ] {
            assert!(n.iter().any(|x| x == want), "{want} missing in {n:?}");
        }
    }

    #[test]
    fn strip_team_id_table() {
        assert_eq!(
            strip_team_id("UBF8T346G9.com.microsoft.teams"),
            "com.microsoft.teams"
        );
        assert_eq!(strip_team_id("com.foo.app"), "com.foo.app");
        assert_eq!(strip_team_id("group.com.foo"), "group.com.foo");
    }

    #[test]
    fn group_container_team_id_stripped() {
        let e = env();
        let gc = e.home.join("Library/Group Containers");
        mk(&gc.join("UBF8T346G9.com.foo.app"));
        mk(&gc.join("ABCDEFGHIJ.group.com.foo.app"));
        mk(&gc.join("UBF8T346G9.com.other"));
        let a = app(&e.home.join("Applications/Foo.app"), "com.foo.app", "Foo");
        let parts = parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib);
        let got = labels(&parts);
        assert!(got.contains(&"UBF8T346G9.com.foo.app".to_string()));
        assert!(got.contains(&"ABCDEFGHIJ.group.com.foo.app".to_string()));
        assert!(!got.contains(&"UBF8T346G9.com.other".to_string()));
    }

    #[test]
    fn launcher_script_destfolder_joins_group() {
        let e = env();
        let payload = e
            .home
            .join("Library/Application Support/Autodesk/webdeploy/production/b43a");
        mk(&payload);
        let a = app(
            &e.home.join("Applications/Autodesk Fusion.app"),
            "com.autodesk.fusion",
            "Autodesk Fusion",
        );
        mk(a.executable.parent().unwrap());
        fs::write(
            &a.executable,
            format!(
                "#!/bin/sh\nrealApp=\"Autodesk Fusion\"\ndestfolder=\"{}\"\n",
                payload.display()
            ),
        )
        .unwrap();
        let parts = parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib);
        assert!(
            parts
                .iter()
                .any(|p| p.path == payload && p.label == "payload")
        );
        // A destfolder outside ~/Library is ignored.
        fs::write(&a.executable, "destfolder=\"/etc\"\n").unwrap();
        assert!(launcher_target(&a, &e.home.join("Library")).is_none());
    }

    #[test]
    fn vendor_folder_needs_two_segments_and_name_match() {
        let e = env();
        mk(&e.home.join("Library/Application Support/Autodesk"));
        let a = app(
            &e.home.join("Applications/Fusion.app"),
            "com.autodesk.fusion",
            "Fusion",
        );
        let other = app(
            &e.home.join("Applications/Maya.app"),
            "com.autodesk.maya",
            "Maya",
        );
        let alone = parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib);
        let v = alone.iter().find(|p| p.label == "Vendor folder").unwrap();
        assert!(!v.shared_vendor);
        let both = parts_of(&a, &[a.clone(), other], &e.home, &e.lib);
        assert!(
            both.iter()
                .find(|p| p.label == "Vendor folder")
                .unwrap()
                .shared_vendor
        );
    }

    #[test]
    fn big_vendors_never_attach_vendor_folder() {
        let e = env();
        mk(&e.home.join("Library/Application Support/Google"));
        mk(&e.home.join("Library/Application Support/Microsoft"));
        for (id, n) in [
            ("com.google.Chrome", "Chrome"),
            ("com.microsoft.Word", "Word"),
        ] {
            let a = app(&e.home.join(format!("Applications/{n}.app")), id, n);
            assert!(
                !parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib)
                    .iter()
                    .any(|p| p.label == "Vendor folder")
            );
        }
    }

    #[test]
    fn wrapper_bundles_in_same_dir_join_and_the_main_app_is_not_one() {
        let e = env();
        let dir = e.home.join("Applications");
        let base = "com.autodesk.dls.streamer.scriptapp";
        let a = app(
            &dir.join("Autodesk Fusion.app"),
            &format!("{base}.Autodesk-Fusion"),
            "Autodesk Fusion",
        );
        let w = app(
            &dir.join("Remove Autodesk Fusion.app"),
            &format!("{base}.Remove-Autodesk-Fusion"),
            "Remove Autodesk Fusion",
        );
        let unrelated = app(&dir.join("Other.app"), "com.autodesk.other", "Other");
        for x in [&a, &w, &unrelated] {
            mk(&x.path);
        }
        let sib = [a.clone(), w.clone(), unrelated];
        let n = labels(&parts_of(&a, &sib, &e.home, &e.lib));
        assert!(n.contains(&"Remove Autodesk Fusion.app".to_string()));
        assert!(!n.contains(&"Other.app".to_string()));
        assert_eq!(
            wrappers(&a, &sib).len(),
            1,
            "the remover is a wrapper of the app"
        );
        assert!(wrappers(&w, &sib).is_empty(), "never the other way round");
    }

    #[test]
    fn chrome_canary_is_not_a_wrapper_of_chrome() {
        let dir = PathBuf::from("/Applications");
        let a = app(
            &dir.join("Google Chrome.app"),
            "com.google.Chrome",
            "Google Chrome",
        );
        let c = app(
            &dir.join("Google Chrome Canary.app"),
            "com.google.Chrome.canary",
            "Google Chrome Canary",
        );
        assert!(wrappers(&a, &[a.clone(), c]).is_empty());
    }

    #[test]
    fn library_items_outside_home_are_flagged() {
        let e = env();
        mk(&e.lib.join("LaunchDaemons"));
        fs::write(e.lib.join("LaunchDaemons/com.foo.app.helper.plist"), b"x").unwrap();
        let a = app(&e.home.join("Applications/Foo.app"), "com.foo.app", "Foo");
        let parts = parts_of(&a, std::slice::from_ref(&a), &e.home, &e.lib);
        let p = parts.iter().find(|p| p.label == "LaunchDaemons").unwrap();
        assert!(p.outside_home);
    }

    #[test]
    fn never_follows_symlinks_out_of_launcher_target() {
        let e = env();
        let a = app(&e.home.join("Applications/Foo.app"), "com.foo.app", "Foo");
        mk(a.executable.parent().unwrap());
        fs::write(&a.executable, "destfolder=\"/Users/x/Library/../../etc\"\n").unwrap();
        assert!(launcher_target(&a, &e.home.join("Library")).is_none());
    }
}
