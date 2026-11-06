//! Known caches of developer tools. Finding them means asking the installed
//! tools, which can take seconds (a corepack shim may start Node), so
//! [`discover_tools`] belongs on a background thread, never the UI thread.
//!
//! Several Node versions (fnm, nvm, volta) can each carry their own pnpm, and
//! every pnpm major keeps its own store folder (`store/v10`, `store/v11`).
//! Each store is cleaned by the pnpm that reports it. Every command is pinned
//! to one exact install, runs in the home folder (so a corepack shim picks
//! its default, not a project's version), cannot download anything, and has
//! the matching Node first on `PATH`.

use std::fs;
use std::path::{Path, PathBuf};

use super::command::{CommandRunner, CommandSpec};
use super::resolve::{Places, resolve_in};
use crate::bg::Work;
use crate::safety::keeps_offline_data;

/// How a cache is cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Move the folder to ToBeDeleted like any other.
    Folder,
    /// Run the tool's own command.
    Command(CommandSpec),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolEntry {
    /// Stable key for marks, e.g. `pnpm-store:v11`.
    pub id: String,
    /// The group it is listed under, e.g. `pnpm`.
    pub tool: String,
    pub label: String,
    /// The folder this entry stands for (always inside the home folder).
    pub path: PathBuf,
    /// How it comes back after cleaning.
    pub regen: String,
    pub mode: Mode,
    /// Why this entry is not command-owned, or other care to show the user.
    pub note: Option<String>,
}

/// Find every tool cache that exists, using the real `PATH` and environment.
/// `work` shows what it is doing (it asks installed programs, which is slow).
pub fn discover_tools(home: &Path, runner: &dyn CommandRunner, work: &Work) -> Vec<ToolEntry> {
    discover_with(home, runner, &Places::from_env(), work)
}

/// One fixed cache folder and the command that cleans it, if there is one.
struct Fixed {
    id: &'static str,
    tool: &'static str,
    label: &'static str,
    rel: &'static str,
    regen: &'static str,
    /// Programs to try in order, the arguments for the clean command, and
    /// the arguments that force it (empty when it has no such variant).
    command: Option<Command>,
}

type Command = (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

const FIXED: &[Fixed] = &[
    Fixed {
        id: "pnpm-cache",
        tool: "pnpm",
        label: "metadata cache",
        rel: "Library/Caches/pnpm",
        regen: "fetched again when needed",
        command: None,
    },
    Fixed {
        id: "yarn",
        tool: "Yarn",
        label: "cache",
        rel: "Library/Caches/Yarn",
        regen: "downloaded again when needed",
        command: Some((&["yarn"], &["cache", "clean"], &[])),
    },
    Fixed {
        id: "go-build",
        tool: "Go",
        label: "build cache",
        rel: "Library/Caches/go-build",
        regen: "rebuilt when needed",
        command: Some((&["go"], &["clean", "-cache"], &[])),
    },
    Fixed {
        id: "go-mod",
        tool: "Go",
        label: "module cache",
        rel: "go/pkg/mod",
        regen: "downloaded again when needed",
        command: Some((&["go"], &["clean", "-modcache"], &[])),
    },
    Fixed {
        id: "uv",
        tool: "uv",
        label: "cache",
        rel: ".cache/uv",
        regen: "downloaded again when needed",
        // uv waits (up to 5 minutes) while another uv process, for example
        // a `uv run` server, holds its cache; `--force` skips that check.
        command: Some((&["uv"], &["cache", "clean"], &["--force"])),
    },
    Fixed {
        id: "pip",
        tool: "pip",
        label: "cache",
        rel: "Library/Caches/pip",
        regen: "downloaded again when needed",
        command: Some((&["pip", "pip3"], &["cache", "purge"], &[])),
    },
    Fixed {
        id: "brew",
        tool: "Homebrew",
        label: "downloads cache",
        rel: "Library/Caches/Homebrew",
        regen: "downloaded again when needed",
        command: Some((&["brew"], &["cleanup", "--prune=all"], &[])),
    },
    Fixed {
        id: "pub",
        tool: "Dart & Flutter",
        label: "pub cache",
        rel: ".pub-cache",
        regen: "restored by flutter pub get (needs network)",
        command: None,
    },
    Fixed {
        id: "cargo-registry",
        tool: "Cargo",
        label: "registry cache",
        rel: ".cargo/registry",
        regen: "downloaded again by cargo build (needs network)",
        command: None,
    },
    Fixed {
        id: "xcode-derived",
        tool: "Xcode",
        label: "DerivedData",
        rel: "Library/Developer/Xcode/DerivedData",
        regen: "rebuilt by the next Xcode build",
        command: None,
    },
    Fixed {
        id: "playwright",
        tool: "Playwright",
        label: "browsers",
        rel: "Library/Caches/ms-playwright",
        regen: "reinstall with playwright install",
        command: None,
    },
];

/// Pin a command to one install: absolute program, run in the home folder,
/// no corepack downloads or prompts, matching Node first on `PATH`.
fn pinned(home: &Path, exe: &Path, args: &[&str]) -> CommandSpec {
    let mut spec = CommandSpec::new(&exe.to_string_lossy(), args)
        .with_cwd(home)
        .with_env("COREPACK_ENABLE_NETWORK", "0")
        .with_env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0");
    if let Some(dir) = exe.parent() {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(own) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&own));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            spec = spec.with_env("PATH", &joined.to_string_lossy());
        }
    }
    spec
}

