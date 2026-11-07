//! Installed language runtimes and the packages installed globally into them.
//! This module only reads folders and small project files: it never runs a
//! program (commands come later, from the entries built on top of it).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::command::{CommandRunner, CommandSpec, is_executable};
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
    corepack_versions(home, &mut out);
    uv_pythons(home, places, &mut out);
    uv_tools(home, places, &mut out);
    rust_toolchains(home, places, &mut out);
    cargo_binaries(home, places, &mut out);
    android_sdk(home, &mut out);
    out
}

fn plain(home: &Path, exe: &Path, args: &[&str]) -> CommandSpec {
    CommandSpec::new(&exe.to_string_lossy(), args).with_cwd(home)
}

/// A program of a tool that installs under the home folder: its own bin
/// folder first, then wherever `PATH` and the version managers have it.
fn find_exe(home: &Path, places: &Places, rel: &str, name: &str) -> Option<PathBuf> {
    let own = home.join(rel).join(name);
    if is_executable(&own) {
        return Some(own);
    }
    resolve_in(home, name, places).into_iter().next()
}

/// Real (not symlinked) sub-folders of `dir` whose names may reach a command.
fn real_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| safe_name(n))
        .collect();
    out.sort();
    out
}

struct Item<'a> {
    section: SectionKind,
    id: String,
    group: &'a str,
    label: String,
    path: PathBuf,
    regen: String,
    spec: Option<CommandSpec>,
    note: Option<String>,
}

fn push(out: &mut Vec<RuntimeEntry>, i: Item) {
    out.push(RuntimeEntry {
        section: i.section,
        bulk: false,
        entry: ToolEntry {
            id: i.id,
            tool: i.group.to_string(),
            label: i.label,
            path: i.path,
            regen: i.regen,
            mode: i.spec.map_or(Mode::Folder, Mode::Command),
            note: i.note,
        },
    });
}

/// Pythons that uv installed. The short names (`cpython-3.12-...`) are links
/// to the full ones, so only the real folders are listed.
fn uv_pythons(home: &Path, places: &Places, out: &mut Vec<RuntimeEntry>) {
    let uv = find_exe(home, places, ".local/bin", "uv");
    for (name, path) in real_dirs(&home.join(".local/share/uv/python")) {
        if name.starts_with('.') {
            continue;
        }
        let spec = uv
            .as_ref()
            .map(|u| plain(home, u, &["python", "uninstall", &name]));
        push(
            out,
            Item {
                section: SectionKind::PythonVersions,
                id: format!("uvpy:{name}"),
                group: "Python (uv)",
                label: name.clone(),
                path,
                regen: format!("reinstall with uv python install {name}"),
                spec,
                note: Some("virtual environments built on it stop working".to_string()),
            },
        );
    }
}

/// Command line tools that `uv tool install` put in their own environments.
fn uv_tools(home: &Path, places: &Places, out: &mut Vec<RuntimeEntry>) {
    let uv = find_exe(home, places, ".local/bin", "uv");
    for (name, path) in real_dirs(&home.join(".local/share/uv/tools")) {
        let spec = uv
            .as_ref()
            .map(|u| plain(home, u, &["tool", "uninstall", &name]));
        push(
            out,
            Item {
                section: SectionKind::UvTools,
                id: format!("uvtool:{name}"),
                group: "uv tools",
                label: name.clone(),
                path,
                regen: format!("reinstall with uv tool install {name}"),
                spec,
                note: None,
            },
        );
    }
}

/// The toolchain `rustup` uses when nothing overrides it.
fn rustup_default(home: &Path) -> Option<String> {
    let text = read_small(&home.join(".rustup/settings.toml"))?;
    text.lines().find_map(|l| {
        let v = l.trim().strip_prefix("default_toolchain")?;
        let v = v.trim().strip_prefix('=')?.trim().trim_matches('"');
        Some(v.to_string())
    })
}

fn rust_toolchains(home: &Path, places: &Places, out: &mut Vec<RuntimeEntry>) {
    let rustup = find_exe(home, places, ".cargo/bin", "rustup");
    let default = rustup_default(home);
    for (name, path) in real_dirs(&home.join(".rustup/toolchains")) {
        let is_default = default.as_deref() == Some(name.as_str());
        let spec = rustup
            .as_ref()
            .map(|r| plain(home, r, &["toolchain", "uninstall", &name]));
        push(
            out,
            Item {
                section: SectionKind::RustToolchains,
                id: format!("rustup:{name}"),
                group: "Rust (rustup)",
                label: name.clone(),
                path,
                regen: format!("reinstall with rustup toolchain install {name}"),
                spec,
                note: is_default.then(|| "the default toolchain".to_string()),
            },
        );
    }
}

