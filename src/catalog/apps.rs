//! The installed-app index: every app bundle the Mac knows about, with the
//! ids of the helpers nested inside them. The Uninstall page lists these; the
//! Leftovers page uses the index to prove that an app is really gone.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::command::{CommandRunner, CommandSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppBundle {
    pub path: PathBuf,
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: PathBuf,
    /// Name of the owning user (`root`, `macports`, ...).
    pub owner: String,
    /// Unix seconds of the last time it was opened; 0 = never.
    pub last_used: i64,
}

#[derive(Clone, Debug, Default)]
pub struct InstalledIndex {
    pub apps: Vec<AppBundle>,
    /// Ids of bundles nested inside apps (helpers, login items, extensions).
    pub nested_ids: BTreeSet<String>,
    /// Did Spotlight answer the question "which apps exist"?
    pub spotlight_ok: bool,
}

/// The first two dot-separated parts of an id, lowercased (`com.autodesk`).
pub fn vendor_of(id: &str) -> String {
    id.split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
        .to_ascii_lowercase()
}

impl InstalledIndex {
    fn known_ids(&self) -> impl Iterator<Item = &str> {
        self.apps
            .iter()
            .map(|a| a.id.as_str())
            .chain(self.nested_ids.iter().map(String::as_str))
    }

    /// Is `id` an installed bundle's id, or the beginning of one (an app
    /// whose data folders carry a shorter id than its bundle)?
    pub fn has_id(&self, id: &str) -> bool {
        let id = id.to_ascii_lowercase();
        self.known_ids().any(|k| {
            let k = k.to_ascii_lowercase();
            k == id || k.starts_with(&format!("{id}."))
        })
    }

    /// Does any installed bundle share the vendor part (`com.autodesk`)?
    pub fn vendor_has_app(&self, id: &str) -> bool {
        let v = vendor_of(id);
        self.known_ids().any(|k| vendor_of(k) == v)
    }
}

fn plist_string(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key)?.as_string().map(str::to_string)
}

fn read_plist(bundle: &Path) -> Option<plist::Dictionary> {
    ["Contents/Info.plist", "Info.plist"]
        .iter()
        .find_map(|p| plist::Value::from_file(bundle.join(p)).ok())
        .and_then(|v| v.into_dictionary())
}

fn owner_name(uid: u32) -> String {
    // SAFETY: getpwuid returns a pointer to static storage or null; the name
    // is copied out immediately and the pointer is not kept.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return uid.to_string();
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_string_lossy()
            .into_owned()
    }
}

/// Read an app bundle's identity. `None` when it has no readable plist.
pub fn read_bundle(path: &Path) -> Option<AppBundle> {
    let info = read_plist(path)?;
    let id = plist_string(&info, "CFBundleIdentifier")?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = plist_string(&info, "CFBundleDisplayName")
        .or_else(|| plist_string(&info, "CFBundleName"))
        .unwrap_or(stem);
    let exe = plist_string(&info, "CFBundleExecutable").unwrap_or_default();
    let uid = fs::metadata(path).map_or(0, |m| m.uid());
    Some(AppBundle {
        path: path.to_path_buf(),
        id,
        name,
        version: plist_string(&info, "CFBundleShortVersionString").unwrap_or_default(),
        executable: path.join("Contents/MacOS").join(exe),
        owner: owner_name(uid),
        last_used: 0,
    })
}

/// Ids of bundles nested inside an app (`.app`, `.xpc`, `.appex`), a few
/// levels deep under `Contents`. Symlinks are not followed.
fn nested_ids(app: &Path, out: &mut BTreeSet<String>) {
    let mut stack = vec![(app.join("Contents"), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if !t.is_dir() {
                continue; // also skips symlinks
            }
            let p = e.path();
            let is_bundle = p
                .extension()
                .is_some_and(|x| matches!(x.to_str(), Some("app" | "xpc" | "appex")));
            if is_bundle
                && let Some(info) = read_plist(&p)
                && let Some(id) = plist_string(&info, "CFBundleIdentifier")
            {
                out.insert(id);
            }
            if depth < 4 {
                stack.push((p, depth + 1));
            }
        }
    }
}

/// Apps under `root`, descending into plain subfolders (`Utilities`,
/// `Chrome Apps.localized`) but never into an app. A symlink to an app counts
/// once, by its real location.
fn collect(root: &Path, depth: usize, seen: &mut HashSet<PathBuf>, out: &mut Vec<AppBundle>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let is_app = path.extension().is_some_and(|x| x == "app");
        let Ok(real) = path.canonicalize() else {
            continue;
        };
        if is_app {
            if seen.insert(real.clone())
                && let Some(mut b) = read_bundle(&real)
            {
                b.path = path.clone();
                out.push(b);
            }
        } else if depth < 2 && e.file_type().is_ok_and(|t| t.is_dir()) {
            collect(&path, depth + 1, seen, out);
        }
    }
}

