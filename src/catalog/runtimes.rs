//! Installed language runtimes and the packages installed globally into them.
//! This module only reads folders and small project files: it never runs a
//! program (commands come later, from the entries built on top of it).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::command::{CommandSpec, is_executable};
use super::resolve::{Places, resolve_in};
use super::tools::{Mode, ToolEntry, pinned};
use crate::targets::SectionKind;

/// The tool that installed a Node version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Fnm,
    Nvm,
}

/// One Node version installed by fnm or nvm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInstall {
    pub manager: Manager,
    /// As the folder is named: `v22.1.0`.
    pub version: String,
    /// Where `bin` and `lib/node_modules` are: `<version>/installation` for
    /// fnm, `<version>` for nvm.
    pub root: PathBuf,
    /// The folder the manager's uninstall removes (the version folder).
    pub dir: PathBuf,
    pub is_default: bool,
}

/// `(base, versions folder)` for every place fnm and nvm may keep versions.
fn roots(home: &Path, places: &Places) -> Vec<(Manager, PathBuf, PathBuf)> {
    let mut fnm: Vec<PathBuf> = places.fnm_dir.iter().cloned().collect();
    fnm.push(home.join(".local/share/fnm"));
    fnm.push(home.join("Library/Application Support/fnm"));
    let mut nvm: Vec<PathBuf> = places.nvm_dir.iter().cloned().collect();
    nvm.push(home.join(".nvm"));
    fnm.into_iter()
        .map(|b| (Manager::Fnm, b.join("node-versions"), b))
        .chain(
            nvm.into_iter()
                .map(|b| (Manager::Nvm, b.join("versions/node"), b)),
        )
        .collect()
}

/// `v22.1.0` as numbers, for ordering; `None` if it is not that shape.
fn parse_version(name: &str) -> Option<(u32, u32, u32)> {
    let mut it = name.strip_prefix('v')?.split('.');
    let mut next = || it.next()?.parse::<u32>().ok();
    let v = (next()?, next()?, next()?);
    it.next().is_none().then_some(v)
}

/// Every Node version fnm and nvm have installed under `home`, newest first.
/// The default is read from the manager's own alias (never by running it).
pub(crate) fn node_installs(home: &Path, places: &Places) -> Vec<NodeInstall> {
    let mut out: Vec<NodeInstall> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (manager, versions, base) in roots(home, places) {
        let Ok(rd) = fs::read_dir(&versions) else {
            continue;
        };
        let default = default_of(manager, &base);
        for e in rd.flatten() {
            let version = e.file_name().to_string_lossy().into_owned();
            let dir = e.path();
            if parse_version(&version).is_none() || !dir.is_dir() {
                continue;
            }
            let key = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let root = match manager {
                Manager::Fnm => dir.join("installation"),
                Manager::Nvm => dir.clone(),
            };
            let is_default = match &default {
                Some(Default::Path(p)) => fs::canonicalize(&root).ok().as_ref() == Some(p),
                Some(Default::Name(n)) => *n == version,
                None => false,
            };
            out.push(NodeInstall {
                manager,
                version,
                root,
                dir,
                is_default,
            });
        }
    }
    out.sort_by(|a, b| {
        parse_version(&b.version)
            .cmp(&parse_version(&a.version))
            .then_with(|| a.dir.cmp(&b.dir))
    });
    out
}

enum Default {
    Path(PathBuf),
    Name(String),
}

fn default_of(manager: Manager, base: &Path) -> Option<Default> {
    match manager {
        // fnm: `aliases/default` links (through `lts-latest`) to an install.
        Manager::Fnm => fs::canonicalize(base.join("aliases/default"))
            .ok()
            .map(Default::Path),
        // nvm: `alias/default` is a file holding a version name (or `lts/*`).
        Manager::Nvm => fs::read_to_string(base.join("alias/default"))
            .ok()
            .map(|s| Default::Name(s.trim().to_string())),
    }
}

/// The packages installed globally into one Node version: name (scoped names
/// as `@scope/name`) and folder. `npm` and `corepack` ship with Node and are
/// not packages the user installed.
pub fn node_globals(install: &NodeInstall) -> Vec<(String, PathBuf)> {
    let base = install.root.join("lib/node_modules");
    let Ok(rd) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "npm" || name == "corepack" {
            continue;
        }
        if name.starts_with('@') {
            let Ok(sub) = fs::read_dir(e.path()) else {
                continue;
            };
            for s in sub.flatten() {
                let n = s.file_name().to_string_lossy().into_owned();
                if !n.starts_with('.') && s.path().is_dir() {
                    out.push((format!("{name}/{n}"), s.path()));
                }
            }
        } else if e.path().is_dir() {
            out.push((name, e.path()));
        }
    }
    out.sort();
    out
}