/// Programs that `cargo install` built, from cargo's own record of them.
fn cargo_binaries(home: &Path, places: &Places, out: &mut Vec<RuntimeEntry>) {
    let Some(json) = read_small(&home.join(".cargo/.crates2.json"))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return;
    };
    let Some(installs) = json.get("installs").and_then(|v| v.as_object()) else {
        return;
    };
    let cargo = find_exe(home, places, ".cargo/bin", "cargo");
    let mut keys: Vec<&String> = installs.keys().collect();
    keys.sort();
    for key in keys {
        let mut words = key.split(' ');
        let (Some(name), Some(version)) = (words.next(), words.next()) else {
            continue;
        };
        let bin = installs[key]
            .get("bins")
            .and_then(|b| b.as_array())
            .and_then(|b| b.first())
            .and_then(|b| b.as_str());
        let Some(bin) = bin.filter(|b| safe_name(b) && !b.contains('/')) else {
            continue;
        };
        let path = home.join(".cargo/bin").join(bin);
        if !path.is_file() || !safe_name(name) {
            continue;
        }
        let spec = cargo.as_ref().map(|c| plain(home, c, &["uninstall", name]));
        push(
            out,
            Item {
                section: SectionKind::CargoBinaries,
                id: format!("cargo:{name}"),
                group: "Cargo",
                label: format!("{name} {version}"),
                path,
                regen: format!("reinstall with cargo install {name}"),
                spec,
                note: None,
            },
        );
    }
}

/// Package manager versions Corepack downloaded (pnpm, yarn). Corepack can
/// only clear everything, so each version is a folder to move; it downloads
/// the one a project asks for again.
fn corepack_versions(home: &Path, out: &mut Vec<RuntimeEntry>) {
    for (manager, dir) in real_dirs(&home.join(".cache/node/corepack/v1")) {
        for (version, path) in real_dirs(&dir) {
            push(
                out,
                Item {
                    section: SectionKind::PackageManagers,
                    id: format!("corepack:{manager}:{version}"),
                    group: &manager,
                    label: version.clone(),
                    path,
                    regen: "downloaded again when a project asks for it (needs network)"
                        .to_string(),
                    spec: None,
                    note: None,
                },
            );
        }
    }
}

/// A simulator id is a UUID: hex digits and dashes only.
fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && s.matches('-').count() == 4
}

/// `com.apple.CoreSimulator.SimRuntime.iOS-17-2` as `iOS 17.2`.
fn runtime_title(key: &str) -> String {
    let id = key.rsplit('.').next().unwrap_or(key);
    let mut parts = id.split('-');
    let name = parts.next().unwrap_or(id);
    let version: Vec<&str> = parts.collect();
    if version.is_empty() {
        name.to_string()
    } else {
        format!("{name} {}", version.join("."))
    }
}

