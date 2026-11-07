//! The Caches & Projects list: project artifacts grouped by project, tool
//! caches grouped by tool, each as a [`Row`] with the same numbers the
//! Explorer shows.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::{SortBy, cmp_rows};
use crate::bg::Work;
use crate::catalog::resolve::Places;
use crate::catalog::runtimes::discover_runtimes;
use crate::catalog::split::{SplitKind, split};
use crate::catalog::{ArtifactKind, CommandSpec, Mode, ToolEntry, detect_artifacts};
use crate::filter::Query;
use crate::safety::{Policy, Safety};
use crate::scan::{Node, Row, SpaceView, file_space, locate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionKind {
    Projects,
    Tools,
    NodeVersions,
    NodeGlobals,
}

/// The screen a section is listed on: Caches & Projects or Packages & runtimes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Caches,
    Runtimes,
}

impl SectionKind {
    pub fn page(self) -> Page {
        match self {
            SectionKind::Projects | SectionKind::Tools => Page::Caches,
            SectionKind::NodeVersions | SectionKind::NodeGlobals => Page::Runtimes,
        }
    }

    /// Sections in the order they are listed.
    pub const ALL: [SectionKind; 4] = [
        SectionKind::Projects,
        SectionKind::Tools,
        SectionKind::NodeVersions,
        SectionKind::NodeGlobals,
    ];
}

#[derive(Clone, Debug)]
pub enum RowKind {
    /// A folder that is moved to ToBeDeleted.
    Folder,
    /// Cleaned by the tool's own command (never moved).
    Command { id: String, spec: CommandSpec },
}

/// One row: the Explorer's [`Row`] plus what only this screen knows.
#[derive(Clone)]
pub struct TRow {
    pub row: Row,
    pub kind: RowKind,
    pub safety: Safety,
    /// How it comes back after cleaning.
    pub regen: String,
    pub note: Option<String>,
    /// A final product (a built program) that bulk marks leave alone: it is
    /// deleted only when the user marks that row itself.
    pub kept: bool,
    /// A command row that a bulk mark of its group or section takes (an old
    /// Node version nothing asks for); every other command row is marked alone.
    pub bulk: bool,
}

impl TRow {
    pub fn is_command(&self) -> bool {
        matches!(self.kind, RowKind::Command { .. })
    }
}

#[derive(Clone)]
pub struct Group {
    pub title: String,
    /// The short name the graph shows: the project folder's name, or the tool.
    pub label: String,
    pub subtitle: String,
    pub section: SectionKind,
    pub rows: Vec<TRow>,
    pub open: bool,
}

impl Group {
    /// Stable key for remembering a fold across rebuilds.
    pub fn key(&self) -> String {
        format!("{:?}:{}", self.section, self.title)
    }

    pub fn size(&self) -> u64 {
        self.rows.iter().map(|r| r.row.size).sum()
    }

    pub fn disk(&self) -> u64 {
        self.rows.iter().map(|r| r.row.disk).sum()
    }

    /// Sum of the rows' own Frees (None while any is unknown).
    pub fn frees(&self) -> Option<u64> {
        self.rows.iter().map(|r| r.row.frees).sum()
    }

    /// The group as a pseudo-row, so groups sort by the same keys as rows
    /// and draw their totals in the same columns.
    pub fn as_row(&self) -> Row {
        Row {
            name: self.title.clone().into(),
            path: PathBuf::new(),
            is_dir: true,
            size: self.size(),
            disk: self.disk(),
            files: 0,
            scanning: false,
            node: None,
            space: None,
            frees: self.frees(),
            mtime: self.rows.iter().map(|r| r.row.mtime).max().unwrap_or(0),
        }
    }
}

/// One line of the flattened list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Section(SectionKind),
    Group(usize),
    Row { group: usize, row: usize },
}

#[derive(Clone, Default)]
pub struct Targets {
    pub groups: Vec<Group>,
}

/// A candidate before it becomes a row.
struct Cand {
    path: PathBuf,
    /// The scanned folder; `None` for a single file (a built binary).
    node: Option<Arc<Node>>,
    /// Does this folder split into intermediate files and products?
    split: Option<SplitKind>,
    /// A product: bulk marks leave it alone.
    kept: bool,
    /// Lower wins when two candidates share a path (tools before artifacts).
    priority: u8,
    section: SectionKind,
    group: String,
    label: String,
    subtitle: String,
    /// A workspace member (its project folder is below the group's root).
    member: bool,
    name: String,
    kind: RowKind,
    regen: String,
    note: Option<String>,
    bulk: bool,
}

fn project_kind(k: ArtifactKind) -> &'static str {
    match k {
        ArtifactKind::NodeModules | ArtifactKind::Next | ArtifactKind::Turbo => "Node",
        ArtifactKind::RustTarget => "Rust",
        ArtifactKind::DartTool | ArtifactKind::FlutterBuild => "Flutter",
        ArtifactKind::Pods => "CocoaPods",
        ArtifactKind::XcodeBuild => "Xcode",
        ArtifactKind::GradleBuild | ArtifactKind::GradleCache => "Gradle",
        ArtifactKind::MavenTarget => "Maven",
        ArtifactKind::SwiftBuild => "Swift",
        ArtifactKind::PythonBuild | ArtifactKind::PythonDist => "Python",
    }
}

/// `~/dev/shop` for paths under home, the full path otherwise.
pub fn display_path(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.to_string_lossy()),
        Err(_) => p.to_string_lossy().into_owned(),
    }
}