/// The numbers of a Node version request (`22`, `v22.1`, `22.x`, `22.1.0`),
/// or `None` for anything else (`lts/*`, `>=18`, `node`, ranges).
fn parse_pin(text: &str) -> Option<Vec<u32>> {
    let t = text.trim().trim_start_matches(['v', '=']);
    let mut nums = Vec::new();
    let mut wild = false;
    for part in t.split('.') {
        if wild || part.is_empty() {
            return None;
        }
        match part.parse::<u32>() {
            Ok(n) => nums.push(n),
            Err(_) if matches!(part, "x" | "X" | "*") => wild = true,
            Err(_) => return None,
        }
    }
    (!nums.is_empty() && nums.len() <= 3).then_some(nums)
}

fn read_small(path: &Path) -> Option<String> {
    let mut s = String::new();
    fs::File::open(path)
        .ok()?
        .take(64 * 1024)
        .read_to_string(&mut s)
        .ok()?;
    Some(s)
}

/// The Node versions a project asks for, as parsed pins.
fn project_pins(project: &Path) -> Vec<Vec<u32>> {
    let mut texts: Vec<String> = Vec::new();
    for f in [".nvmrc", ".node-version"] {
        if let Some(s) = read_small(&project.join(f)) {
            texts.extend(s.lines().next().map(str::to_string));
        }
    }
    if let Some(json) = read_small(&project.join("package.json"))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        for path in [["engines", "node"], ["volta", "node"]] {
            if let Some(v) = json
                .get(path[0])
                .and_then(|o| o.get(path[1]))
                .and_then(|v| v.as_str())
            {
                texts.push(v.to_string());
            }
        }
    }
    texts.iter().filter_map(|t| parse_pin(t)).collect()
}

/// Which of `projects` ask for this Node version: by its major, or by the
/// exact version when the project names all three numbers.
pub fn pins_for(version: &str, projects: &[PathBuf]) -> Vec<PathBuf> {
    let Some((a, b, c)) = parse_version(version) else {
        return Vec::new();
    };
    let have = [a, b, c];
    projects
        .iter()
        .filter(|p| {
            project_pins(p)
                .iter()
                .any(|pin| pin.iter().zip(have).all(|(w, h)| *w == h))
        })
        .cloned()
        .collect()
}

/// One row of the Packages & runtimes page, before it becomes a list row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEntry {
    pub section: SectionKind,
    pub entry: ToolEntry,
    /// Taken by a bulk mark of its section (an old, unpinned, non-default
    /// install); everything else is marked row by row.
    pub bulk: bool,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Fnm => "fnm",
            Manager::Nvm => "nvm",
        }
    }
}

/// Key of a Node version in the shared selection.
pub fn version_id(i: &NodeInstall) -> String {
    format!("node:{}:{}", i.manager.name(), i.version)
}

/// Start of the key of every global package of that version.
pub fn globals_prefix(i: &NodeInstall) -> String {
    format!("node-global:{}:{}:", i.manager.name(), i.version)
}

/// A name that may reach a command line: letters, digits and `@ / . _ -`, and
/// not starting with `-` (so it can never read as an option).
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@/._-".contains(c))
}

/// The uninstall command for one Node version, pinned to the manager that
/// installed it; `None` if the manager's program cannot be found.
fn uninstall_version(home: &Path, places: &Places, i: &NodeInstall) -> Option<CommandSpec> {
    match i.manager {
        Manager::Fnm => {
            let exe = resolve_in(home, "fnm", places).into_iter().next()?;
            let base = i.dir.parent()?.parent()?;
            Some(
                pinned(home, &exe, &["uninstall", &i.version])
                    .with_env("FNM_DIR", &base.to_string_lossy()),
            )
        }
        Manager::Nvm => {
            let base = i.dir.ancestors().nth(3)?;
            let script = base.join("nvm.sh");
            if !script.is_file() || !Path::new("/bin/bash").is_file() {
                return None;
            }
            // The version arrives as `$1`, never spliced into the script.
            Some(
                CommandSpec::new(
                    "/bin/bash",
                    &[
                        "-c",
                        "source \"$NVM_DIR/nvm.sh\" && nvm uninstall \"$1\"",
                        "nvm",
                        &i.version,
                    ],
                )
                .with_cwd(home)
                .with_env("NVM_DIR", &base.to_string_lossy()),
            )
        }
    }
}

