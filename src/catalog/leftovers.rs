//! Leftovers: folders that belong to an app that is no longer installed.
//! A group is listed only with positive evidence (see the design doc, "Leftover
//! evidence"); when unsure, nothing is listed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::appfiles::{AppPart, BIG_VENDORS, strip_team_id};
use super::apppolicy::is_apple_id;
use super::apps::{InstalledIndex, vendor_of};

#[derive(Clone, Debug)]
pub struct LeftoverGroup {
    pub id: String,
    pub parts: Vec<AppPart>,
    /// Why it is listed (shown in the details pane).
    pub reason: String,
}

#[derive(Clone, Debug)]
pub enum LeftoversState {
    Ready(Vec<LeftoverGroup>),
    /// Nothing is listed, and this says why.
    Unavailable(String),
}

/// Does the text look like a reverse-DNS bundle id (`com.foo.app`)?
fn id_shaped(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        && parts[0].len() <= 6
        && parts[0].chars().all(|c| c.is_ascii_lowercase())
}

/// The bundle id a per-app file or folder name stands for, if any.
fn id_of(name: &str) -> Option<String> {
    let mut n = strip_team_id(name);
    for suffix in [".plist", ".savedState", ".binarycookies"] {
        if let Some(s) = n.strip_suffix(suffix) {
            n = s;
        }
    }
    let n = n.strip_prefix("group.").unwrap_or(n);
    id_shaped(n).then(|| n.to_string())
}

/// The program a launchd plist starts, from `Program` or `ProgramArguments[0]`.
fn plist_program(path: &Path) -> Option<PathBuf> {
    let v = plist::Value::from_file(path).ok()?;
    let d = v.as_dictionary()?;
    let prog = d
        .get("Program")
        .and_then(|p| p.as_string())
        .or_else(|| d.get("ProgramArguments")?.as_array()?.first()?.as_string())?;
    Some(PathBuf::from(prog))
}

fn add_part(
    groups: &mut BTreeMap<String, LeftoverGroup>,
    id: String,
    path: PathBuf,
    label: &str,
    outside: bool,
) {
    let g = groups
        .entry(id.to_ascii_lowercase())
        .or_insert_with(|| LeftoverGroup {
            reason: format!("no app with id {id} is installed"),
            id,
            parts: Vec::new(),
        });
    if !g.parts.iter().any(|p| p.path == path) {
        g.parts.push(AppPart {
            path,
            label: label.to_string(),
            shared_vendor: false,
            outside_home: outside,
        });
    }
}