/// The first usable line of a query's output: a folder under `home` that exists.
fn folder_under(home: &Path, out: Option<String>) -> Option<PathBuf> {
    let line = out?
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())?
        .trim()
        .to_string();
    let p = PathBuf::from(line);
    (p.is_absolute()
        && !p.components().any(|c| c.as_os_str() == "..")
        && p.starts_with(home)
        && p.is_dir())
    .then_some(p)
}

pub(crate) fn discover_with(
    home: &Path,
    runner: &dyn CommandRunner,
    places: &Places,
    work: &Work,
) -> Vec<ToolEntry> {
    work.begin("looking for installed tools", 0);
    let find = |name: &str| resolve_in(home, name, places);
    let mut out: Vec<ToolEntry> = Vec::new();

    for f in FIXED {
        let path = home.join(f.rel);
        if !path.is_dir() {
            continue;
        }
        let mode = f
            .command
            .and_then(|(programs, args, force)| {
                programs
                    .iter()
                    .find_map(|p| find(p).into_iter().next())
                    .map(|exe| Mode::Command(pinned(home, &exe, args).with_force_args(force)))
            })
            .unwrap_or(Mode::Folder);
        out.push(ToolEntry {
            id: f.id.to_string(),
            tool: f.tool.to_string(),
            label: f.label.to_string(),
            path,
            regen: f.regen.to_string(),
            mode,
            note: None,
        });
    }

    let (pnpms, npms) = (find("pnpm"), find("npm"));
    // One question per install: that is the length of the slow part.
    work.begin(
        "asking pnpm and npm where their caches are",
        pnpms.len() + npms.len(),
    );
    out.extend(pnpm_stores(home, runner, &pnpms, work));
    out.extend(npm_caches(home, runner, &npms, work));
    work.stage(1, 1, "listing app caches");
    let claimed: Vec<PathBuf> = out.iter().map(|e| e.path.clone()).collect();
    out.extend(app_caches(home, &claimed));
    out
}