/// `npm uninstall -g`, run by the npm that belongs to this Node version (it
/// takes its prefix from where it lives).
fn uninstall_global(home: &Path, i: &NodeInstall, name: &str) -> Option<CommandSpec> {
    let npm = i.root.join("bin/npm");
    (safe_name(name) && is_executable(&npm)).then(|| pinned(home, &npm, &["uninstall", "-g", name]))
}

/// Every Node version and global package fnm and nvm hold, as page rows.
/// `projects` are the project folders to check for `.nvmrc` style pins.
pub(crate) fn discover_runtimes(
    home: &Path,
    places: &Places,
    projects: &[PathBuf],
) -> Vec<RuntimeEntry> {
    let installs = node_installs(home, places);
    // "Old" means older than the default, or than the newest when there is none.
    let reference = installs
        .iter()
        .find(|i| i.is_default)
        .or_else(|| installs.first())
        .and_then(|i| parse_version(&i.version));
    let mut out = Vec::new();
    for i in &installs {
        let pins = pins_for(&i.version, projects);
        let old = reference.is_some_and(|r| parse_version(&i.version).is_some_and(|v| v < r));
        let mut notes: Vec<String> = Vec::new();
        if i.is_default {
            notes.push("the default version".to_string());
        }
        if old {
            notes.push("older than the default".to_string());
        }
        if !pins.is_empty() {
            let names: Vec<String> = pins
                .iter()
                .take(3)
                .map(|p| {
                    p.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                })
                .collect();
            let more = pins.len().saturating_sub(3);
            notes.push(format!(
                "asked for by {} project(s): {}{}",
                pins.len(),
                names.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        let (mode, tail) = match uninstall_version(home, places, i) {
            Some(spec) => (Mode::Command(spec), None),
            None => (
                Mode::Folder,
                Some(format!(
                    "{} was not found: the folder is moved instead",
                    i.manager.name()
                )),
            ),
        };
        notes.extend(tail);
        let group = format!("Node ({})", i.manager.name());
        out.push(RuntimeEntry {
            section: SectionKind::NodeVersions,
            bulk: old && !i.is_default && pins.is_empty(),
            entry: ToolEntry {
                id: version_id(i),
                tool: group,
                label: i.version.clone(),
                path: i.dir.clone(),
                regen: format!("reinstall with {} install {}", i.manager.name(), i.version),
                mode,
                note: (!notes.is_empty()).then(|| notes.join("; ")),
            },
        });
        for (name, path) in node_globals(i) {
            let (mode, note) = match uninstall_global(home, i, &name) {
                Some(spec) => (
                    Mode::Command(spec),
                    Some("removed with the version too".to_string()),
                ),
                None => (
                    Mode::Folder,
                    Some("npm was not found for this version: the folder is moved".to_string()),
                ),
            };
            out.push(RuntimeEntry {
                section: SectionKind::NodeGlobals,
                bulk: false,
                entry: ToolEntry {
                    id: format!("{}{name}", globals_prefix(i)),
                    tool: format!("Node {} ({})", i.version, i.manager.name()),
                    label: name.clone(),
                    path,
                    regen: format!("reinstall with npm install -g {name}"),
                    mode,
                    note,
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn places() -> Places {
        Places {
            path_var: None,
            fnm_dir: None,
            nvm_dir: None,
            system: Vec::new(),
        }
    }

    fn mk(p: &Path) {
        fs::create_dir_all(p).unwrap();
    }

    fn fnm_home() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        let base = home.join(".local/share/fnm");
        mk(&base.join("node-versions/v20.9.0/installation/bin"));
        mk(&base.join("node-versions/v22.1.0/installation/bin"));
        mk(&base.join("node-versions/v22.10.0/installation/bin"));
        mk(&base.join("aliases"));
        symlink(
            base.join("node-versions/v22.10.0/installation"),
            base.join("aliases/default"),
        )
        .unwrap();
        (d, home)
    }

    #[test]
    fn lists_fnm_versions_newest_first_and_marks_the_default() {
        let (_d, home) = fnm_home();
        let got = node_installs(&home, &places());
        let names: Vec<&str> = got.iter().map(|i| i.version.as_str()).collect();
        assert_eq!(names, ["v22.10.0", "v22.1.0", "v20.9.0"]);
        assert!(got[0].is_default && !got[1].is_default && !got[2].is_default);
        assert!(got[0].root.ends_with("v22.10.0/installation"));
        assert!(got[0].dir.ends_with("v22.10.0"));
    }

    #[test]
    fn a_dangling_default_alias_marks_nothing_and_odd_folders_are_skipped() {
        let (_d, home) = fnm_home();
        let base = home.join(".local/share/fnm");
        fs::remove_file(base.join("aliases/default")).unwrap();
        symlink(base.join("gone"), base.join("aliases/default")).unwrap();
        mk(&base.join("node-versions/.downloads"));
        fs::write(base.join("node-versions/v18.0.0"), "a file").unwrap();
        let got = node_installs(&home, &places());
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|i| !i.is_default));
    }

    #[test]
    fn lists_nvm_versions_with_the_default_from_its_alias_file() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        mk(&home.join(".nvm/versions/node/v18.2.0/bin"));
        mk(&home.join(".nvm/versions/node/v20.1.0/bin"));
        mk(&home.join(".nvm/alias"));
        fs::write(home.join(".nvm/alias/default"), "v18.2.0\n").unwrap();
        let got = node_installs(home, &places());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].version, "v20.1.0");
        assert!(got[1].is_default);
        assert_eq!(got[1].manager, Manager::Nvm);
        assert_eq!(got[1].root, got[1].dir);
    }

    #[test]
    fn no_managers_means_no_installs() {
        let d = tempfile::tempdir().unwrap();
        assert!(node_installs(d.path(), &places()).is_empty());
    }

    #[test]
    fn globals_skip_bundled_packages_and_join_scoped_names() {
        let (_d, home) = fnm_home();
        let inst = node_installs(&home, &places()).remove(0);
        let nm = inst.root.join("lib/node_modules");
        for p in ["npm", "corepack", "left-pad", "@scope/tool", "@scope/other"] {
            mk(&nm.join(p));
        }
        fs::write(nm.join(".package-lock.json"), "{}").unwrap();
        let names: Vec<String> = node_globals(&inst).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["@scope/other", "@scope/tool", "left-pad"]);
    }

    #[test]
    fn a_version_without_global_packages_has_none() {
        let (_d, home) = fnm_home();
        let inst = node_installs(&home, &places()).remove(0);
        assert!(node_globals(&inst).is_empty());
    }

    fn project(d: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let p = d.join(name);
        mk(&p);
        for (f, c) in files {
            fs::write(p.join(f), c).unwrap();
        }
        p
    }

    #[test]
    fn pins_are_read_from_every_kind_of_file() {
        let d = tempfile::tempdir().unwrap();
        let a = project(d.path(), "a", &[(".nvmrc", "v22\n")]);
        let b = project(d.path(), "b", &[(".node-version", "22.1.0")]);
        let c = project(
            d.path(),
            "c",
            &[("package.json", r#"{"engines":{"node":"22.x"}}"#)],
        );
        let e = project(
            d.path(),
            "e",
            &[("package.json", r#"{"volta":{"node":"20.9.0"}}"#)],
        );
        let all = vec![a.clone(), b.clone(), c.clone(), e.clone()];
        assert_eq!(pins_for("v22.1.0", &all), [a.clone(), b, c.clone()]);
        assert_eq!(pins_for("v22.10.0", &all), [a, c]);
        assert_eq!(pins_for("v20.9.0", &all), [e]);
        assert!(pins_for("v18.0.0", &all).is_empty());
    }

    #[test]
    fn unreadable_or_vague_pins_match_nothing() {
        let d = tempfile::tempdir().unwrap();
        let mut all = Vec::new();
        for (i, (file, text)) in [
            (".nvmrc", "lts/*"),
            (".nvmrc", "node"),
            (".nvmrc", ""),
            (".nvmrc", ">=18"),
            (".node-version", "^22.1.0"),
            ("package.json", "not json"),
            ("package.json", r#"{"engines":{"node":">=18 <23"}}"#),
            ("package.json", r#"{"engines":{"node":"22 || 24"}}"#),
            ("package.json", r#"{"engines":7}"#),
        ]
        .iter()
        .enumerate()
        {
            all.push(project(d.path(), &format!("p{i}"), &[(file, text)]));
        }
        let bin = project(d.path(), "bin", &[]);
        fs::write(bin.join(".nvmrc"), [0xff, 0xfe, 0x00]).unwrap();
        all.push(bin);
        all.push(d.path().join("missing"));
        assert!(pins_for("v22.1.0", &all).is_empty());
        assert!(pins_for("garbage", &all).is_empty());
    }

    #[test]
    fn a_two_number_pin_needs_the_minor_too() {
        let d = tempfile::tempdir().unwrap();
        let p = project(d.path(), "p", &[(".nvmrc", "22.1")]);
        assert_eq!(pins_for("v22.1.5", std::slice::from_ref(&p)), vec![p.clone()]);
        assert!(pins_for("v22.2.0", &[p]).is_empty());
    }
}
