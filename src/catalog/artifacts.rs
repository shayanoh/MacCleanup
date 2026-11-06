//! Project build output that can be regenerated: `node_modules`, Rust
//! `target`, and friends. A folder only counts when its project marker file
//! sits next to it, so a stray folder with a familiar name is never offered.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scan::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    NodeModules,
    Next,
    Turbo,
    RustTarget,
    XcodeBuild,
    DartTool,
    Pods,
    FlutterBuild,
}

const ALL: [ArtifactKind; 8] = [
    ArtifactKind::NodeModules,
    ArtifactKind::Next,
    ArtifactKind::Turbo,
    ArtifactKind::RustTarget,
    ArtifactKind::XcodeBuild,
    ArtifactKind::DartTool,
    ArtifactKind::Pods,
    ArtifactKind::FlutterBuild,
];

impl ArtifactKind {
    /// The folder name that is looked for.
    pub fn dir_name(&self) -> &'static str {
        match self {
            ArtifactKind::NodeModules => "node_modules",
            ArtifactKind::Next => ".next",
            ArtifactKind::Turbo => ".turbo",
            ArtifactKind::RustTarget => "target",
            ArtifactKind::XcodeBuild | ArtifactKind::FlutterBuild => "build",
            ArtifactKind::DartTool => ".dart_tool",
            ArtifactKind::Pods => "Pods",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ArtifactKind::NodeModules => "node_modules",
            ArtifactKind::Next => ".next",
            ArtifactKind::Turbo => ".turbo",
            ArtifactKind::RustTarget => "target",
            ArtifactKind::XcodeBuild => "build (Xcode)",
            ArtifactKind::DartTool => ".dart_tool",
            ArtifactKind::Pods => "Pods",
            ArtifactKind::FlutterBuild => "build (Flutter)",
        }
    }

    /// How the folder comes back after it is deleted.
    pub fn regen(&self) -> &'static str {
        match self {
            ArtifactKind::NodeModules => "reinstall with npm, pnpm or yarn install",
            ArtifactKind::Next | ArtifactKind::Turbo => "rebuilt on the next dev or build run",
            ArtifactKind::RustTarget => "rebuilt by cargo build (built binaries inside go too)",
            ArtifactKind::XcodeBuild => "rebuilt by the next Xcode build",
            ArtifactKind::DartTool => "recreated by flutter pub get",
            ArtifactKind::Pods => "reinstall with pod install",
            ArtifactKind::FlutterBuild => "rebuilt by the next flutter build",
        }
    }

    /// Does the project folder `dir` prove this is a real project artifact?
    fn marked(&self, dir: &Path) -> bool {
        match self {
            ArtifactKind::NodeModules | ArtifactKind::Next | ArtifactKind::Turbo => {
                dir.join("package.json").is_file()
            }
            ArtifactKind::RustTarget => dir.join("Cargo.toml").is_file(),
            ArtifactKind::DartTool | ArtifactKind::FlutterBuild => {
                dir.join("pubspec.yaml").is_file()
            }
            ArtifactKind::Pods => dir.join("Podfile").is_file(),
            ArtifactKind::XcodeBuild => fs::read_dir(dir).is_ok_and(|rd| {
                rd.flatten().any(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
                })
            }),
        }
    }
}

/// A project artifact folder found in the scan tree.
#[derive(Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub node: Arc<Node>,
    pub kind: ArtifactKind,
    /// The folder that holds the project marker.
    pub project: PathBuf,
    /// The project root the node tools would find: the nearest folder at or
    /// above `project` that declares a workspace (pnpm, npm/yarn, lerna, nx,
    /// Cargo, ...), or `project` itself. Members share their root's group.
    pub root: PathBuf,
}

