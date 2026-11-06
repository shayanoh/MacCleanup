//! Build folders hold two kinds of things: intermediate files that any build
//! recreates, and the products people actually want (a built program, an APK,
//! a bundle). Splitting a build folder into rows lets the user delete the
//! intermediates and keep the products: bulk marks skip the products, and a
//! product is deleted only when that row itself is marked.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scan::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitKind {
    RustTarget,
    Swift,
    Xcode,
    /// Xcode's `~/Library/Developer/Xcode/DerivedData`: one folder per project.
    DerivedData,
    Flutter,
    Gradle,
    Maven,
}

/// One row of a split build folder.
pub struct Part {
    pub path: PathBuf,
    /// The scanned folder (`None` for a single file).
    pub node: Option<Arc<Node>>,
    /// Relative to the build folder: `debug/deps`.
    pub name: String,
    /// A product: kept unless marked on its own.
    pub kept: bool,
}

/// Split a build folder into its parts. Empty means "nothing recognised":
/// the caller then shows the folder as one row.
pub fn split(kind: SplitKind, node: &Arc<Node>) -> Vec<Part> {
    let mut out = Vec::new();
    match kind {
        SplitKind::RustTarget => rust(node, &mut out),
        SplitKind::Swift => swift(node, &mut out),
        SplitKind::Xcode => xcode(node, "", &mut out),
        SplitKind::DerivedData => derived_data(node, &mut out),
        SplitKind::Flutter => flutter(node, &mut out),
        SplitKind::Gradle => gradle(node, &mut out),
        SplitKind::Maven => maven(node, &mut out),
    }
    out
}

fn name_of(n: &Node) -> String {
    n.name.to_string_lossy().into_owned()
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn children_sorted(node: &Node) -> Vec<Arc<Node>> {
    let mut c = node.children();
    c.sort_by_key(|n| name_of(n));
    c
}

fn dir_part(n: &Arc<Node>, name: String, kept: bool) -> Part {
    Part {
        path: n.path.clone(),
        node: Some(Arc::clone(n)),
        name,
        kept,
    }
}

/// The executables directly in `dir` (programs and dynamic libraries): real
/// files with an execute bit, never links.
fn executables(dir: &Path, prefix: &str, out: &mut Vec<Part>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let md = fs::symlink_metadata(e.path()).ok()?;
            (md.is_file() && md.permissions().mode() & 0o111 != 0)
                .then(|| (e.file_name().to_string_lossy().into_owned(), e.path()))
        })
        .collect();
    files.sort();
    out.extend(files.into_iter().map(|(name, path)| Part {
        path,
        node: None,
        name: join(prefix, &name),
        kept: true,
    }));
}

/// A cargo/SwiftPM profile folder (`debug`, `release`): its folders are
/// intermediate (`bundle` holds installers and is a product) and the
/// executables beside them are the products.
fn profile(p: &Arc<Node>, prefix: &str, out: &mut Vec<Part>) {
    for c in children_sorted(p) {
        let name = name_of(&c);
        let kept = name == "bundle";
        out.push(dir_part(&c, join(prefix, &name), kept));
    }
    executables(&p.path, prefix, out);
}

fn is_profile(n: &Node) -> bool {
    let name = name_of(n);
    name == "debug"
        || name == "release"
        || n.children()
            .iter()
            .any(|c| c.name == "deps" || c.name == ".fingerprint")
}

fn rust(target: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(target) {
        let name = name_of(&c);
        if is_profile(&c) {
            profile(&c, &name, out);
        } else if c.children().iter().any(|g| is_profile(g)) {
            // A cross-compile target such as aarch64-apple-darwin.
            for p in children_sorted(&c).iter().filter(|g| is_profile(g)) {
                profile(p, &join(&name, &name_of(p)), out);
            }
        } else {
            out.push(dir_part(&c, name, false));
        }
    }
}

fn swift(build: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(build) {
        let name = name_of(&c);
        let triple = c
            .children()
            .iter()
            .any(|g| g.name == "debug" || g.name == "release");
        if triple {
            for p in children_sorted(&c)
                .iter()
                .filter(|g| g.name == "debug" || g.name == "release")
            {
                // Module build folders are intermediate; executables are products.
                for m in children_sorted(p) {
                    out.push(dir_part(
                        &m,
                        join(&join(&name, &name_of(p)), &name_of(&m)),
                        false,
                    ));
                }
                executables(&p.path, &join(&name, &name_of(p)), out);
            }
        } else {
            out.push(dir_part(&c, name, false));
        }
    }
}

/// Xcode build output: `Build/Products` holds the apps, everything else
/// (`Intermediates`, `Index`, `Logs`, module caches) is intermediate.
fn xcode(dir: &Arc<Node>, prefix: &str, out: &mut Vec<Part>) {
    for c in children_sorted(dir) {
        let name = name_of(&c);
        if name == "Build" {
            for b in children_sorted(&c) {
                let kept = b.name == "Products";
                out.push(dir_part(
                    &b,
                    join(&join(prefix, "Build"), &name_of(&b)),
                    kept,
                ));
            }
        } else {
            // The older layout puts products in `Debug-*` / `Release-*` folders.
            let kept =
                name.starts_with("Debug") || name.starts_with("Release") || name.ends_with(".app");
            out.push(dir_part(&c, join(prefix, &name), kept));
        }
    }
}