/// One entry per pnpm store folder, cleaned by the pnpm that reports it.
fn pnpm_stores(
    home: &Path,
    runner: &dyn CommandRunner,
    pnpms: &[PathBuf],
    work: &Work,
) -> Vec<ToolEntry> {
    // Which pnpm owns which store folder (first one found wins).
    let mut owners: Vec<(PathBuf, PathBuf)> = Vec::new();
    for exe in pnpms {
        let spec = pinned(home, exe, &["store", "path"]);
        let answer = runner.query(&spec);
        work.step();
        if let Some(store) = folder_under(home, answer)
            && !owners.iter().any(|(s, _)| *s == store)
        {
            owners.push((store, exe.clone()));
        }
    }
    // Every store folder on disk, in the usual places, plus the reported ones.
    let mut stores: Vec<PathBuf> = Vec::new();
    for root in [
        home.join("Library/pnpm/store"),
        home.join(".local/share/pnpm/store"),
    ] {
        let Ok(rd) = fs::read_dir(&root) else {
            continue;
        };
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy();
                        n.starts_with('v')
                            && n[1..].chars().all(|c| c.is_ascii_digit())
                            && n.len() > 1
                    })
            })
            .collect();
        found.sort();
        stores.extend(found);
    }
    for (store, _) in &owners {
        if !stores.contains(store) {
            stores.push(store.clone());
        }
    }
    stores
        .into_iter()
        .map(|store| {
            let name = store
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let owner = owners.iter().find(|(s, _)| *s == store).map(|(_, e)| e);
            let (mode, note) = match owner {
                Some(exe) => (Mode::Command(pinned(home, exe, &["store", "prune"])), None),
                None => (
                    Mode::Folder,
                    Some(
                        "no installed pnpm reports this store; it is moved instead of pruned"
                            .to_string(),
                    ),
                ),
            };
            ToolEntry {
                id: format!("pnpm-store:{name}"),
                tool: "pnpm".to_string(),
                label: format!("store {name}"),
                path: store,
                regen: "packages are fetched again by pnpm install".to_string(),
                mode,
                note,
            }
        })
        .collect()
}

/// The npm cache folder(s): what each npm reports, or `~/.npm` by default.
fn npm_caches(
    home: &Path,
    runner: &dyn CommandRunner,
    npms: &[PathBuf],
    work: &Work,
) -> Vec<ToolEntry> {
    let mut found: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for exe in npms {
        let spec = pinned(home, exe, &["config", "get", "cache"]);
        let answer = runner.query(&spec);
        work.step();
        if let Some(dir) = folder_under(home, answer)
            && !found.iter().any(|(d, _)| *d == dir)
        {
            found.push((dir, Some(exe.clone())));
        }
    }
    if found.is_empty() {
        let default = home.join(".npm");
        if default.is_dir() {
            found.push((default, npms.first().cloned()));
        }
    }
    found
        .into_iter()
        .map(|(dir, exe)| {
            let rel = dir
                .strip_prefix(home)
                .unwrap_or(&dir)
                .to_string_lossy()
                .into_owned();
            ToolEntry {
                id: format!("npm-cache:{rel}"),
                tool: "npm".to_string(),
                label: "cache".to_string(),
                path: dir,
                regen: "downloaded again when needed".to_string(),
                mode: exe.map_or(Mode::Folder, |e| {
                    Mode::Command(pinned(home, &e, &["cache", "clean", "--force"]))
                }),
                note: None,
            }
        })
        .collect()
}