/// A build folder that splits becomes one candidate per part; anything else
/// (or a folder with nothing recognisable inside) stays as it is.
fn expand_split(c: Cand) -> Vec<Cand> {
    let parts = match (&c.split, &c.node, &c.kind) {
        (Some(kind), Some(node), RowKind::Folder) => split(*kind, node),
        _ => Vec::new(),
    };
    if parts.is_empty() {
        return vec![c];
    }
    parts
        .into_iter()
        .map(|p| Cand {
            // A project's rows are named from the project root; a tool's rows
            // from the tool's own folder.
            name: if c.section == SectionKind::Projects {
                format!("{}/{}", c.name, p.name)
            } else {
                p.name
            },
            regen: if p.kept {
                "a finished product, kept unless you mark this row; rebuilt by the next build"
                    .to_string()
            } else {
                "intermediate files, recreated by the next build".to_string()
            },
            path: p.path,
            node: p.node,
            split: None,
            kept: p.kept,
            priority: c.priority,
            section: c.section,
            group: c.group.clone(),
            label: c.label.clone(),
            subtitle: c.subtitle.clone(),
            member: c.member,
            kind: RowKind::Folder,
            note: c.note.clone(),
            bulk: false,
        })
        .collect()
}

/// The Explorer-style row for a candidate: a scanned folder, or a single file.
fn row_for(c: &Cand, space: Option<crate::scan::Space>) -> Option<Row> {
    match &c.node {
        Some(node) => Some(Row {
            name: c.name.clone().into(),
            path: c.path.clone(),
            is_dir: true,
            size: node.size(),
            disk: space.map_or_else(|| node.disk(), |s| s.footprint),
            files: node.files(),
            scanning: !node.is_done(),
            node: Some(Arc::clone(node)),
            space,
            frees: space.map(|s| s.frees_now),
            mtime: node.mtime(),
        }),
        None => {
            use std::os::unix::fs::MetadataExt;
            let md = std::fs::symlink_metadata(&c.path).ok()?;
            let sp = file_space(&c.path).ok();
            Some(Row {
                name: c.name.clone().into(),
                path: c.path.clone(),
                is_dir: false,
                size: md.len(),
                disk: sp.map_or(md.blocks() * 512, |s| s.footprint),
                files: 1,
                scanning: false,
                node: None,
                space: sp,
                frees: sp.map(|s| s.frees_now),
                mtime: md.mtime(),
            })
        }
    }
}

/// Everything a list build reads, gathered so it can move to a thread.
pub struct BuildSource {
    pub root: Arc<Node>,
    pub home: PathBuf,
    pub view: SpaceView,
    /// Was the scan finished when the build was asked for?
    pub scan_idle: bool,
    pub policy: Arc<Policy>,
    pub entries: Vec<ToolEntry>,
    pub closed: HashSet<String>,
}