/// `2026-11-05 19:04:38 +0000` as Unix seconds; `None` for `(null)` or junk.
pub fn parse_md_date(text: &str) -> Option<i64> {
    let t = text.trim();
    let (date, rest) = t.split_once(' ')?;
    let (time, zone) = rest.split_once(' ').unwrap_or((rest, "+0000"));
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut hms = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi, s) = (hms.next()??, hms.next()??, hms.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since 1970-01-01 (civil-from-days, Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z: i64 = zone.trim_start_matches(['+', '-']).parse().unwrap_or(0);
    let offset = sign * ((z / 100) * 3600 + (z % 100) * 60);
    Some(days * 86_400 + h * 3600 + mi * 60 + s - offset)
}

/// Fill `last_used` from Spotlight, 50 bundles per `mdls` call. A missing
/// answer stays 0 ("never").
fn fill_last_used(apps: &mut [AppBundle], runner: &dyn CommandRunner) {
    for chunk in apps.chunks_mut(50) {
        let mut args: Vec<String> =
            vec!["-name".into(), "kMDItemLastUsedDate".into(), "-raw".into()];
        args.extend(chunk.iter().map(|a| a.path.to_string_lossy().into_owned()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let Some(out) = runner.query(&CommandSpec::new("/usr/bin/mdls", &refs)) else {
            continue;
        };
        for (app, field) in chunk.iter_mut().zip(out.split('\0')) {
            app.last_used = parse_md_date(field).unwrap_or(0);
        }
    }
}

/// Build the index from the scan roots plus whatever Spotlight lists.
pub fn build_index(roots: &[PathBuf], runner: &dyn CommandRunner) -> InstalledIndex {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for r in roots {
        collect(r, 0, &mut seen, &mut apps);
    }
    // Spotlight knows apps in places we do not scan (other volumes, Setapp).
    let found = runner.query(&CommandSpec::new(
        "/usr/bin/mdfind",
        &["kMDItemContentType == 'com.apple.application-bundle'"],
    ));
    let spotlight_ok = found
        .as_deref()
        .is_some_and(|t| t.lines().any(|l| l.starts_with("/Applications/")));
    let mut spotlight_nested = BTreeSet::new();
    for line in found.as_deref().unwrap_or("").lines() {
        let p = PathBuf::from(line);
        // Only top-level app bundles; nested ones are covered by `nested_ids`.
        let Ok(real) = p.canonicalize() else { continue };
        if p.is_absolute()
            && p.extension().is_some_and(|x| x == "app")
            && !p.components().any(|c| c.as_os_str() == "Contents")
            && seen.insert(real.clone())
            && let Some(mut b) = read_bundle(&real)
        {
            // An app that lives inside a Library folder (Fusion's downloaded
            // payload) belongs to another app: it proves an id exists but is
            // not a program to uninstall by itself.
            if p.components().any(|c| c.as_os_str() == "Library") {
                spotlight_nested.insert(b.id);
            } else {
                b.path = p;
                apps.push(b);
            }
        }
    }
    let mut nested = spotlight_nested;
    for a in &apps {
        nested_ids(&a.path, &mut nested);
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    fill_last_used(&mut apps, runner);
    InstalledIndex {
        apps,
        nested_ids: nested,
        spotlight_ok,
    }
}

/// Executable paths of running processes (for locking running apps).
pub fn running_executables(runner: &dyn CommandRunner) -> Vec<PathBuf> {
    runner
        .query(&CommandSpec::new("/bin/ps", &["-axo", "comm="]))
        .map(|t| t.lines().map(|l| PathBuf::from(l.trim())).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;

    fn make_app(root: &Path, name: &str, id: &str) -> PathBuf {
        let app = root.join(format!("{name}.app"));
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleShortVersionString</key><string>1.2</string>
<key>CFBundleExecutable</key><string>{name}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        app
    }

    #[test]
    fn read_bundle_reads_id_name_version() {
        let d = tempfile::tempdir().unwrap();
        let app = make_app(d.path(), "Foo", "com.foo.app");
        let b = read_bundle(&app).unwrap();
        assert_eq!(
            (b.id.as_str(), b.name.as_str(), b.version.as_str()),
            ("com.foo.app", "Foo", "1.2")
        );
        assert_eq!(b.executable, app.join("Contents/MacOS/Foo"));
        assert_eq!(b.last_used, 0);
    }

    #[test]
    fn read_bundle_none_for_missing_or_garbage_plist() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_bundle(&d.path().join("None.app")).is_none());
        let app = d.path().join("Bad.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        fs::write(app.join("Contents/Info.plist"), b"not a plist \x00\xff").unwrap();
        assert!(read_bundle(&app).is_none());
    }

    #[test]
    fn read_bundle_binary_plist() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join("Bin.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        let mut dict = plist::Dictionary::new();
        dict.insert("CFBundleIdentifier".into(), "com.bin.app".into());
        plist::Value::Dictionary(dict)
            .to_file_binary(app.join("Contents/Info.plist"))
            .unwrap();
        assert_eq!(read_bundle(&app).unwrap().id, "com.bin.app");
    }

    #[test]
    fn index_counts_a_symlinked_app_once() {
        let d = tempfile::tempdir().unwrap();
        let apps = d.path().join("Applications");
        let other = d.path().join("Other");
        fs::create_dir_all(&apps).unwrap();
        fs::create_dir_all(&other).unwrap();
        let real = make_app(&other, "Linked", "com.linked.app");
        std::os::unix::fs::symlink(&real, apps.join("Linked.app")).unwrap();
        make_app(&apps, "Plain", "com.plain.app");
        // A symlinked folder must not be followed (no loops).
        std::os::unix::fs::symlink(&apps, apps.join("Loop")).unwrap();
        let idx = build_index(&[apps.clone(), apps.clone()], &FakeRunner::new(&["mdls"]));
        let mut ids: Vec<_> = idx.apps.iter().map(|a| a.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["com.linked.app", "com.plain.app"]);
    }

    #[test]
    fn nested_helper_ids_are_collected() {
        let d = tempfile::tempdir().unwrap();
        let app = make_app(d.path(), "Foo", "com.foo.app");
        make_app(
            &app.join("Contents/Library/LoginItems"),
            "Helper",
            "com.foo.helper",
        );
        let idx = build_index(&[d.path().to_path_buf()], &FakeRunner::new(&["mdls"]));
        assert!(idx.nested_ids.contains("com.foo.helper"));
        assert!(idx.has_id("com.foo.helper"));
    }

    #[test]
    fn has_id_vendor_prefix_and_vendor_check() {
        let idx = InstalledIndex {
            apps: vec![AppBundle {
                path: "/Applications/Fusion.app".into(),
                id: "com.autodesk.dls.streamer.scriptapp.Autodesk-Fusion".into(),
                name: "Fusion".into(),
                version: String::new(),
                executable: PathBuf::new(),
                owner: "u".into(),
                last_used: 0,
            }],
            ..Default::default()
        };
        assert!(idx.has_id("com.autodesk.dls.streamer"));
        assert!(!idx.has_id("com.google.x"));
        assert!(idx.vendor_has_app("com.Autodesk.streamer"));
        assert!(!idx.vendor_has_app("com.google.x"));
    }

    #[test]
    fn last_used_never_is_zero_and_dates_parse() {
        let d = tempfile::tempdir().unwrap();
        make_app(d.path(), "Foo", "com.foo.app");
        make_app(d.path(), "Bar", "com.bar.app");
        let runner = FakeRunner::new(&["mdls"])
            .answering(&[("/usr/bin/mdls", "2026-11-05 19:04:38 +0000\0(null)")]);
        let idx = build_index(&[d.path().to_path_buf()], &runner);
        let bar = idx.apps.iter().find(|a| a.id == "com.bar.app").unwrap();
        let foo = idx.apps.iter().find(|a| a.id == "com.foo.app").unwrap();
        assert_eq!(bar.last_used, 1_793_905_478);
        assert_eq!(foo.last_used, 0);
        assert_eq!(parse_md_date("1970-01-02 00:00:00 +0000"), Some(86_400));
        assert_eq!(parse_md_date("(null)"), None);
    }

    #[test]
    fn spotlight_app_inside_a_library_folder_is_an_id_not_an_app() {
        let d = tempfile::tempdir().unwrap();
        let lib = d
            .path()
            .canonicalize()
            .unwrap()
            .join("Library/Application Support/Vendor");
        let payload = make_app(&lib, "Payload", "com.vendor.payload");
        let top = d.path().canonicalize().unwrap().join("Applications");
        fs::create_dir_all(&top).unwrap();
        let top_app = make_app(&top, "Real", "com.vendor.real");
        let runner = FakeRunner::new(&["mdls"]).answering(&[(
            "mdfind",
            &format!(
                "/Applications/x\n{}\n{}",
                payload.display(),
                top_app.display()
            ),
        )]);
        let idx = build_index(&[], &runner);
        let ids: Vec<_> = idx.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["com.vendor.real"]);
        assert!(
            idx.has_id("com.vendor.payload"),
            "its id still counts as installed"
        );
    }

    #[test]
    fn spotlight_ok_false_when_query_fails() {
        let d = tempfile::tempdir().unwrap();
        let idx = build_index(&[d.path().to_path_buf()], &FakeRunner::new(&[]));
        assert!(!idx.spotlight_ok);
    }
}