/// Folders that hold installed software, not projects: editors' extensions,
/// Node and Rust version managers, app bundles, pnpm's global folder. A
/// `node_modules` in there is part of the software and cannot be reinstalled
/// with `npm install`, so nothing inside is ever offered.
fn is_install_tree(node: &Node) -> bool {
    const NAMES: &[&str] = &[
        ".vscode",
        ".vscode-server",
        ".cursor",
        ".nvm",
        ".fnm",
        ".volta",
        ".rustup",
        ".cargo",
        ".pyenv",
        ".rbenv",
        ".asdf",
        ".sdkman",
    ];
    const UNDER: &[(&str, &str)] = &[
        ("Library", "Application Support"),
        ("Library", "pnpm"),
        ("share", "fnm"),
        ("share", "nvm"),
        ("share", "pnpm"),
    ];
    let name = node.name.to_string_lossy();
    if NAMES.contains(&name.as_ref())
        || name.ends_with(".app")
        || name.ends_with(".framework")
        || name.ends_with(".xcodeproj")
        || name.ends_with(".xcworkspace")
    {
        return true;
    }
    let parent = node
        .path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    UNDER.iter().any(|(p, n)| *p == parent && *n == name)
}

/// What makes a folder a toolchain install rather than a project: a program
/// in `bin/` together with a folder only that toolchain has. Flutter's SDK
/// is itself a Flutter project full of sample apps, so without this its
/// `.dart_tool` and `build` folders would be offered.
const TOOLCHAIN_SIGNATURES: &[(&str, &str)] = &[
    ("flutter", "packages/flutter"),
    ("dart", "lib/_internal"),
    ("go", "src/cmd"),
    ("node", "lib/node_modules"),
    ("java", "lib/modules"),
    ("rustc", "lib/rustlib"),
    ("swift", "lib/swift"),
    ("python3", "lib"),
    ("ruby", "lib/ruby"),
];

/// Is this folder an SDK or language install (found by layout, not by name or
/// place)? Everything inside one belongs to the toolchain, not to a project.
fn is_toolchain_install(node: &Node) -> bool {
    // Cheap first: every install has a `bin` folder, most folders do not.
    if !node.children().iter().any(|c| c.name == "bin") {
        return false;
    }
    TOOLCHAIN_SIGNATURES.iter().any(|(program, sibling)| {
        crate::catalog::command::is_executable(&node.path.join("bin").join(program))
            && node.path.join(sibling).is_dir()
    })
}

/// Files whose presence marks a workspace root.
const WORKSPACE_FILES: &[&str] = &[
    "pnpm-workspace.yaml",
    "lerna.json",
    "nx.json",
    "rush.json",
    "go.work",
    "melos.yaml",
];

/// Does a (small) file contain `needle`? Unreadable counts as no.
fn file_contains(path: &Path, needle: &str) -> bool {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut text = String::new();
    file.take(64 * 1024).read_to_string(&mut text).is_ok() && text.contains(needle)
}

/// Does this folder declare a workspace, the way the package managers read it?
fn is_workspace_root(dir: &Path) -> bool {
    WORKSPACE_FILES.iter().any(|f| dir.join(f).is_file())
        || file_contains(&dir.join("package.json"), "\"workspaces\"")
        || file_contains(&dir.join("Cargo.toml"), "[workspace")
        || file_contains(&dir.join("pubspec.yaml"), "\nworkspace:")
}

/// The nearest workspace root at or above `project`, never above `limit`
/// (the scan root); `project` itself when there is none.
fn workspace_root(
    project: &Path,
    limit: &Path,
    cache: &mut std::collections::HashMap<PathBuf, bool>,
) -> PathBuf {
    for dir in project.ancestors() {
        if !dir.starts_with(limit) {
            break;
        }
        let is_root = *cache
            .entry(dir.to_path_buf())
            .or_insert_with(|| is_workspace_root(dir));
        if is_root {
            return dir.to_path_buf();
        }
    }
    project.to_path_buf()
}