impl Targets {
    pub fn build(src: &BuildSource, work: &Work) -> Targets {
        let BuildSource {
            root,
            home,
            view,
            scan_idle,
            policy,
            entries,
            closed,
        } = src;
        let (home, scan_idle) = (home.as_path(), *scan_idle);
        work.stage(0, 3, "listing tool caches");
        let mut cands: Vec<Cand> = Vec::new();
        for e in entries {
            let Some(node) = locate(root, &e.path) else {
                continue;
            };
            cands.push(Cand {
                path: e.path.clone(),
                node: Some(node),
                split: (e.id == "xcode-derived" && e.mode == Mode::Folder)
                    .then_some(SplitKind::DerivedData),
                kept: false,
                priority: 0,
                section: SectionKind::Tools,
                group: e.tool.clone(),
                label: e.tool.clone(),
                subtitle: String::new(),
                member: false,
                name: e.label.clone(),
                kind: match &e.mode {
                    Mode::Folder => RowKind::Folder,
                    Mode::Command(spec) => RowKind::Command {
                        id: e.id.clone(),
                        spec: spec.clone(),
                    },
                },
                regen: e.regen.clone(),
                note: e.note.clone(),
                bulk: false,
            });
        }
        work.stage(1, 3, "finding project folders");
        let artifacts = detect_artifacts(root);
        let mut project_roots: Vec<PathBuf> = artifacts.iter().map(|c| c.root.clone()).collect();
        project_roots.sort();
        project_roots.dedup();
        for c in artifacts {
            cands.push(Cand {
                // Named relative to the root project: "apps/web/node_modules".
                name: c.path.strip_prefix(&c.root).map_or_else(
                    |_| c.kind.label().to_string(),
                    |rel| rel.to_string_lossy().into_owned(),
                ),
                path: c.path,
                node: Some(c.node),
                split: c.kind.split_kind(),
                kept: c.kind.keeps_by_default(),
                priority: 1,
                section: SectionKind::Projects,
                group: display_path(home, &c.root),
                label: c
                    .root
                    .file_name()
                    .map_or_else(|| "~".to_string(), |n| n.to_string_lossy().into_owned()),
                subtitle: project_kind(c.kind).to_string(),
                member: c.project != c.root,
                kind: RowKind::Folder,
                regen: c.kind.regen().to_string(),
                note: None,
                bulk: false,
            });
        }
        // Installed runtimes and their packages (the Packages & runtimes page).
        for r in discover_runtimes(home, &Places::from_env(), &project_roots) {
            let e = r.entry;
            let Some(node) = locate(root, &e.path) else {
                continue;
            };
            cands.push(Cand {
                path: e.path,
                node: Some(node),
                split: None,
                kept: false,
                priority: 0,
                section: r.section,
                label: e.tool.clone(),
                group: e.tool,
                subtitle: String::new(),
                member: false,
                name: e.label,
                kind: match e.mode {
                    Mode::Folder => RowKind::Folder,
                    Mode::Command(spec) => RowKind::Command { id: e.id, spec },
                },
                regen: e.regen,
                note: e.note,
                bulk: r.bulk,
            });
        }

        // One folder, one row: the outer folder wins, and on the same path
        // the tool registry wins over a project artifact.
        cands.retain(|c| policy.classify(&c.path) != Safety::Protected);
        cands.sort_by(|a, b| {
            a.path
                .components()
                .count()
                .cmp(&b.path.components().count())
                .then(a.priority.cmp(&b.priority))
                .then_with(|| a.path.cmp(&b.path))
        });
        // The outer folder only wins within its own page, and a global package
        // sits inside its Node version's folder on purpose.
        let mut kept: HashSet<(Page, PathBuf)> = HashSet::new();
        cands.retain(|c| {
            let page = c.section.page();
            let nested_ok = c.section == SectionKind::NodeGlobals;
            if !nested_ok
                && c.path
                    .ancestors()
                    .any(|a| kept.contains(&(page, a.to_path_buf())))
            {
                return false;
            }
            kept.insert((page, c.path.clone()));
            true
        });

        // Build folders that hold products: one row per intermediate folder,
        // and one per product, so the products can be kept.
        let cands: Vec<Cand> = cands.into_iter().flat_map(expand_split).collect();

        // Numbers: one pass over the clone tables when the scan is idle.
        work.stage(2, 3, "reading space numbers");
        let idle = scan_idle;
        let nodes: Vec<Arc<Node>> = cands.iter().filter_map(|c| c.node.clone()).collect();
        let node_spaces = if idle {
            view.spaces_of(&nodes)
        } else {
            Vec::new()
        };
        // Which entry of `node_spaces` belongs to which candidate.
        let mut next_node = 0usize;
        let spaces: Vec<Option<crate::scan::Space>> = cands
            .iter()
            .map(|c| {
                if c.node.is_some() {
                    next_node += 1;
                    node_spaces.get(next_node - 1).copied()
                } else {
                    None
                }
            })
            .collect();

        let mut groups: Vec<Group> = Vec::new();
        // What each project group holds, to write its subtitle at the end.
        let mut ecos: std::collections::HashMap<
            String,
            (std::collections::BTreeSet<String>, bool),
        > = std::collections::HashMap::new();
        for (i, c) in cands.into_iter().enumerate() {
            let Some(row) = row_for(&c, spaces[i]) else {
                continue; // a file that vanished since the scan
            };
            if c.section == SectionKind::Projects {
                let e = ecos.entry(c.group.clone()).or_default();
                e.0.insert(c.subtitle.clone());
                e.1 |= c.member;
            }
            let trow = TRow {
                row,
                kind: c.kind,
                safety: policy.classify(&c.path),
                regen: c.regen,
                note: c.note,
                kept: c.kept,
                bulk: c.bulk,
            };
            match groups
                .iter_mut()
                .find(|g| g.section == c.section && g.title == c.group)
            {
                Some(g) => g.rows.push(trow),
                None => groups.push(Group {
                    open: !closed.contains(&format!("{:?}:{}", c.section, c.group)),
                    title: c.group,
                    label: c.label,
                    subtitle: c.subtitle,
                    section: c.section,
                    rows: vec![trow],
                }),
            }
        }
        // "Node", "Node + Rust", "Node workspace": what the project holds.
        for g in groups
            .iter_mut()
            .filter(|g| g.section == SectionKind::Projects)
        {
            if let Some((kinds, member)) = ecos.get(&g.title) {
                g.subtitle = kinds.iter().cloned().collect::<Vec<_>>().join(" + ");
                if *member {
                    g.subtitle.push_str(" workspace");
                }
            }
        }
        let mut t = Targets { groups };
        t.sort(SortBy::Disk);
        work.stage(3, 3, "done");
        t
    }

    /// Order rows inside each group, and groups inside each section, by the
    /// same key the Explorer uses. Sections keep their fixed order.
    pub fn sort(&mut self, by: SortBy) {
        for g in &mut self.groups {
            g.rows.sort_by(|a, b| cmp_rows(by, &a.row, &b.row));
        }
        self.groups.sort_by(|a, b| {
            let section = |g: &Group| {
                SectionKind::ALL
                    .iter()
                    .position(|s| *s == g.section)
                    .unwrap_or(0)
            };
            section(a)
                .cmp(&section(b))
                .then_with(|| cmp_rows(by, &a.as_row(), &b.as_row()))
        });
    }

    pub fn row(&self, group: usize, row: usize) -> &TRow {
        &self.groups[group].rows[row]
    }

    pub fn find(&self, path: &Path) -> Option<(usize, usize)> {
        self.groups.iter().enumerate().find_map(|(g, grp)| {
            grp.rows
                .iter()
                .position(|r| r.row.path == path)
                .map(|r| (g, r))
        })
    }

