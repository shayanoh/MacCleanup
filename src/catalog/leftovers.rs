//! Leftovers: folders that belong to an app that is no longer installed.
//! A group is listed only with positive evidence (see the design doc, "Leftover
//! evidence"); when unsure, nothing is listed.

use std::collections::BTreeMap;
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
    let n = n
        .strip_prefix("group.")
        .or_else(|| n.strip_prefix("groups."))
        .unwrap_or(n);
    // Temporary files (`...binarycookies_tmp_597_0.dat`) are not app ids.
    (id_shaped(n) && !n.contains("_tmp_")).then(|| n.to_string())
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

/// A group that anything touched this recently is still in use by something
/// (an app we did not see, a service): never listed.
pub const MIN_IDLE_DAYS: i64 = 30;

/// Newest modification time (Unix seconds) among the top-level parts.
fn newest_mtime(parts: &[AppPart]) -> i64 {
    use std::os::unix::fs::MetadataExt;
    parts
        .iter()
        .filter_map(|p| fs::symlink_metadata(&p.path).ok())
        .map(|m| m.mtime())
        .max()
        .unwrap_or(0)
}

/// Fold `a.b.c.helper` groups into `a.b.c` when both are leftovers.
fn merge_children(groups: BTreeMap<String, LeftoverGroup>) -> Vec<LeftoverGroup> {
    let keys: Vec<String> = groups.keys().cloned().collect();
    let parent_of = |k: &str| {
        keys.iter()
            .filter(|p| p.as_str() != k && k.starts_with(&format!("{p}.")))
            .min_by_key(|p| p.len())
            .cloned()
    };
    let mut out: BTreeMap<String, LeftoverGroup> = BTreeMap::new();
    let mut children: Vec<(String, LeftoverGroup)> = Vec::new();
    for (k, g) in groups {
        match parent_of(&k) {
            Some(p) => children.push((p, g)),
            None => {
                out.insert(k, g);
            }
        }
    }
    for (p, g) in children {
        if let Some(parent) = out.get_mut(&p) {
            for part in g.parts {
                if !parent.parts.iter().any(|x| x.path == part.path) {
                    parent.parts.push(part);
                }
            }
        }
    }
    out.into_values().collect()
}

pub fn find_leftovers(
    index: &InstalledIndex,
    home: &Path,
    lib: &Path,
    running: &[PathBuf],
    now: i64,
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
            // Only "not found" means gone: a program in a folder we may not
            // read is still there.
            let gone = matches!(
                fs::metadata(&prog),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound
            );
            if allowed(&id) && gone {
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
    let idle_before = now - MIN_IDLE_DAYS * 86_400;
    groups.retain(|_, g| newest_mtime(&g.parts) < idle_before);
    // Nothing a running process executes from, and nothing that might belong
    // to an app whose Info.plist could not be read.
    groups.retain(|_, g| {
        let squashed = super::apps::squash(&g.id);
        !running
            .iter()
            .any(|r| g.parts.iter().any(|p| r.starts_with(&p.path)))
            && !index
                .unreadable
                .iter()
                .any(|u| squashed.contains(u.as_str()))
    });
    LeftoversState::Ready(merge_children(groups))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::apps::AppBundle;

    /// "Now" for tests: far after the files the tests just created.
    const LATER: i64 = 4_000_000_000;

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
            unreadable: Vec::new(),
        }
    }

    fn ready(s: LeftoversState) -> Vec<LeftoverGroup> {
        match s {
            LeftoversState::Ready(g) => g,
            LeftoversState::Unavailable(w) => panic!("unavailable: {w}"),
        }
    }

    fn run(e: &Env, idx: &InstalledIndex) -> Vec<LeftoverGroup> {
        ready(find_leftovers(idx, &e.home, &e.lib, &[], LATER))
    }

    #[test]
    fn unavailable_when_spotlight_not_ok() {
        let e = env();
        match find_leftovers(&index(&[], &[], false), &e.home, &e.lib, &[], LATER) {
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
    fn a_group_with_a_running_program_inside_is_skipped() {
        let e = env();
        let dir = e.home.join("Library/Caches/com.run.app");
        mk(&dir);
        let running = vec![dir.join("helper")];
        let s = find_leftovers(&index(&[], &[], true), &e.home, &e.lib, &running, LATER);
        assert!(ready(s).is_empty());
    }

    #[test]
    fn data_of_an_app_with_an_unreadable_plist_is_not_called_a_leftover() {
        let e = env();
        mk(&e.home.join("Library/Caches/com.vendor.broken-app"));
        mk(&e.home.join("Library/Caches/com.other.thing"));
        let mut idx = index(&[], &[], true);
        idx.unreadable = vec!["brokenapp".to_string()];
        let g = run(&e, &idx);
        let ids: Vec<_> = g.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["com.other.thing"]);
    }

    #[test]
    fn apple_group_ids_and_data_of_installed_products_are_not_leftovers() {
        let e = env();
        let hl = e.home.join("Library");
        mk(&hl.join("Group Containers/243LU875E5.groups.com.apple.podcasts"));
        mk(&hl.join("Group Containers/group.is.workflow.shortcuts"));
        touch(&hl.join("Preferences/org.cups.PrintingPrefs.plist"));
        touch(&hl.join("Preferences/com.google.drivefs.settings.plist"));
        mk(&hl.join("Containers/com.microsoft.teams2.agent"));
        let idx = index(&["com.google.drivefs", "com.microsoft.teams2"], &[], true);
        assert!(
            run(&e, &idx).is_empty(),
            "{:?}",
            run(&e, &idx).iter().map(|g| &g.id).collect::<Vec<_>>()
        );
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

    #[test]
    fn recently_touched_groups_are_not_listed() {
        let e = env();
        mk(&e.home.join("Library/Caches/com.busy.app"));
        let idx = index(&[], &[], true);
        // "Now" is the moment of creation: the folder is brand new.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let fresh = ready(find_leftovers(&idx, &e.home, &e.lib, &[], now));
        assert!(fresh.is_empty(), "touched today means still in use");
        let old = ready(find_leftovers(
            &idx,
            &e.home,
            &e.lib,
            &[],
            now + 31 * 86_400,
        ));
        assert_eq!(old.len(), 1);
    }

    #[test]
    fn helper_ids_fold_into_the_main_group_and_temp_files_are_ignored() {
        let e = env();
        let hl = e.home.join("Library");
        mk(&hl.join("Caches/com.gone.app"));
        mk(&hl.join("Caches/com.gone.app.helper"));
        mk(&hl.join("HTTPStorages/com.gone.app.perplexityd"));
        touch(&hl.join("Cookies/com.gone.other.binarycookies_tmp_597_0.dat"));
        let g = run(&e, &index(&[], &[], true));
        assert_eq!(
            g.len(),
            1,
            "{:?}",
            g.iter().map(|g| &g.id).collect::<Vec<_>>()
        );
        assert_eq!(g[0].id, "com.gone.app");
        assert_eq!(g[0].parts.len(), 3);
    }
}