/// Walk the scanned folders and list every project artifact. A folder that
/// matched is not entered, so nested copies are not listed on their own.
pub fn detect_artifacts(root: &Arc<Node>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut workspace_cache = std::collections::HashMap::new();
    let mut stack = vec![Arc::clone(root)];
    while let Some(node) = stack.pop() {
        if is_install_tree(&node) || is_toolchain_install(&node) {
            continue;
        }
        let name = node.name.to_string_lossy();
        let kind = node.path.parent().and_then(|project| {
            ALL.iter()
                .copied()
                .find(|k| k.dir_name() == name && k.marked(project))
        });
        if let (Some(kind), Some(project)) = (kind, node.path.parent()) {
            out.push(Candidate {
                path: node.path.clone(),
                root: workspace_root(project, &root.path, &mut workspace_cache),
                project: project.to_path_buf(),
                kind,
                node: Arc::clone(&node),
            });
            continue;
        }
        // Whatever sits inside an unmatched `node_modules` is a dependency
        // (or a global package), never a project of its own.
        if name == "node_modules" {
            continue;
        }
        stack.extend(node.children());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Scanner;
    use crate::testutil::write;
    use std::path::Path;
    use std::time::Duration;

    fn detect(root: &Path) -> Vec<Candidate> {
        let scanner = Scanner::new(2);
        let node = scanner.scan_root(root);
        assert!(scanner.wait_idle(Duration::from_secs(20)));
        detect_artifacts(&node)
    }

    fn found(root: &Path) -> Vec<(String, ArtifactKind)> {
        let mut v: Vec<_> = detect(root)
            .into_iter()
            .map(|c| {
                (
                    c.path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    c.kind,
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    #[test]
    fn finds_node_modules_next_to_package_json() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("shop/package.json"), 10);
        write(&r.join("shop/node_modules/a/index.js"), 100);
        write(&r.join("shop/.next/cache/x"), 100);
        write(&r.join("shop/.turbo/log"), 10);
        assert_eq!(
            found(r),
            vec![
                ("shop/.next".to_string(), ArtifactKind::Next),
                ("shop/.turbo".to_string(), ArtifactKind::Turbo),
                ("shop/node_modules".to_string(), ArtifactKind::NodeModules),
            ]
        );
    }

    #[test]
    fn ignores_node_modules_without_package_json() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("stray/node_modules/a/index.js"), 100);
        write(&r.join("stray/target/x"), 100);
        assert!(found(r).is_empty());
    }

    #[test]
    fn finds_rust_target_next_to_cargo_toml() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("api/Cargo.toml"), 10);
        write(&r.join("api/target/debug/x"), 100);
        assert_eq!(
            found(r),
            vec![("api/target".to_string(), ArtifactKind::RustTarget)]
        );
    }

    #[test]
    fn does_not_list_nested_node_modules_inside_node_modules() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("shop/package.json"), 10);
        write(&r.join("shop/node_modules/dep/package.json"), 10);
        write(&r.join("shop/node_modules/dep/node_modules/x/i.js"), 10);
        assert_eq!(
            found(r),
            vec![("shop/node_modules".to_string(), ArtifactKind::NodeModules)]
        );
    }

    #[test]
    fn project_is_the_parent_folder() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("a/b/pod/Podfile"), 10);
        write(&r.join("a/b/pod/Pods/x"), 10);
        let c = detect(r);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].project, r.join("a/b/pod"));
        assert_eq!(c[0].kind, ArtifactKind::Pods);
        assert_eq!(c[0].node.path, r.join("a/b/pod/Pods"));
    }

    #[test]
    fn flutter_build_and_dart_tool_need_pubspec() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("app/pubspec.yaml"), 10);
        write(&r.join("app/build/x"), 10);
        write(&r.join("app/.dart_tool/y"), 10);
        write(&r.join("other/build/x"), 10);
        write(&r.join("other/.dart_tool/y"), 10);
        assert_eq!(
            found(r),
            vec![
                ("app/.dart_tool".to_string(), ArtifactKind::DartTool),
                ("app/build".to_string(), ArtifactKind::FlutterBuild),
            ]
        );
    }

    #[test]
    fn xcode_build_needs_an_xcodeproj_next_to_it() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("ios/App.xcodeproj/project.pbxproj"), 10);
        write(&r.join("ios/build/x"), 10);
        assert_eq!(
            found(r),
            vec![("ios/build".to_string(), ArtifactKind::XcodeBuild)]
        );
    }

    #[test]
    fn never_offers_app_or_tool_install_trees() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for project in [
            ".vscode/extensions/ext-1.0",
            ".cursor/extensions/ext-2.0",
            ".nvm/versions/node/v20.0.0/lib/node_modules/npm",
            ".local/share/fnm/node-versions/v22.0.0/installation/lib/node_modules/npm",
            ".volta/tools/image/node/20/lib/node_modules/npm",
            ".cargo/registry/src/x",
            "Applications/Foo.app/Contents/Resources/app",
            "Library/Application Support/Code/x",
            "Library/pnpm/global/5",
        ] {
            write(&r.join(project).join("package.json"), 10);
            write(&r.join(project).join("node_modules/dep/index.js"), 100);
        }
        // A real project next to them is still found.
        write(&r.join("dev/shop/package.json"), 10);
        write(&r.join("dev/shop/node_modules/dep/index.js"), 100);
        assert_eq!(
            found(r),
            vec![(
                "dev/shop/node_modules".to_string(),
                ArtifactKind::NodeModules
            )]
        );
    }

    fn exe(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        write(path, 10);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_flutter_sdk_is_recognised_by_its_layout_wherever_it_lives() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // The SDK is a Flutter project itself and holds many sample apps.
        for sdk in ["tools/anywhere/flutter", "Flutter/flutter"] {
            let sdk = r.join(sdk);
            exe(&sdk.join("bin/flutter"));
            write(&sdk.join("packages/flutter/pubspec.yaml"), 10);
            write(&sdk.join("pubspec.yaml"), 10);
            write(&sdk.join(".dart_tool/x"), 10);
            write(&sdk.join("dev/a11y/pubspec.yaml"), 10);
            write(&sdk.join("dev/a11y/.dart_tool/x"), 10);
            write(&sdk.join("examples/hello/pubspec.yaml"), 10);
            write(&sdk.join("examples/hello/build/x"), 10);
        }
        // A real app next to them is still found.
        write(&r.join("dev/app/pubspec.yaml"), 10);
        write(&r.join("dev/app/.dart_tool/x"), 10);
        assert_eq!(
            found(r),
            vec![("dev/app/.dart_tool".to_string(), ArtifactKind::DartTool)]
        );
    }

    #[test]
    fn other_toolchain_installs_are_recognised_too() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // Dart SDK, a Node install, a JDK, a Rust toolchain.
        exe(&r.join("sdks/dart-sdk/bin/dart"));
        write(&r.join("sdks/dart-sdk/lib/_internal/x"), 10);
        write(&r.join("sdks/dart-sdk/pkg/p/pubspec.yaml"), 10);
        write(&r.join("sdks/dart-sdk/pkg/p/.dart_tool/x"), 10);
        exe(&r.join("sdks/node/bin/node"));
        write(&r.join("sdks/node/lib/node_modules/npm/package.json"), 10);
        write(
            &r.join("sdks/node/lib/node_modules/npm/node_modules/a/i.js"),
            10,
        );
        exe(&r.join("sdks/rust/bin/rustc"));
        write(&r.join("sdks/rust/lib/rustlib/x"), 10);
        write(&r.join("sdks/rust/src/Cargo.toml"), 10);
        write(&r.join("sdks/rust/src/target/x"), 10);
        assert!(found(r).is_empty(), "{:?}", found(r));
    }

    #[test]
    fn a_project_that_merely_has_a_bin_folder_is_not_an_install() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // A command-line tool project: bin/ with scripts named like tools.
        write(&r.join("cli/package.json"), 10);
        exe(&r.join("cli/bin/node"));
        exe(&r.join("cli/bin/flutter"));
        write(&r.join("cli/node_modules/a/i.js"), 10);
        assert_eq!(
            found(r),
            vec![("cli/node_modules".to_string(), ArtifactKind::NodeModules)]
        );
    }

    fn roots(root: &Path) -> Vec<(String, String)> {
        let mut v: Vec<_> = detect(root)
            .into_iter()
            .map(|c| {
                (
                    c.path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    c.root
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
        v.sort();
        v
    }

    fn pkg(dir: &Path) {
        write(&dir.join("package.json"), 10);
        write(&dir.join("node_modules/dep/i.js"), 100);
    }

    #[test]
    fn members_of_a_pnpm_workspace_belong_to_its_root() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        pkg(&r.join("mono"));
        write(&r.join("mono/pnpm-workspace.yaml"), 10);
        pkg(&r.join("mono/apps/web"));
        pkg(&r.join("mono/packages/ui"));
        assert_eq!(
            roots(r),
            [
                ("mono/apps/web/node_modules".to_string(), "mono".to_string()),
                ("mono/node_modules".to_string(), "mono".to_string()),
                (
                    "mono/packages/ui/node_modules".to_string(),
                    "mono".to_string()
                ),
            ]
        );
    }

    #[test]
    fn npm_and_yarn_workspaces_in_package_json_are_found_too() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("mono/package.json"), 10);
        std::fs::write(
            r.join("mono/package.json"),
            r#"{"name":"m","workspaces":["packages/*"]}"#,
        )
        .unwrap();
        write(&r.join("mono/node_modules/a/i.js"), 10);
        pkg(&r.join("mono/packages/a"));
        let got = roots(r);
        assert!(got.iter().all(|(_, root)| root == "mono"), "{got:?}");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_tauri_crate_inside_a_js_workspace_belongs_to_that_workspace() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        pkg(&r.join("sdm"));
        write(&r.join("sdm/pnpm-workspace.yaml"), 10);
        pkg(&r.join("sdm/web"));
        write(&r.join("sdm/src-tauri/Cargo.toml"), 10);
        write(&r.join("sdm/src-tauri/target/debug/x"), 10);
        let got = roots(r);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got.iter().all(|(_, root)| root == "sdm"), "{got:?}");
    }

    #[test]
    fn a_cargo_workspace_root_owns_its_members() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("ws/crates/a")).unwrap();
        std::fs::write(
            r.join("ws/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        write(&r.join("ws/target/debug/x"), 10);
        write(&r.join("ws/crates/a/Cargo.toml"), 10);
        write(&r.join("ws/crates/a/target/debug/x"), 10);
        let got = roots(r);
        assert_eq!(
            got,
            [
                ("ws/crates/a/target".to_string(), "ws".to_string()),
                ("ws/target".to_string(), "ws".to_string()),
            ]
        );
    }

    #[test]
    fn projects_without_a_workspace_are_their_own_root() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        pkg(&r.join("dev/one"));
        pkg(&r.join("dev/two"));
        assert_eq!(
            roots(r),
            [
                ("dev/one/node_modules".to_string(), "dev/one".to_string()),
                ("dev/two/node_modules".to_string(), "dev/two".to_string()),
            ]
        );
    }

    #[test]
    fn the_root_search_never_climbs_above_the_scan_root() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        pkg(&r.join("mono"));
        write(&r.join("mono/pnpm-workspace.yaml"), 10);
        pkg(&r.join("mono/apps/web"));
        // Scanning only the member: the workspace root is outside the scan.
        let got = detect(&r.join("mono/apps/web"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].root, r.join("mono/apps/web"));
    }

    #[test]
    fn a_nearer_workspace_wins_over_an_outer_one() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("outer/pnpm-workspace.yaml"), 10);
        write(&r.join("outer/inner/pnpm-workspace.yaml"), 10);
        pkg(&r.join("outer/inner/app"));
        let got = roots(r);
        assert_eq!(
            got,
            [(
                "outer/inner/app/node_modules".to_string(),
                "outer/inner".to_string()
            )]
        );
    }

    #[test]
    fn a_global_package_inside_node_modules_is_not_a_project() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // lib/node_modules/<pkg>/node_modules with no project marker beside lib.
        write(&r.join("lib/node_modules/pkg/package.json"), 10);
        write(&r.join("lib/node_modules/pkg/node_modules/dep/i.js"), 10);
        assert!(found(r).is_empty());
    }

    #[test]
    fn ignores_a_file_named_target() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("api/Cargo.toml"), 10);
        write(&r.join("api/target"), 10);
        assert!(found(r).is_empty());
    }
}