    /// The visible lines: section bands, group headers, and the rows of open
    /// groups.
    ///
    /// A filter works on groups as well as items, whatever the folds say:
    /// - a group whose *name* matches (`uv`, a project's folder name) is shown
    ///   with all its items;
    /// - otherwise only its items that match are shown, and a group with none
    ///   is hidden;
    /// - several terms may be spread over the group name and the item name
    ///   (`mac node_modules`).
    ///
    /// Only the name is matched, not the group's path, so `dev` does not pick
    /// up every project below `~/dev`.
    pub fn lines(&self, filter: &Query, page: Page) -> Vec<Line> {
        let filtering = !filter.is_empty();
        let mut out = Vec::new();
        for section in SectionKind::ALL.into_iter().filter(|s| s.page() == page) {
            let mut body = Vec::new();
            for (gi, g) in self
                .groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.section == section)
            {
                // Each term matches the group's name or the item's name, so a
                // group whose name matches shows all its items, and an
                // exclusion (`!next`) still hides the items it names.
                let hits: Vec<usize> = g
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        !filtering
                            || filter.matches_fields(&[
                                g.label.as_str(),
                                r.row.name.to_string_lossy().as_ref(),
                            ])
                    })
                    .map(|(i, _)| i)
                    .collect();
                if filtering && hits.is_empty() {
                    continue;
                }
                body.push(Line::Group(gi));
                if g.open || filtering {
                    body.extend(hits.into_iter().map(|row| Line::Row { group: gi, row }));
                }
            }
            if !body.is_empty() {
                out.push(Line::Section(section));
                out.extend(body);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SortBy;
    use crate::catalog::CommandSpec;
    use crate::catalog::{Mode, ToolEntry};
    use crate::filter;
    use crate::scan::Scanner;
    use crate::testutil::{clone_file, write};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    struct Sandbox {
        _d: tempfile::TempDir,
        home: PathBuf,
        scanner: Scanner,
        root: Arc<crate::scan::Node>,
        hold: PathBuf,
    }

    fn sandbox(setup: impl FnOnce(&Path)) -> Sandbox {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        setup(&home);
        let scanner = Scanner::new(2);
        let root = scanner.scan_root(&home);
        assert!(scanner.wait_idle(Duration::from_secs(20)));
        let hold = d.path().join("hold");
        Sandbox {
            _d: d,
            home,
            scanner,
            root,
            hold,
        }
    }

    fn entry(home: &Path, id: &str, tool: &str, rel: &str, mode: Mode) -> ToolEntry {
        ToolEntry {
            id: id.to_string(),
            tool: tool.to_string(),
            label: id.to_string(),
            path: home.join(rel),
            regen: "again".to_string(),
            mode,
            note: None,
        }
    }

    fn source(s: &Sandbox, entries: &[ToolEntry], closed: HashSet<String>) -> BuildSource {
        BuildSource {
            root: Arc::clone(&s.root),
            home: s.home.clone(),
            view: s.scanner.view(),
            scan_idle: s.scanner.is_idle(),
            policy: Arc::new(Policy::new(&s.home, &s.hold)),
            entries: entries.to_vec(),
            closed,
        }
    }

    fn build(s: &Sandbox, entries: &[ToolEntry]) -> Targets {
        Targets::build(&source(s, entries, HashSet::new()), &Work::new())
    }

    fn project(home: &Path) {
        write(&home.join("shop/package.json"), 10);
        write(&home.join("shop/node_modules/a/i.js"), 30_000);
        write(&home.join("shop/.next/c"), 10_000);
        write(&home.join("api/Cargo.toml"), 10);
        write(&home.join("api/target/debug/x"), 90_000);
        write(&home.join("Library/Caches/Spotify/blob"), 50_000);
    }

    fn titles(t: &Targets, section: SectionKind) -> Vec<String> {
        t.groups
            .iter()
            .filter(|g| g.section == section)
            .map(|g| g.title.clone())
            .collect()
    }

    #[test]
    fn groups_projects_by_folder_and_tools_by_tool() {
        let s = sandbox(project);
        let e = entry(
            &s.home,
            "spotify",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        let mut p = titles(&t, SectionKind::Projects);
        p.sort();
        assert_eq!(p, ["~/api", "~/shop"]);
        assert_eq!(titles(&t, SectionKind::Tools), ["App caches"]);
        let shop = t.groups.iter().find(|g| g.title == "~/shop").unwrap();
        let mut names: Vec<_> = shop
            .rows
            .iter()
            .map(|r| r.row.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, [".next", "node_modules"]);
        assert_eq!(shop.subtitle, "Node");
    }

    #[test]
    fn groups_have_a_short_label_for_the_graph() {
        let s = sandbox(project);
        let e = entry(
            &s.home,
            "spotify",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        let label = |title: &str| {
            t.groups
                .iter()
                .find(|g| g.title == title)
                .unwrap()
                .label
                .clone()
        };
        assert_eq!(label("~/shop"), "shop");
        assert_eq!(label("~/api"), "api");
        assert_eq!(label("App caches"), "App caches");
    }

    #[test]
    fn workspace_members_are_listed_under_the_root_project() {
        let s = sandbox(|h| {
            write(&h.join("mono/package.json"), 10);
            write(&h.join("mono/pnpm-workspace.yaml"), 10);
            write(&h.join("mono/node_modules/a/i.js"), 30_000);
            write(&h.join("mono/apps/web/package.json"), 10);
            write(&h.join("mono/apps/web/node_modules/a/i.js"), 20_000);
            write(&h.join("mono/src-tauri/Cargo.toml"), 10);
            write(&h.join("mono/src-tauri/target/x"), 10_000);
            write(&h.join("other/package.json"), 10);
            write(&h.join("other/node_modules/a/i.js"), 5_000);
        });
        let t = build(&s, &[]);
        let mut titles = titles(&t, SectionKind::Projects);
        titles.sort();
        assert_eq!(
            titles,
            ["~/mono", "~/other"],
            "members do not get groups of their own"
        );
        let mono = t.groups.iter().find(|g| g.title == "~/mono").unwrap();
        assert_eq!(mono.label, "mono");
        let mut names: Vec<_> = mono
            .rows
            .iter()
            .map(|r| r.row.name.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["apps/web/node_modules", "node_modules", "src-tauri/target"]
        );
        assert!(
            mono.subtitle.contains("Node") && mono.subtitle.contains("Rust"),
            "{}",
            mono.subtitle
        );
        assert!(mono.subtitle.contains("workspace"), "{}", mono.subtitle);
        let other = t.groups.iter().find(|g| g.title == "~/other").unwrap();
        assert_eq!(other.subtitle, "Node");
    }

    fn exe(path: &Path, bytes: usize) {
        use std::os::unix::fs::PermissionsExt;
        write(path, bytes);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn rust_project(h: &Path) {
        write(&h.join("api/Cargo.toml"), 10);
        for profile in ["debug", "release"] {
            write(&h.join(format!("api/target/{profile}/deps/f")), 40_000);
            write(
                &h.join(format!("api/target/{profile}/incremental/f")),
                20_000,
            );
            exe(&h.join(format!("api/target/{profile}/myapp")), 8_000);
        }
        write(&h.join("api/target/release/bundle/dmg/a.dmg"), 9_000);
    }

    fn names_of(t: &Targets, title: &str) -> Vec<(String, bool, bool)> {
        let g = t.groups.iter().find(|g| g.title == title).unwrap();
        let mut v: Vec<_> = g
            .rows
            .iter()
            .map(|r| {
                (
                    r.row.name.to_string_lossy().into_owned(),
                    r.kept,
                    r.row.is_dir,
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_rust_target_lists_intermediate_folders_and_the_binaries_separately() {
        let s = sandbox(rust_project);
        let t = build(&s, &[]);
        assert_eq!(
            names_of(&t, "~/api"),
            [
                ("target/debug/deps".to_string(), false, true),
                ("target/debug/incremental".to_string(), false, true),
                ("target/debug/myapp".to_string(), true, false),
                ("target/release/bundle".to_string(), true, true),
                ("target/release/deps".to_string(), false, true),
                ("target/release/incremental".to_string(), false, true),
                ("target/release/myapp".to_string(), true, false),
            ]
        );
    }

    #[test]
    fn a_binary_row_has_the_files_own_numbers_and_a_kept_marker() {
        let s = sandbox(rust_project);
        let t = build(&s, &[]);
        let g = t.groups.iter().find(|g| g.title == "~/api").unwrap();
        let bin = g
            .rows
            .iter()
            .find(|r| r.row.name == "target/release/myapp")
            .unwrap();
        assert!(!bin.row.is_dir && bin.row.node.is_none());
        assert_eq!(bin.row.size, 8_000);
        assert!(bin.row.disk >= 8_000);
        assert_eq!(
            bin.row.frees,
            Some(bin.row.disk),
            "a plain file frees what it holds"
        );
        assert!(bin.regen.contains("kept"), "{}", bin.regen);
    }

    #[test]
    fn a_target_that_cannot_be_split_is_still_one_row() {
        let s = sandbox(|h| {
            write(&h.join("api/Cargo.toml"), 10);
            write(&h.join("api/target/x"), 10_000);
        });
        let t = build(&s, &[]);
        assert_eq!(names_of(&t, "~/api"), [("target".to_string(), false, true)]);
    }

    #[test]
    fn a_dist_folder_is_one_kept_row() {
        let s = sandbox(|h| {
            write(&h.join("py/pyproject.toml"), 10);
            write(&h.join("py/build/lib/x"), 5_000);
            write(&h.join("py/dist/pkg.whl"), 5_000);
        });
        let t = build(&s, &[]);
        assert_eq!(
            names_of(&t, "~/py"),
            [
                ("build".to_string(), false, true),
                ("dist".to_string(), true, true)
            ]
        );
    }

    #[test]
    fn derived_data_is_split_into_products_and_intermediates() {
        let s = sandbox(|h| {
            write(
                &h.join(
                    "Library/Developer/Xcode/DerivedData/App-abc/Build/Products/Debug/App.app/App",
                ),
                9_000,
            );
            write(
                &h.join(
                    "Library/Developer/Xcode/DerivedData/App-abc/Build/Intermediates.noindex/x",
                ),
                4_000,
            );
            write(
                &h.join("Library/Developer/Xcode/DerivedData/App-abc/Logs/l"),
                100,
            );
        });
        let e = entry(
            &s.home,
            "xcode-derived",
            "Xcode",
            "Library/Developer/Xcode/DerivedData",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        assert_eq!(
            names_of(&t, "Xcode"),
            [
                (
                    "App-abc/Build/Intermediates.noindex".to_string(),
                    false,
                    true
                ),
                ("App-abc/Build/Products".to_string(), true, true),
                ("App-abc/Logs".to_string(), false, true),
            ]
        );
    }

    #[test]
    fn workspace_members_with_split_output_keep_root_relative_names() {
        let s = sandbox(|h| {
            write(&h.join("mono/package.json"), 10);
            write(&h.join("mono/pnpm-workspace.yaml"), 10);
            write(&h.join("mono/src-tauri/Cargo.toml"), 10);
            write(&h.join("mono/src-tauri/target/release/deps/f"), 9_000);
            exe(&h.join("mono/src-tauri/target/release/app"), 5_000);
        });
        let t = build(&s, &[]);
        assert_eq!(
            names_of(&t, "~/mono"),
            [
                ("src-tauri/target/release/app".to_string(), true, false),
                ("src-tauri/target/release/deps".to_string(), false, true),
            ]
        );
    }

    #[test]
    fn registry_wins_over_an_artifact_on_the_same_path() {
        let s = sandbox(project);
        let e = entry(&s.home, "nm", "Mine", "shop/node_modules", Mode::Folder);
        let t = build(&s, &[e]);
        let shop = t.groups.iter().find(|g| g.title == "~/shop").unwrap();
        assert!(shop.rows.iter().all(|r| r.row.name != "node_modules"));
        assert_eq!(titles(&t, SectionKind::Tools), ["Mine"]);
    }

    #[test]
    fn a_candidate_inside_another_is_absorbed() {
        let s = sandbox(|h| {
            write(&h.join("Library/Caches/Big/proj/package.json"), 10);
            write(&h.join("Library/Caches/Big/proj/node_modules/x"), 10_000);
        });
        let e = entry(
            &s.home,
            "big",
            "App caches",
            "Library/Caches/Big",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        let all: Vec<_> = t.groups.iter().flat_map(|g| g.rows.iter()).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].row.path, s.home.join("Library/Caches/Big"));
    }

    #[test]
    fn row_numbers_match_the_scan_and_frees_is_what_deleting_it_alone_frees() {
        let s = sandbox(|h| {
            write(&h.join("Library/Caches/Store/f"), 200_000);
            clone_file(
                &h.join("Library/Caches/Store/f"),
                &h.join("proj/node_modules/f"),
            );
            write(&h.join("proj/package.json"), 10);
        });
        let e = entry(
            &s.home,
            "store",
            "Tool",
            "Library/Caches/Store",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        let store = &t.groups.iter().find(|g| g.title == "Tool").unwrap().rows[0];
        let nm = &t.groups.iter().find(|g| g.title == "~/proj").unwrap().rows[0];
        for r in [store, nm] {
            assert!(r.row.space.is_some());
            assert_eq!(r.row.disk, r.row.space.unwrap().footprint);
            assert_eq!(
                r.row.frees,
                Some(0),
                "the other holder keeps the data alive"
            );
            assert!(!r.row.scanning);
        }
        assert_eq!(store.row.size, 200_000);
    }

    #[test]
    fn a_cache_that_may_hold_offline_data_is_caution_and_still_listed() {
        let s = sandbox(|h| {
            write(&h.join("Library/Caches/Spotify/blob"), 50_000);
            write(&h.join("Library/Caches/Plain/blob"), 50_000);
        });
        let entries = [
            entry(
                &s.home,
                "spotify",
                "App caches",
                "Library/Caches/Spotify",
                Mode::Folder,
            ),
            entry(
                &s.home,
                "plain",
                "App caches",
                "Library/Caches/Plain",
                Mode::Folder,
            ),
        ];
        let t = build(&s, &entries);
        let rows = &t
            .groups
            .iter()
            .find(|g| g.title == "App caches")
            .unwrap()
            .rows;
        let safety = |n: &str| rows.iter().find(|r| r.row.name == n).unwrap().safety;
        assert_eq!(safety("spotify"), Safety::Caution);
        assert_eq!(safety("plain"), Safety::Safe);
    }

    #[test]
    fn command_rows_carry_their_spec() {
        let s = sandbox(project);
        let spec = CommandSpec::new("/x/pnpm", &["store", "prune"]);
        let e = entry(
            &s.home,
            "pnpm-store:v11",
            "pnpm",
            "Library/Caches/Spotify",
            Mode::Command(spec.clone()),
        );
        let t = build(&s, &[e]);
        let row = &t.groups.iter().find(|g| g.title == "pnpm").unwrap().rows[0];
        match &row.kind {
            RowKind::Command { id, spec: got } => {
                assert_eq!(id, "pnpm-store:v11");
                assert_eq!(got, &spec);
            }
            RowKind::Folder => panic!("expected a command row"),
        }
    }

    #[test]
    fn an_entry_outside_the_scan_or_protected_is_not_listed() {
        let s = sandbox(|h| {
            write(&h.join("Library/Keychains/proj/package.json"), 10);
            write(&h.join("Library/Keychains/proj/node_modules/x"), 10);
        });
        let gone = entry(
            &s.home,
            "gone",
            "Tool",
            "Library/Caches/Missing",
            Mode::Folder,
        );
        let t = build(&s, &[gone]);
        assert!(
            t.groups.is_empty(),
            "{:?}",
            t.groups.iter().map(|g| &g.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn sorting_orders_rows_in_each_group_and_groups_by_total() {
        let s = sandbox(project);
        let mut t = build(&s, &[]);
        t.sort(SortBy::Disk);
        // api (90 KB) outweighs shop (40 KB).
        assert_eq!(titles(&t, SectionKind::Projects), ["~/api", "~/shop"]);
        let shop = t.groups.iter().find(|g| g.title == "~/shop").unwrap();
        assert_eq!(shop.rows[0].row.name, "node_modules");
        t.sort(SortBy::Name);
        assert_eq!(titles(&t, SectionKind::Projects), ["~/api", "~/shop"]);
        let shop = t.groups.iter().find(|g| g.title == "~/shop").unwrap();
        assert_eq!(shop.rows[0].row.name, ".next");
    }

    #[test]
    fn sections_stay_in_fixed_order_under_every_sort() {
        let s = sandbox(project);
        let e = entry(
            &s.home,
            "spotify",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let mut t = build(&s, &[e]);
        for by in [SortBy::Name, SortBy::Size, SortBy::Disk, SortBy::Frees] {
            t.sort(by);
            let lines = t.lines(&filter::parse(""), Page::Caches);
            let sections: Vec<_> = lines
                .iter()
                .filter_map(|l| match l {
                    Line::Section(k) => Some(*k),
                    _ => None,
                })
                .collect();
            assert_eq!(
                sections,
                [SectionKind::Projects, SectionKind::Tools],
                "{by:?}"
            );
            assert!(matches!(lines[0], Line::Section(SectionKind::Projects)));
        }
    }

    #[test]
    fn a_closed_group_hides_its_rows_but_a_filter_shows_the_matches() {
        let s = sandbox(project);
        let closed: HashSet<String> = [format!("{:?}:~/shop", SectionKind::Projects)].into();
        let t = Targets::build(&source(&s, &[], closed), &Work::new());
        let rows = |q: &str| {
            t.lines(&filter::parse(q), Page::Caches)
                .iter()
                .filter(|l| matches!(l, Line::Row { .. }))
                .count()
        };
        assert_eq!(rows(""), 1, "only api/target is visible");
        assert_eq!(
            rows("next"),
            1,
            "the filter overrides the fold and shows .next"
        );
    }

    /// The rows `lines` shows for a filter, as `group title / row name`.
    fn shown(t: &Targets, q: &str) -> Vec<String> {
        t.lines(&filter::parse(q), Page::Caches)
            .iter()
            .filter_map(|l| match *l {
                Line::Row { group, row } => Some(format!(
                    "{}/{}",
                    t.groups[group].title,
                    t.row(group, row).row.name.to_string_lossy()
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_filter_that_matches_a_group_shows_all_its_items() {
        let s = sandbox(|h| {
            project(h);
            write(&h.join("other/package.json"), 10);
            write(&h.join("other/node_modules/a"), 5_000);
        });
        let t = build(&s, &[]);
        let mut got = shown(&t, "shop");
        got.sort();
        assert_eq!(
            got,
            ["~/shop/.next", "~/shop/node_modules"],
            "both of shop's items, though neither is named shop"
        );
        // Another group's items are not dragged in.
        assert!(!shown(&t, "shop").iter().any(|r| r.contains("other")));
    }

    #[test]
    fn a_tools_name_shows_that_tools_caches() {
        let s = sandbox(|h| {
            project(h);
            write(&h.join("Library/Caches/Spotify/blob"), 10_000);
            write(&h.join(".cache/uv/wheels/x"), 5_000);
            write(&h.join("Library/Caches/pip/y"), 4_000);
        });
        let mut uv = entry(&s.home, "uv", "uv", ".cache/uv", Mode::Folder);
        uv.label = "cache".to_string();
        let mut pip = entry(&s.home, "pip", "pip", "Library/Caches/pip", Mode::Folder);
        pip.label = "cache".to_string();
        let spot = entry(
            &s.home,
            "spot",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let t = build(&s, &[uv, pip, spot]);
        assert_eq!(shown(&t, "uv"), ["uv/cache"], "only uv's caches");
        assert_eq!(shown(&t, "pip"), ["pip/cache"]);
    }

    #[test]
    fn an_item_name_still_shows_just_the_matching_items_of_other_groups() {
        let s = sandbox(project);
        let t = build(&s, &[]);
        assert_eq!(shown(&t, "next"), ["~/shop/.next"]);
        assert_eq!(shown(&t, "node_modules"), ["~/shop/node_modules"]);
    }

    #[test]
    fn terms_combine_across_the_group_and_its_items() {
        let s = sandbox(|h| {
            project(h);
            write(&h.join("other/package.json"), 10);
            write(&h.join("other/node_modules/a"), 5_000);
            write(&h.join("other/.next/a"), 5_000);
        });
        let t = build(&s, &[]);
        assert_eq!(shown(&t, "shop next"), ["~/shop/.next"]);
        assert_eq!(shown(&t, "other node_modules"), ["~/other/node_modules"]);
        assert!(shown(&t, "shop nothingelse").is_empty());
    }

    #[test]
    fn an_exclusion_still_hides_the_items_it_names_inside_a_matching_group() {
        let s = sandbox(project);
        let t = build(&s, &[]);
        assert_eq!(shown(&t, "shop !next"), ["~/shop/node_modules"]);
        assert!(
            shown(&t, "!shop").is_empty() || !shown(&t, "!shop").iter().any(|r| r.contains("shop"))
        );
    }

    #[test]
    fn a_letters_cannot_be_borrowed_from_the_group_and_the_item_for_one_term() {
        let s = sandbox(|h| {
            project(h);
            write(&h.join("Library/Caches/Spotify/blob"), 10_000);
        });
        let e = entry(
            &s.home,
            "spot",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        // "api" is not in "App caches" nor in "Spotify" on its own.
        assert!(
            shown(&t, "api").iter().all(|r| !r.contains("App caches")),
            "{:?}",
            shown(&t, "api")
        );
    }

    #[test]
    fn a_matching_group_is_shown_open_even_if_it_was_folded() {
        let s = sandbox(project);
        let closed: HashSet<String> = [format!("{:?}:~/shop", SectionKind::Projects)].into();
        let t = Targets::build(&source(&s, &[], closed), &Work::new());
        assert!(!t.groups.iter().find(|g| g.title == "~/shop").unwrap().open);
        assert_eq!(shown(&t, "shop").len(), 2);
    }

    #[test]
    fn a_groups_path_does_not_match_only_its_name() {
        let s = sandbox(|h| {
            write(&h.join("dev/zzz/package.json"), 10);
            write(&h.join("dev/zzz/node_modules/a"), 5_000);
        });
        let t = build(&s, &[]);
        assert!(
            shown(&t, "dev").is_empty(),
            "dev is in the path, not the name"
        );
        assert_eq!(shown(&t, "zzz").len(), 1);
    }

    #[test]
    fn the_matching_group_header_stays_visible_with_its_section() {
        let s = sandbox(project);
        let t = build(&s, &[]);
        let lines = t.lines(&filter::parse("shop"), Page::Caches);
        assert!(matches!(lines[0], Line::Section(SectionKind::Projects)));
        assert!(matches!(lines[1], Line::Group(_)));
        assert_eq!(lines.len(), 4, "section, group, two items: {lines:?}");
    }

    #[test]
    fn a_filter_hides_groups_and_sections_without_matches() {
        let s = sandbox(project);
        let e = entry(
            &s.home,
            "spotify",
            "App caches",
            "Library/Caches/Spotify",
            Mode::Folder,
        );
        let t = build(&s, &[e]);
        let lines = t.lines(&filter::parse("spotify"), Page::Caches);
        assert!(
            lines
                .iter()
                .all(|l| !matches!(l, Line::Section(SectionKind::Projects)))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, Line::Section(SectionKind::Tools)))
        );
        assert!(t.lines(&filter::parse("zzzz"), Page::Caches).is_empty());
    }

    #[test]
    fn find_locates_a_row_by_path() {
        let s = sandbox(project);
        let t = build(&s, &[]);
        let (g, r) = t.find(&s.home.join("api/target")).unwrap();
        assert_eq!(t.row(g, r).row.name, "target");
        assert!(t.find(&s.home.join("nope")).is_none());
    }
}

/// Timing check on a big clone-heavy tree. Slow, so it only runs on request:
/// `cargo test --release stress_ -- --ignored --nocapture`
#[cfg(test)]
mod stress {
    use super::*;
    use crate::scan::Scanner;
    use std::time::{Duration, Instant};

    fn clonefile(from: &std::path::Path, to: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes()).unwrap();
        let t = std::ffi::CString::new(to.as_os_str().as_bytes()).unwrap();
        let rc = unsafe { libc::clonefile(f.as_ptr(), t.as_ptr(), 0) };
        assert_eq!(rc, 0, "clonefile failed (needs APFS)");
    }

    #[test]
    #[ignore]
    fn stress_one_build_on_a_clone_heavy_tree() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap().join("home");
        let (projects, pkgs, files) = (12usize, 300usize, 8usize);
        std::fs::create_dir_all(home.join("Library/Caches/Store")).unwrap();
        // A store of package files, cloned into every project's node_modules.
        for p in 0..pkgs {
            let dir = home.join("Library/Caches/Store").join(format!("pkg{p}"));
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..files {
                std::fs::write(dir.join(format!("f{f}.js")), vec![b'x'; 5000]).unwrap();
            }
        }
        for proj in 0..projects {
            let root = home.join(format!("dev/proj{proj}"));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("package.json"), "{}").unwrap();
            for p in 0..pkgs {
                let dir = root.join(format!("node_modules/pkg{p}"));
                std::fs::create_dir_all(&dir).unwrap();
                for f in 0..files {
                    clonefile(
                        &home.join(format!("Library/Caches/Store/pkg{p}/f{f}.js")),
                        &dir.join(format!("f{f}.js")),
                    );
                }
            }
        }
        let scanner = Scanner::new(8);
        let t = Instant::now();
        let root = scanner.scan_root(&home);
        assert!(scanner.wait_idle(Duration::from_secs(600)));
        println!("scan: {:?}", t.elapsed());
        let policy = Policy::new(&home, &d.path().join("hold"));
        let entries = [ToolEntry {
            id: "store".into(),
            tool: "Tool".into(),
            label: "Store".into(),
            path: home.join("Library/Caches/Store"),
            regen: String::new(),
            mode: Mode::Folder,
            note: None,
        }];
        let src = BuildSource {
            root: Arc::clone(&root),
            home: home.clone(),
            view: scanner.view(),
            scan_idle: true,
            policy: Arc::new(policy),
            entries: entries.to_vec(),
            closed: HashSet::new(),
        };
        let work = Work::new();
        let t = Instant::now();
        let built = Targets::build(&src, &work);
        println!(
            "build: {:?} for {} rows ({} clone files)",
            t.elapsed(),
            built.groups.iter().map(|g| g.rows.len()).sum::<usize>(),
            projects * pkgs * files
        );
        let t = Instant::now();
        let sp = scanner.space(&root);
        println!(
            "space(root) as the Explorer asks on the UI thread: {:?} ({sp:?})",
            t.elapsed()
        );
    }

    /// Look at real projects (read-only): `MC_REAL_ROOTS=dir1:dir2 cargo test
    /// --release real_projects -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_projects_listing() {
        let Some(roots) = std::env::var_os("MC_REAL_ROOTS") else {
            return;
        };
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        for root in std::env::split_paths(&roots) {
            let scanner = Scanner::new(8);
            let t = Instant::now();
            let node = scanner.scan_root(&root);
            assert!(scanner.wait_idle(Duration::from_secs(900)));
            let scan = t.elapsed();
            let policy = Policy::new(&home, &home.join("ToBeDeleted-test-nonexistent"));
            let src = BuildSource {
                root: Arc::clone(&node),
                home: home.clone(),
                view: scanner.view(),
                scan_idle: true,
                policy: Arc::new(policy),
                entries: Vec::new(),
                closed: HashSet::new(),
            };
            let t = Instant::now();
            let built = Targets::build(&src, &Work::new());
            println!(
                "== {} (scan {scan:?}, build {:?})",
                root.display(),
                t.elapsed()
            );
            if built.groups.is_empty() {
                println!("   (nothing listed)");
            }
            for g in &built.groups {
                println!("   {} [{}] {} rows", g.title, g.subtitle, g.rows.len());
                for r in g.rows.iter().take(14) {
                    println!(
                        "      {}{}  {} (frees {:?})",
                        r.row.name.to_string_lossy(),
                        if r.kept { " (kept)" } else { "" },
                        crate::fmt::human_size(r.row.disk),
                        r.row.frees.map(crate::fmt::human_size)
                    );
                }
                if g.rows.len() > 14 {
                    println!("      ... and {} more", g.rows.len() - 14);
                }
            }
        }
    }
}