fn derived_data(dd: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(dd) {
        let name = name_of(&c);
        let is_project = c
            .children()
            .iter()
            .any(|g| g.name == "Build" || g.name == "Index.noindex" || g.name == "Logs");
        if is_project {
            xcode(&c, &name, out);
        } else {
            out.push(dir_part(&c, name, false));
        }
    }
}

fn flutter(build: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(build) {
        let name = name_of(&c);
        match name.as_str() {
            // Finished builds for the web and the desktop platforms.
            "web" | "linux" | "windows" => out.push(dir_part(&c, name, true)),
            "app" | "ios" | "macos" => {
                for g in children_sorted(&c) {
                    let kept = matches!(name_of(&g).as_str(), "outputs" | "ipa" | "iphoneos");
                    out.push(dir_part(&g, join(&name, &name_of(&g)), kept));
                }
            }
            _ => out.push(dir_part(&c, name, false)),
        }
    }
}

fn gradle(build: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(build) {
        let name = name_of(&c);
        let kept = matches!(
            name.as_str(),
            "outputs" | "libs" | "distributions" | "install"
        );
        out.push(dir_part(&c, name, kept));
    }
}

fn maven(target: &Arc<Node>, out: &mut Vec<Part>) {
    for c in children_sorted(target) {
        out.push(dir_part(&c, name_of(&c), false));
    }
    // Packaged results sit directly in `target`.
    let Ok(rd) = fs::read_dir(&target.path) else {
        return;
    };
    let mut files: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let md = fs::symlink_metadata(e.path()).ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            let packaged = [".jar", ".war", ".ear", ".zip", ".tar.gz"]
                .iter()
                .any(|ext| name.ends_with(ext));
            (md.is_file() && packaged).then_some((name, e.path()))
        })
        .collect();
    files.sort();
    out.extend(files.into_iter().map(|(name, path)| Part {
        path,
        node: None,
        name,
        kept: true,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Scanner;
    use crate::testutil::write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    fn exe(path: &Path, bytes: usize) {
        write(path, bytes);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Scan `root` and return the parts of the folder `dir` inside it.
    fn parts_of(root: &Path, dir: &str, kind: SplitKind) -> Vec<(String, bool, bool)> {
        let scanner = Scanner::new(2);
        let top = scanner.scan_root(root);
        assert!(scanner.wait_idle(Duration::from_secs(20)));
        let node = crate::scan::locate(&top, &root.join(dir)).unwrap();
        let mut v: Vec<_> = split(kind, &node)
            .into_iter()
            .map(|p| (p.name, p.kept, p.node.is_none()))
            .collect();
        v.sort();
        v
    }

    fn row(name: &str, kept: bool, file: bool) -> (String, bool, bool) {
        (name.to_string(), kept, file)
    }

    #[test]
    fn rust_target_separates_intermediate_folders_from_binaries() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for profile in ["debug", "release"] {
            for dir in ["deps", "build", "incremental", ".fingerprint"] {
                write(&r.join(format!("target/{profile}/{dir}/f")), 1000);
            }
            exe(&r.join(format!("target/{profile}/myapp")), 5000);
            write(&r.join(format!("target/{profile}/myapp.d")), 100);
            write(&r.join(format!("target/{profile}/libfoo.rlib")), 100);
        }
        write(&r.join("target/release/bundle/dmg/app.dmg"), 9000);
        write(&r.join("target/doc/index.html"), 100);
        write(&r.join("target/CACHEDIR.TAG"), 10);
        assert_eq!(
            parts_of(r, "target", SplitKind::RustTarget),
            [
                row("debug/.fingerprint", false, false),
                row("debug/build", false, false),
                row("debug/deps", false, false),
                row("debug/incremental", false, false),
                row("debug/myapp", true, true),
                row("doc", false, false),
                row("release/.fingerprint", false, false),
                row("release/build", false, false),
                row("release/bundle", true, false),
                row("release/deps", false, false),
                row("release/incremental", false, false),
                row("release/myapp", true, true),
            ]
        );
    }

    #[test]
    fn rust_target_triple_folders_are_split_by_profile_too() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("target/aarch64-apple-darwin/release/deps/f"), 1000);
        exe(&r.join("target/aarch64-apple-darwin/release/tool"), 3000);
        write(&r.join("target/debug/deps/f"), 1000);
        let got = parts_of(r, "target", SplitKind::RustTarget);
        assert!(
            got.contains(&row("aarch64-apple-darwin/release/deps", false, false)),
            "{got:?}"
        );
        assert!(
            got.contains(&row("aarch64-apple-darwin/release/tool", true, true)),
            "{got:?}"
        );
    }

    #[test]
    fn a_target_folder_with_nothing_recognisable_is_not_split() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("target/x"), 10);
        assert!(parts_of(r, "target", SplitKind::RustTarget).is_empty());
    }

    #[test]
    fn a_non_executable_file_in_a_profile_is_not_a_binary() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("target/debug/deps/f"), 1000);
        write(&r.join("target/debug/notes.txt"), 100);
        write(&r.join("target/debug/myapp.d"), 100);
        let got = parts_of(r, "target", SplitKind::RustTarget);
        assert_eq!(got, [row("debug/deps", false, false)]);
    }

    #[test]
    fn xcode_build_keeps_the_products_and_lists_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("build/Build/Products/Debug/App.app/App"), 5000);
        write(&r.join("build/Build/Intermediates.noindex/x/y"), 1000);
        write(&r.join("build/Logs/Build/log"), 100);
        write(&r.join("build/ModuleCache.noindex/m"), 100);
        assert_eq!(
            parts_of(r, "build", SplitKind::Xcode),
            [
                row("Build/Intermediates.noindex", false, false),
                row("Build/Products", true, false),
                row("Logs", false, false),
                row("ModuleCache.noindex", false, false),
            ]
        );
    }

    #[test]
    fn derived_data_is_split_per_project() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(
            &r.join("DerivedData/App-abc/Build/Products/Debug/App.app/App"),
            5000,
        );
        write(
            &r.join("DerivedData/App-abc/Build/Intermediates.noindex/x"),
            1000,
        );
        write(&r.join("DerivedData/App-abc/Index.noindex/i"), 100);
        write(&r.join("DerivedData/ModuleCache.noindex/m"), 100);
        assert_eq!(
            parts_of(r, "DerivedData", SplitKind::DerivedData),
            [
                row("App-abc/Build/Intermediates.noindex", false, false),
                row("App-abc/Build/Products", true, false),
                row("App-abc/Index.noindex", false, false),
                row("ModuleCache.noindex", false, false),
            ]
        );
    }

    #[test]
    fn flutter_build_keeps_the_outputs_and_the_web_build() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(
            &r.join("build/app/outputs/flutter-apk/app-release.apk"),
            9000,
        );
        write(&r.join("build/app/intermediates/x"), 1000);
        write(&r.join("build/app/tmp/t"), 100);
        write(&r.join("build/web/index.html"), 100);
        write(&r.join("build/native_assets/n"), 100);
        assert_eq!(
            parts_of(r, "build", SplitKind::Flutter),
            [
                row("app/intermediates", false, false),
                row("app/outputs", true, false),
                row("app/tmp", false, false),
                row("native_assets", false, false),
                row("web", true, false),
            ]
        );
    }

    #[test]
    fn gradle_build_keeps_outputs_and_libs() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("build/outputs/apk/release/app.apk"), 9000);
        write(&r.join("build/libs/lib.jar"), 3000);
        write(&r.join("build/intermediates/x"), 1000);
        write(&r.join("build/tmp/t"), 100);
        write(&r.join("build/generated/g"), 100);
        assert_eq!(
            parts_of(r, "build", SplitKind::Gradle),
            [
                row("generated", false, false),
                row("intermediates", false, false),
                row("libs", true, false),
                row("outputs", true, false),
                row("tmp", false, false),
            ]
        );
    }

    #[test]
    fn maven_target_keeps_the_packaged_files() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        exe_or_not(&r.join("target/app-1.0.jar"), 5000);
        exe_or_not(&r.join("target/app-1.0.war"), 5000);
        write(&r.join("target/classes/A.class"), 1000);
        write(&r.join("target/maven-status/x"), 100);
        write(&r.join("target/notes.txt"), 100);
        assert_eq!(
            parts_of(r, "target", SplitKind::Maven),
            [
                row("app-1.0.jar", true, true),
                row("app-1.0.war", true, true),
                row("classes", false, false),
                row("maven-status", false, false),
            ]
        );
    }

    fn exe_or_not(path: &Path, bytes: usize) {
        write(path, bytes);
    }

    #[test]
    fn swiftpm_build_keeps_executables_and_lists_dependencies_and_module_builds() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join(".build/checkouts/dep/Sources/x.swift"), 1000);
        write(&r.join(".build/repositories/dep/objects"), 1000);
        write(
            &r.join(".build/arm64-apple-macosx/release/Foo.build/f.o"),
            1000,
        );
        exe(&r.join(".build/arm64-apple-macosx/release/foo"), 4000);
        write(
            &r.join(".build/arm64-apple-macosx/release/Foo.swiftmodule"),
            10,
        );
        let got = parts_of(r, ".build", SplitKind::Swift);
        assert_eq!(
            got,
            [
                row("arm64-apple-macosx/release/Foo.build", false, false),
                row("arm64-apple-macosx/release/foo", true, true),
                row("checkouts", false, false),
                row("repositories", false, false),
            ]
        );
    }

    #[test]
    fn symlinks_are_never_listed_as_binaries() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("target/debug/deps/f"), 1000);
        exe(&r.join("elsewhere/real"), 100);
        std::os::unix::fs::symlink(r.join("elsewhere/real"), r.join("target/debug/link")).unwrap();
        let got = parts_of(r, "target", SplitKind::RustTarget);
        assert_eq!(got, [row("debug/deps", false, false)]);
    }
}