pub fn find_leftovers(
    index: &InstalledIndex,
    home: &Path,
    lib: &Path,
    running_ids: &BTreeSet<String>,
) -> LeftoversState {
    if !index.spotlight_ok {
        return LeftoversState::Unavailable(
            "Spotlight did not list the installed apps (it may be switched off), so leftovers \
             cannot be told apart from data of apps that are still installed"
                .to_string(),
        );
    }
    let mut groups: BTreeMap<String, LeftoverGroup> = BTreeMap::new();
    let allowed = |id: &str| {
        !is_apple_id(id)
            && !index.has_id(id)
            && !running_ids.contains(id)
            && !(index.vendor_has_app(id) && !BIG_VENDORS.contains(&vendor_of(id).as_str()))
    };
    let hl = home.join("Library");
    for dir in [
        "Caches",
        "Containers",
        "Group Containers",
        "HTTPStorages",
        "WebKit",
        "Application Scripts",
        "Preferences",
        "Preferences/ByHost",
        "Saved Application State",
        "Cookies",
        "Application Support",
    ] {
        let Ok(rd) = fs::read_dir(hl.join(dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let base = if dir.ends_with("ByHost") {
                // `com.foo.app.<uuid>.plist`: keep the id part only when the
                // whole name is not id-shaped on its own.
                name.rsplitn(3, '.')
                    .last()
                    .map(str::to_string)
                    .unwrap_or(name.clone())
            } else {
                name.clone()
            };
            let Some(id) = id_of(&base) else { continue };
            if allowed(&id) {
                add_part(&mut groups, id, e.path(), dir, false);
            }
        }
    }
    // Launch items whose program is gone, and helper tools named like them.
    for (dir, base) in [
        ("LaunchAgents", hl.join("LaunchAgents")),
        ("LaunchAgents", lib.join("LaunchAgents")),
        ("LaunchDaemons", lib.join("LaunchDaemons")),
    ] {
        let Ok(rd) = fs::read_dir(&base) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = id_of(&name) else { continue };
            let Some(prog) = plist_program(&e.path()) else {
                continue;
            };
            if allowed(&id) && !prog.exists() {
                let outside = !e.path().starts_with(home);
                add_part(&mut groups, id, e.path(), dir, outside);
            }
        }
    }
    if let Ok(rd) = fs::read_dir(lib.join("PrivilegedHelperTools")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) = id_of(&name)
                && groups.contains_key(&id.to_ascii_lowercase())
            {
                add_part(&mut groups, id, e.path(), "PrivilegedHelperTools", true);
            }
        }
    }
    LeftoversState::Ready(groups.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::apps::AppBundle;

    struct Env {
        _d: tempfile::TempDir,
        home: PathBuf,
        lib: PathBuf,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let (home, lib) = (root.join("home"), root.join("lib"));
        fs::create_dir_all(home.join("Library")).unwrap();
        fs::create_dir_all(&lib).unwrap();
        Env { _d: d, home, lib }
    }

    fn mk(p: &Path) {
        fs::create_dir_all(p).unwrap();
    }

    fn touch(p: &Path) {
        mk(p.parent().unwrap());
        fs::write(p, b"x").unwrap();
    }

    fn index(ids: &[&str], nested: &[&str], ok: bool) -> InstalledIndex {
        InstalledIndex {
            apps: ids
                .iter()
                .map(|id| AppBundle {
                    path: format!("/Applications/{id}.app").into(),
                    id: id.to_string(),
                    name: id.to_string(),
                    version: String::new(),
                    executable: PathBuf::new(),
                    owner: "u".into(),
                    last_used: 0,
                })
                .collect(),
            nested_ids: nested.iter().map(|s| s.to_string()).collect(),
            spotlight_ok: ok,
        }
    }

    fn ready(s: LeftoversState) -> Vec<LeftoverGroup> {
        match s {
            LeftoversState::Ready(g) => g,
            LeftoversState::Unavailable(w) => panic!("unavailable: {w}"),
        }
    }

    fn run(e: &Env, idx: &InstalledIndex) -> Vec<LeftoverGroup> {
        ready(find_leftovers(idx, &e.home, &e.lib, &BTreeSet::new()))
    }

    #[test]
    fn unavailable_when_spotlight_not_ok() {
        let e = env();
        match find_leftovers(&index(&[], &[], false), &e.home, &e.lib, &BTreeSet::new()) {
            LeftoversState::Unavailable(w) => assert!(w.contains("Spotlight")),
            _ => panic!("expected unavailable"),
        }
    }

    #[test]
    fn lists_folder_named_by_uninstalled_id_and_groups_it() {
        let e = env();
        let hl = e.home.join("Library");
        mk(&hl.join("Caches/com.gone.app"));
        touch(&hl.join("Preferences/com.gone.app.plist"));
        mk(&hl.join("Saved Application State/com.gone.app.savedState"));
        let g = run(&e, &index(&["com.other.app"], &[], true));
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].id, "com.gone.app");
        assert_eq!(g[0].parts.len(), 3);
        assert!(g[0].reason.contains("com.gone.app"));
    }

    #[test]
    fn ignores_ids_of_installed_or_nested_helpers() {
        let e = env();
        let hl = e.home.join("Library");
        mk(&hl.join("Caches/com.foo.app"));
        mk(&hl.join("Caches/com.foo.helper"));
        assert!(run(&e, &index(&["com.foo.app"], &["com.foo.helper"], true)).is_empty());
    }

    #[test]
    fn name_only_folders_are_never_leftovers() {
        let e = env();
        mk(&e.home.join("Library/Application Support/Some App"));
        mk(&e.home.join("Library/Application Support/Autodesk"));
        assert!(run(&e, &index(&[], &[], true)).is_empty());
    }

    #[test]
    fn apple_ids_are_never_leftovers() {
        let e = env();
        mk(&e.home.join("Library/Caches/com.apple.Safari"));
        assert!(run(&e, &index(&[], &[], true)).is_empty());
    }

    #[test]
    fn small_vendor_with_an_installed_app_is_hidden_but_big_vendor_is_not() {
        let e = env();
        let hl = e.home.join("Library");
        mk(&hl.join("HTTPStorages/com.Autodesk.streamer"));
        mk(&hl.join("Caches/com.microsoft.teams"));
        let g = run(
            &e,
            &index(
                &[
                    "com.autodesk.dls.streamer.scriptapp.Autodesk-Fusion",
                    "com.microsoft.Word",
                ],
                &[],
                true,
            ),
        );
        let ids: Vec<_> = g.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["com.microsoft.teams"]);
    }

    #[test]
    fn running_ids_are_skipped() {
        let e = env();
        mk(&e.home.join("Library/Caches/com.run.app"));
        let running = BTreeSet::from(["com.run.app".to_string()]);
        let s = find_leftovers(&index(&[], &[], true), &e.home, &e.lib, &running);
        assert!(ready(s).is_empty());
    }

    fn daemon(path: &Path, program: &str) {
        mk(path.parent().unwrap());
        let mut d = plist::Dictionary::new();
        d.insert("Label".into(), "x".into());
        d.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![program.into()]),
        );
        plist::Value::Dictionary(d).to_file_xml(path).unwrap();
    }

    #[test]
    fn launch_daemon_listed_only_when_program_missing() {
        let e = env();
        let live = e.lib.join("bin_live");
        touch(&live);
        daemon(
            &e.lib.join("LaunchDaemons/com.gone.helper.plist"),
            "/nonexistent/helper",
        );
        daemon(
            &e.lib.join("LaunchDaemons/com.live.helper.plist"),
            live.to_str().unwrap(),
        );
        touch(&e.lib.join("PrivilegedHelperTools/com.gone.helper"));
        let g = run(&e, &index(&[], &[], true));
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].id, "com.gone.helper");
        assert_eq!(g[0].parts.len(), 2);
        assert!(g[0].parts.iter().all(|p| p.outside_home));
    }
}