/// iOS simulator devices, from `simctl list -j devices` (slow: it starts
/// CoreSimulator, so this runs once on the discovery thread). A running
/// device is left out: it has to be shut down before it can be removed.
pub fn discover_simulators(home: &Path, runner: &dyn CommandRunner) -> Vec<RuntimeEntry> {
    let xcrun = Path::new("/usr/bin/xcrun");
    if !is_executable(xcrun) {
        return Vec::new();
    }
    let spec =
        CommandSpec::new("/usr/bin/xcrun", &["simctl", "list", "-j", "devices"]).with_cwd(home);
    let Some(json) = runner
        .query(&spec)
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    else {
        return Vec::new();
    };
    let Some(runtimes) = json.get("devices").and_then(|d| d.as_object()) else {
        return Vec::new();
    };
    let base = home.join("Library/Developer/CoreSimulator/Devices");
    let mut out = Vec::new();
    let mut keys: Vec<&String> = runtimes.keys().collect();
    keys.sort();
    for key in keys {
        let group = runtime_title(key);
        for d in runtimes[key].as_array().into_iter().flatten() {
            let (Some(udid), Some(name)) = (
                d.get("udid").and_then(|v| v.as_str()),
                d.get("name").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let booted = d.get("state").and_then(|v| v.as_str()) == Some("Booted");
            let path = base.join(udid);
            if !is_uuid(udid) || booted || !path.is_dir() {
                continue;
            }
            let usable = d.get("isAvailable").and_then(|v| v.as_bool()) != Some(false);
            out.push(RuntimeEntry {
                section: SectionKind::Simulators,
                bulk: false,
                entry: ToolEntry {
                    id: format!("sim:{udid}"),
                    tool: group.clone(),
                    label: name.to_string(),
                    path,
                    regen: "create it again in Xcode (its apps and data are lost)".to_string(),
                    mode: Mode::Command(plain(home, xcrun, &["simctl", "delete", udid])),
                    note: (!usable).then(|| {
                        "unavailable: its runtime is gone, nothing can start it".to_string()
                    }),
                },
            });
        }
    }
    out
}

/// The `sdkmanager` of the Android SDK in `sdk`, newest command-line tools first.
fn sdkmanager(sdk: &Path) -> Option<PathBuf> {
    let tools = sdk.join("cmdline-tools");
    let mut dirs: Vec<PathBuf> = vec![tools.join("latest")];
    if let Ok(rd) = fs::read_dir(&tools) {
        let mut rest: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        rest.sort();
        rest.reverse();
        dirs.extend(rest);
    }
    dirs.into_iter()
        .map(|d| d.join("bin/sdkmanager"))
        .find(|p| is_executable(p))
}

/// The package id an SDK component folder says it is (`platforms;android-34`),
/// read from the `path` of its `package.xml`.
fn sdk_package_id(dir: &Path) -> Option<String> {
    let text = read_small(&dir.join("package.xml"))?;
    let rest = text.split("<localPackage path=\"").nth(1)?;
    let id = rest.split('"').next()?;
    (id.chars()
        .all(|c| c.is_ascii_alphanumeric() || "._;-".contains(c))
        && !id.starts_with('-')
        && !id.is_empty())
    .then(|| id.to_string())
}

/// SDK components: platforms, build-tools, ndk, cmake, sources and system
/// images. `cmdline-tools` and `platform-tools` are what manage the SDK and
/// are not listed.
fn android_sdk(home: &Path, out: &mut Vec<RuntimeEntry>) {
    let sdk = home.join("Library/Android/sdk");
    let manager = sdkmanager(&sdk);
    for (category, depth) in [
        ("platforms", 1),
        ("build-tools", 1),
        ("ndk", 1),
        ("cmake", 1),
        ("sources", 1),
        ("system-images", 3),
    ] {
        let mut level = vec![sdk.join(category)];
        for _ in 0..depth {
            level = level
                .iter()
                .flat_map(|d| real_dirs(d).into_iter().map(|(_, p)| p))
                .collect();
        }
        for dir in level {
            let Some(id) = sdk_package_id(&dir) else {
                continue;
            };
            if !id.starts_with(&format!("{category};")) {
                continue;
            }
            let label = id
                .split_once(';')
                .map_or(id.clone(), |(_, rest)| rest.replace(';', " / "));
            let spec = manager.as_ref().map(|m| {
                plain(
                    home,
                    m,
                    &[&format!("--sdk_root={}", sdk.display()), "--uninstall", &id],
                )
            });
            let note = spec
                .is_none()
                .then(|| "sdkmanager was not found: the folder is moved instead".to_string());
            push(
                out,
                Item {
                    section: SectionKind::AndroidSdk,
                    id: format!("android:{id}"),
                    group: category,
                    label,
                    path: dir,
                    regen: format!("reinstall with sdkmanager \"{id}\""),
                    spec,
                    note,
                },
            );
        }
    }
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
        assert_eq!(
            pins_for("v22.1.5", std::slice::from_ref(&p)),
            vec![p.clone()]
        );
        assert!(pins_for("v22.2.0", &[p]).is_empty());
    }

    fn exe(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn entries(home: &Path, section: SectionKind) -> Vec<RuntimeEntry> {
        discover_runtimes(home, &places(), &[])
            .into_iter()
            .filter(|r| r.section == section)
            .collect()
    }

    fn command(r: &RuntimeEntry) -> Vec<String> {
        match &r.entry.mode {
            Mode::Command(spec) => spec.args.clone(),
            Mode::Folder => panic!("not a command: {:?}", r.entry.id),
        }
    }

    #[test]
    fn uv_pythons_list_real_folders_only_and_uninstall_by_full_name() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        exe(&home.join(".local/bin/uv"));
        let base = home.join(".local/share/uv/python");
        mk(&base.join("cpython-3.12.13-macos-x86_64-none"));
        mk(&base.join(".temp"));
        symlink(
            base.join("cpython-3.12.13-macos-x86_64-none"),
            base.join("cpython-3.12-macos-x86_64-none"),
        )
        .unwrap();
        let got = entries(home, SectionKind::PythonVersions);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].entry.id, "uvpy:cpython-3.12.13-macos-x86_64-none");
        assert_eq!(
            command(&got[0]),
            ["python", "uninstall", "cpython-3.12.13-macos-x86_64-none"]
        );
    }

    #[test]
    fn without_uv_a_python_is_a_plain_folder() {
        let d = tempfile::tempdir().unwrap();
        mk(&d.path().join(".local/share/uv/python/cpython-3.11.1-x"));
        let got = entries(d.path(), SectionKind::PythonVersions);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].entry.mode, Mode::Folder));
    }

    #[test]
    fn uv_tools_and_rust_toolchains_have_their_own_commands() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        exe(&home.join(".local/bin/uv"));
        exe(&home.join(".cargo/bin/rustup"));
        mk(&home.join(".local/share/uv/tools/graphifyy"));
        mk(&home.join(".rustup/toolchains/stable-x86_64-apple-darwin"));
        mk(&home.join(".rustup/toolchains/nightly-2026-01-01-x86_64-apple-darwin"));
        fs::write(
            home.join(".rustup/settings.toml"),
            "version = \"12\"\ndefault_toolchain = \"stable-x86_64-apple-darwin\"\n",
        )
        .unwrap();
        let tools = entries(home, SectionKind::UvTools);
        assert_eq!(command(&tools[0]), ["tool", "uninstall", "graphifyy"]);
        let rust = entries(home, SectionKind::RustToolchains);
        assert_eq!(rust.len(), 2);
        let stable = rust
            .iter()
            .find(|r| r.entry.label.starts_with("stable"))
            .unwrap();
        assert_eq!(stable.entry.note.as_deref(), Some("the default toolchain"));
        assert_eq!(
            command(stable),
            ["toolchain", "uninstall", "stable-x86_64-apple-darwin"]
        );
        let nightly = rust
            .iter()
            .find(|r| r.entry.label.starts_with("nightly"))
            .unwrap();
        assert!(nightly.entry.note.is_none());
    }

    #[test]
    fn cargo_binaries_come_from_cargos_own_record_and_skip_odd_entries() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        exe(&home.join(".cargo/bin/cargo"));
        exe(&home.join(".cargo/bin/rg"));
        exe(&home.join(".cargo/bin/rustc")); // a rustup proxy: not in the record
        fs::write(
            home.join(".cargo/.crates2.json"),
            r#"{"installs":{
              "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"]},
              "gone 1.0.0 (registry+x)":{"bins":["missing"]},
              "evil 1.0.0 (registry+x)":{"bins":["../../etc/passwd"]},
              "-rf 1.0.0 (registry+x)":{"bins":["rg"]},
              "nobins 1.0.0 (registry+x)":{}
            }}"#,
        )
        .unwrap();
        let got = entries(home, SectionKind::CargoBinaries);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].entry.label, "ripgrep 14.1.0");
        assert_eq!(command(&got[0]), ["uninstall", "ripgrep"]);
        assert!(got[0].entry.path.ends_with(".cargo/bin/rg"));
    }

    #[test]
    fn a_broken_crates_file_lists_nothing() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(".cargo")).unwrap();
        fs::write(d.path().join(".cargo/.crates2.json"), "{ nope").unwrap();
        assert!(entries(d.path(), SectionKind::CargoBinaries).is_empty());
    }

    #[test]
    fn corepack_versions_are_folder_rows_per_manager() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        mk(&home.join(".cache/node/corepack/v1/pnpm/12.18.0"));
        mk(&home.join(".cache/node/corepack/v1/yarn/4.1.0"));
        fs::write(
            home.join(".cache/node/corepack/v1/lastKnownGood.json"),
            "{}",
        )
        .unwrap();
        let got = entries(home, SectionKind::PackageManagers);
        let ids: Vec<&str> = got.iter().map(|r| r.entry.id.as_str()).collect();
        assert_eq!(ids, ["corepack:pnpm:12.18.0", "corepack:yarn:4.1.0"]);
        assert!(got.iter().all(|r| matches!(r.entry.mode, Mode::Folder)));
    }

    #[test]
    fn an_empty_home_has_no_runtime_rows() {
        let d = tempfile::tempdir().unwrap();
        assert!(discover_runtimes(d.path(), &places(), &[]).is_empty());
    }

    fn pkg(dir: &Path, id: &str) {
        mk(dir);
        fs::write(
            dir.join("package.xml"),
            format!("<?xml version=\"1.0\"?><repo><localPackage path=\"{id}\" obsolete=\"false\"/></repo>"),
        )
        .unwrap();
    }

    #[test]
    fn android_components_use_their_package_id_and_skip_the_sdk_manager_itself() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let sdk = home.join("Library/Android/sdk");
        exe(&sdk.join("cmdline-tools/latest/bin/sdkmanager"));
        pkg(&sdk.join("platforms/android-34"), "platforms;android-34");
        pkg(&sdk.join("build-tools/34.0.0"), "build-tools;34.0.0");
        pkg(
            &sdk.join("system-images/android-34/google_apis/arm64-v8a"),
            "system-images;android-34;google_apis;arm64-v8a",
        );
        pkg(&sdk.join("platform-tools"), "platform-tools");
        pkg(&sdk.join("cmdline-tools/latest"), "cmdline-tools;latest");
        pkg(&sdk.join("platforms/android-35"), "platforms;android 35"); // a space: refused
        pkg(&sdk.join("platforms/android-36"), "build-tools;1.0"); // wrong category
        mk(&sdk.join("platforms/android-27")); // no package.xml
        let got = entries(home, SectionKind::AndroidSdk);
        let ids: Vec<&str> = got.iter().map(|r| r.entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "android:platforms;android-34",
                "android:build-tools;34.0.0",
                "android:system-images;android-34;google_apis;arm64-v8a"
            ]
        );
        let args = command(&got[0]);
        assert_eq!(args[0], format!("--sdk_root={}", sdk.display()));
        assert_eq!(args[1..], ["--uninstall", "platforms;android-34"]);
    }

    #[test]
    fn without_sdkmanager_android_components_are_folders() {
        let d = tempfile::tempdir().unwrap();
        let sdk = d.path().join("Library/Android/sdk");
        pkg(&sdk.join("ndk/28.2.1"), "ndk;28.2.1");
        let got = entries(d.path(), SectionKind::AndroidSdk);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].entry.mode, Mode::Folder));
    }

    const SIMS: &str = r#"{"devices":{
      "com.apple.CoreSimulator.SimRuntime.iOS-17-2":[
        {"udid":"11111111-2222-3333-4444-555555555555","name":"iPhone 15","state":"Shutdown","isAvailable":true,"dataPathSize":13254656},
        {"udid":"AAAAAAAA-2222-3333-4444-555555555555","name":"iPhone 15 Pro","state":"Booted","isAvailable":true},
        {"udid":"../../etc","name":"evil","state":"Shutdown"}],
      "com.apple.CoreSimulator.SimRuntime.iOS-12-0":[
        {"udid":"BBBBBBBB-2222-3333-4444-555555555555","name":"iPhone XR","state":"Shutdown","isAvailable":false}]}}"#;

    #[test]
    fn simulators_leave_out_running_and_odd_devices_and_delete_by_udid() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        for u in [
            "11111111-2222-3333-4444-555555555555",
            "AAAAAAAA-2222-3333-4444-555555555555",
            "BBBBBBBB-2222-3333-4444-555555555555",
        ] {
            mk(&home.join("Library/Developer/CoreSimulator/Devices").join(u));
        }
        let runner =
            crate::catalog::command::FakeRunner::new(&[]).answering(&[("simctl list", SIMS)]);
        let got = discover_simulators(home, &runner);
        let ids: Vec<&str> = got.iter().map(|r| r.entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "sim:BBBBBBBB-2222-3333-4444-555555555555",
                "sim:11111111-2222-3333-4444-555555555555"
            ]
        );
        assert_eq!(got[1].entry.tool, "iOS 17.2");
        assert_eq!(command(&got[1])[..2], ["simctl", "delete"]);
        assert!(
            got[0]
                .entry
                .note
                .as_deref()
                .unwrap()
                .contains("unavailable")
        );
    }

    #[test]
    fn broken_simctl_output_lists_no_simulators() {
        let d = tempfile::tempdir().unwrap();
        let runner = crate::catalog::command::FakeRunner::new(&[]).answering(&[("simctl", "oops")]);
        assert!(discover_simulators(d.path(), &runner).is_empty());
        let none = crate::catalog::command::FakeRunner::new(&[]);
        assert!(discover_simulators(d.path(), &none).is_empty());
    }
}
