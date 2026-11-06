//! The Caches & Projects list: project artifacts grouped by project, tool
//! caches grouped by tool, each as a [`Row`] with the same numbers the
//! Explorer shows.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::{SortBy, cmp_rows};
use crate::bg::Work;
use crate::catalog::{ArtifactKind, CommandSpec, Mode, ToolEntry, detect_artifacts};
use crate::filter::Query;
use crate::safety::{Policy, Safety};
use crate::scan::{Node, Row, SpaceView, locate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionKind {
    Projects,
    Tools,
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
    node: Arc<Node>,
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
}

fn project_kind(k: ArtifactKind) -> &'static str {
    match k {
        ArtifactKind::NodeModules | ArtifactKind::Next | ArtifactKind::Turbo => "Node",
        ArtifactKind::RustTarget => "Rust",
        ArtifactKind::DartTool | ArtifactKind::FlutterBuild => "Flutter",
        ArtifactKind::Pods => "CocoaPods",
        ArtifactKind::XcodeBuild => "Xcode",
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
                node,
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
            });
        }
        work.stage(1, 3, "finding project folders");
        for c in detect_artifacts(root) {
            cands.push(Cand {
                // Named relative to the root project: "apps/web/node_modules".
                name: c.path.strip_prefix(&c.root).map_or_else(
                    |_| c.kind.label().to_string(),
                    |rel| rel.to_string_lossy().into_owned(),
                ),
                path: c.path,
                node: c.node,
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
        let mut kept: HashSet<PathBuf> = HashSet::new();
        cands.retain(|c| {
            if c.path.ancestors().any(|a| kept.contains(a)) {
                return false;
            }
            kept.insert(c.path.clone());
            true
        });

        // Numbers: one pass over the clone tables when the scan is idle.
        work.stage(2, 3, "reading space numbers");
        let idle = scan_idle;
        let nodes: Vec<Arc<Node>> = cands.iter().map(|c| Arc::clone(&c.node)).collect();
        let spaces = if idle {
            view.spaces_of(&nodes)
        } else {
            Vec::new()
        };

        let mut groups: Vec<Group> = Vec::new();
        // What each project group holds, to write its subtitle at the end.
        let mut ecos: std::collections::HashMap<
            String,
            (std::collections::BTreeSet<String>, bool),
        > = std::collections::HashMap::new();
        for (i, c) in cands.into_iter().enumerate() {
            let space = spaces.get(i).copied();
            let row = Row {
                name: c.name.clone().into(),
                path: c.path.clone(),
                is_dir: true,
                size: c.node.size(),
                disk: space.map_or_else(|| c.node.disk(), |s| s.footprint),
                files: c.node.files(),
                scanning: !c.node.is_done(),
                node: Some(Arc::clone(&c.node)),
                space,
                frees: space.map(|s| s.frees_now),
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
                kept: false,
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
            let section = |g: &Group| match g.section {
                SectionKind::Projects => 0,
                SectionKind::Tools => 1,
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
    /// groups. An active filter shows only matching rows (and their groups)
    /// whatever the folds say.
    pub fn lines(&self, filter: &Query) -> Vec<Line> {
        let filtering = !filter.is_empty();
        let matches = |r: &TRow| filter.matches(&r.row.name.to_string_lossy()).is_some();
        let mut out = Vec::new();
        for section in [SectionKind::Projects, SectionKind::Tools] {
            let mut body = Vec::new();
            for (gi, g) in self
                .groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.section == section)
            {
                let hits: Vec<usize> = g
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !filtering || matches(r))
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
            let lines = t.lines(&filter::parse(""));
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
            t.lines(&filter::parse(q))
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
        let lines = t.lines(&filter::parse("spotify"));
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
        assert!(t.lines(&filter::parse("zzzz")).is_empty());
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
}