/// Every other folder in `~/Library/Caches` and `~/.cache`, one entry each.
/// System-managed `com.apple.*` caches are left alone.
fn app_caches(home: &Path, claimed: &[PathBuf]) -> Vec<ToolEntry> {
    let mut out = Vec::new();
    for root in [home.join("Library/Caches"), home.join(".cache")] {
        let Ok(rd) = fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for path in dirs {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let related = claimed
                .iter()
                .any(|c| c.starts_with(&path) || path.starts_with(c));
            if related || name.starts_with("com.apple.") {
                continue;
            }
            let rel = path
                .strip_prefix(home)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let offline = keeps_offline_data(&name);
            out.push(ToolEntry {
                id: format!("app-cache:{rel}"),
                tool: "App caches".to_string(),
                label: name,
                path,
                regen: if offline {
                    "hard to rebuild: may hold offline downloads or saved media".to_string()
                } else {
                    "rebuilt by the app as needed".to_string()
                },
                mode: Mode::Folder,
                note: offline.then(|| {
                    "check this one before you empty ToBeDeleted: offline music, saved media or downloads can live here".to_string()
                }),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use crate::catalog::resolve::Places;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn dir(p: &Path) {
        fs::create_dir_all(p).unwrap();
        fs::write(p.join("data"), "x").unwrap();
    }

    fn exe(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    struct Home {
        _d: tempfile::TempDir,
        home: PathBuf,
    }

    fn home() -> Home {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap().join("home");
        fs::create_dir_all(&home).unwrap();
        Home { _d: d, home }
    }

    fn fnm_bin(home: &Path, version: &str, prog: &str) -> PathBuf {
        home.join(format!(
            ".local/share/fnm/node-versions/{version}/installation/bin/{prog}"
        ))
    }

    fn find(h: &Home, runner: &FakeRunner) -> Vec<ToolEntry> {
        discover_with(&h.home, runner, &Places::only(OsStr::new("")), &Work::new())
    }

    fn by_id<'a>(entries: &'a [ToolEntry], id: &str) -> &'a ToolEntry {
        entries.iter().find(|e| e.id == id).unwrap_or_else(|| {
            panic!(
                "no entry {id} in {:?}",
                entries.iter().map(|e| &e.id).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn entries_with_no_existing_path_are_dropped() {
        let h = home();
        let entries = find(&h, &FakeRunner::new(&[]));
        assert!(
            entries.is_empty(),
            "{:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_tool_with_a_program_gets_a_command_and_one_without_stays_a_folder() {
        let h = home();
        dir(&h.home.join("Library/Caches/go-build"));
        dir(&h.home.join("Library/Caches/Yarn"));
        let go = h.home.join("tools/go");
        exe(&go);
        let places = Places::only(go.parent().unwrap().as_os_str());
        let entries = discover_with(&h.home, &FakeRunner::new(&[]), &places, &Work::new());
        match &by_id(&entries, "go-build").mode {
            Mode::Command(spec) => assert_eq!(spec.display(), "go clean -cache"),
            other => panic!("expected a command, got {other:?}"),
        }
        assert!(matches!(by_id(&entries, "yarn").mode, Mode::Folder));
    }

    #[test]
    fn only_uv_cache_clean_offers_a_force_variant() {
        let h = home();
        dir(&h.home.join(".cache/uv"));
        dir(&h.home.join("Library/Caches/go-build"));
        let uv = h.home.join("tools/uv");
        let go = h.home.join("tools/go");
        exe(&uv);
        exe(&go);
        let path = std::env::join_paths([uv.parent().unwrap()]).unwrap();
        let entries = discover_with(
            &h.home,
            &FakeRunner::new(&[]),
            &Places::only(&path),
            &Work::new(),
        );
        let spec = |id: &str| match &by_id(&entries, id).mode {
            Mode::Command(s) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert!(spec("uv").can_force());
        assert_eq!(spec("uv").forced().display(), "uv cache clean --force");
        assert!(!spec("go-build").can_force());
    }

    #[test]
    fn each_pnpm_store_version_gets_the_pnpm_that_owns_it() {
        let h = home();
        dir(&h.home.join("Library/pnpm/store/v10"));
        dir(&h.home.join("Library/pnpm/store/v11"));
        let (p20, p22) = (
            fnm_bin(&h.home, "v20.1.0", "pnpm"),
            fnm_bin(&h.home, "v22.2.0", "pnpm"),
        );
        exe(&p20);
        exe(&p22);
        let runner = FakeRunner::new(&[]).answering(&[
            (
                "v20.1.0/installation/bin/pnpm store path",
                h.home.join("Library/pnpm/store/v10").to_str().unwrap(),
            ),
            (
                "v22.2.0/installation/bin/pnpm store path",
                h.home.join("Library/pnpm/store/v11").to_str().unwrap(),
            ),
        ]);
        let entries = find(&h, &runner);
        let spec = |id: &str| match &by_id(&entries, id).mode {
            Mode::Command(s) => s.clone(),
            other => panic!("{id}: {other:?}"),
        };
        assert_eq!(spec("pnpm-store:v10").program, p20.to_str().unwrap());
        assert_eq!(spec("pnpm-store:v11").program, p22.to_str().unwrap());
        assert_eq!(spec("pnpm-store:v11").display(), "pnpm store prune");
    }

    #[test]
    fn a_store_no_installed_pnpm_reports_falls_back_to_a_folder_with_a_note() {
        let h = home();
        dir(&h.home.join("Library/pnpm/store/v3"));
        exe(&fnm_bin(&h.home, "v22.2.0", "pnpm"));
        let runner = FakeRunner::new(&[]).answering(&[(
            "pnpm store path",
            h.home.join("Library/pnpm/store/v11").to_str().unwrap(),
        )]);
        let entries = find(&h, &runner);
        let e = by_id(&entries, "pnpm-store:v3");
        assert!(matches!(e.mode, Mode::Folder));
        assert!(
            e.note
                .as_deref()
                .unwrap_or("")
                .contains("no installed pnpm")
        );
    }

    #[test]
    fn a_pnpm_that_fails_or_times_out_is_not_fatal() {
        let h = home();
        dir(&h.home.join("Library/pnpm/store/v10"));
        exe(&fnm_bin(&h.home, "v22.2.0", "pnpm"));
        // No answer at all: the query failed.
        let entries = find(&h, &FakeRunner::new(&[]));
        assert!(matches!(
            by_id(&entries, "pnpm-store:v10").mode,
            Mode::Folder
        ));
    }

    #[test]
    fn pnpm_commands_are_pinned_to_one_install_and_cannot_hang() {
        let h = home();
        dir(&h.home.join("Library/pnpm/store/v10"));
        let p = fnm_bin(&h.home, "v20.1.0", "pnpm");
        exe(&p);
        let runner = FakeRunner::new(&[]).answering(&[(
            "pnpm store path",
            h.home.join("Library/pnpm/store/v10").to_str().unwrap(),
        )]);
        let entries = find(&h, &runner);
        let Mode::Command(spec) = &by_id(&entries, "pnpm-store:v10").mode else {
            panic!("expected a command");
        };
        assert_eq!(spec.cwd.as_deref(), Some(h.home.as_path()));
        let env = |k: &str| {
            spec.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(env("COREPACK_ENABLE_NETWORK"), Some("0"));
        assert_eq!(env("COREPACK_ENABLE_DOWNLOAD_PROMPT"), Some("0"));
        let path = env("PATH").expect("PATH is set so the matching node is found");
        assert!(path.starts_with(p.parent().unwrap().to_str().unwrap()));
    }

    #[test]
    fn a_custom_store_under_home_is_listed_and_one_outside_home_is_not() {
        let h = home();
        dir(&h.home.join("data/pnpm-store/v10"));
        let outside = tempfile::tempdir().unwrap();
        dir(&outside.path().join("store/v10"));
        let (a, b) = (
            fnm_bin(&h.home, "v20.0.0", "pnpm"),
            fnm_bin(&h.home, "v22.0.0", "pnpm"),
        );
        exe(&a);
        exe(&b);
        let runner = FakeRunner::new(&[]).answering(&[
            (
                "v20.0.0/installation/bin/pnpm store path",
                h.home.join("data/pnpm-store/v10").to_str().unwrap(),
            ),
            (
                "v22.0.0/installation/bin/pnpm store path",
                outside.path().join("store/v10").to_str().unwrap(),
            ),
        ]);
        let entries = find(&h, &runner);
        assert_eq!(entries.iter().filter(|e| e.tool == "pnpm").count(), 1);
        assert!(entries.iter().all(|e| e.path.starts_with(&h.home)));
    }

    #[test]
    fn the_npm_cache_folder_comes_from_npm_and_one_outside_home_is_ignored() {
        let h = home();
        dir(&h.home.join(".npm"));
        dir(&h.home.join("elsewhere/npm-cache"));
        let n = fnm_bin(&h.home, "v22.0.0", "npm");
        exe(&n);
        let runner = FakeRunner::new(&[]).answering(&[(
            "npm config get cache",
            h.home.join("elsewhere/npm-cache").to_str().unwrap(),
        )]);
        let entries = find(&h, &runner);
        let npm: Vec<_> = entries.iter().filter(|e| e.tool == "npm").collect();
        assert_eq!(npm.len(), 1);
        assert_eq!(npm[0].path, h.home.join("elsewhere/npm-cache"));
        match &npm[0].mode {
            Mode::Command(s) => assert_eq!(s.display(), "npm cache clean --force"),
            other => panic!("{other:?}"),
        }
        // Reported outside home: dropped, the default ~/.npm is used instead.
        let runner = FakeRunner::new(&[]).answering(&[("npm config get cache", "/Volumes/x/npm")]);
        let entries = find(&h, &runner);
        let npm: Vec<_> = entries.iter().filter(|e| e.tool == "npm").collect();
        assert_eq!(npm.len(), 1);
        assert_eq!(npm[0].path, h.home.join(".npm"));
    }

    #[test]
    fn app_caches_list_unclaimed_children_only_and_skip_system_ones() {
        let h = home();
        dir(&h.home.join("Library/Caches/Spotify"));
        dir(&h.home.join("Library/Caches/com.apple.Safari"));
        dir(&h.home.join("Library/Caches/go-build"));
        dir(&h.home.join(".cache/huggingface"));
        let entries = find(&h, &FakeRunner::new(&[]));
        let apps: Vec<_> = entries
            .iter()
            .filter(|e| e.tool == "App caches")
            .map(|e| e.label.as_str())
            .collect();
        assert!(apps.contains(&"Spotify"), "{apps:?}");
        assert!(apps.contains(&"huggingface"), "{apps:?}");
        assert!(!apps.iter().any(|l| l.contains("apple")), "{apps:?}");
        assert!(
            !apps.contains(&"go-build"),
            "claimed by the Go entry: {apps:?}"
        );
    }

    #[test]
    fn app_caches_that_may_hold_offline_data_say_so() {
        let h = home();
        dir(&h.home.join("Library/Caches/Spotify"));
        dir(&h.home.join("Library/Caches/ru.keepcoder.Telegram"));
        dir(&h.home.join(".cache/huggingface"));
        dir(&h.home.join("Library/Caches/SomeEditor"));
        let entries = find(&h, &FakeRunner::new(&[]));
        for label in ["Spotify", "ru.keepcoder.Telegram", "huggingface"] {
            let e = entries.iter().find(|e| e.label == label).unwrap();
            assert!(
                e.regen.starts_with("hard to rebuild"),
                "{label}: {}",
                e.regen
            );
            assert!(
                e.note.as_deref().unwrap_or("").contains("before you empty"),
                "{label}"
            );
        }
        let plain = entries.iter().find(|e| e.label == "SomeEditor").unwrap();
        assert_eq!(plain.regen, "rebuilt by the app as needed");
        assert!(plain.note.is_none());
    }

    #[test]
    fn entry_paths_are_inside_home_and_never_nested() {
        let h = home();
        for p in [
            "Library/Caches/go-build",
            "Library/Caches/Spotify",
            ".cache/uv",
            ".cache/other",
            ".cargo/registry",
            "Library/Developer/Xcode/DerivedData",
            ".pub-cache",
        ] {
            dir(&h.home.join(p));
        }
        let entries = find(&h, &FakeRunner::new(&[]));
        assert!(entries.len() >= 7);
        for a in &entries {
            assert!(a.path.starts_with(&h.home));
            for b in &entries {
                if a.id != b.id {
                    assert!(!a.path.starts_with(&b.path), "{} is inside {}", a.id, b.id);
                }
            }
        }
    }
}
