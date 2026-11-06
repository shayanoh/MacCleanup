//! Shared helpers for unit tests: build an `App` over a sandbox directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use crate::accounting::Accounting;
use crate::app::{Action, App, Screen};
use crate::glyphs::{GlyphMode, Glyphs};
use crate::safety::Policy;
use crate::scan::Scanner;
use crate::softdelete::Holding;
use crate::volume;

pub struct Env {
    pub _dir: TempDir,
    pub home: PathBuf,
    pub app: App,
}

pub fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

/// A sandbox home with the given `(relative path, size)` files, scanned to
/// completion, and an `App` rooted at it. The holding folder lives outside
/// the home folder so it never shows up as a row.
pub fn env(files: &[(&str, usize)]) -> Env {
    env_with(files, |_| {})
}

/// Like [`env`], but runs `setup(home)` (for example to create hard links)
/// after the files are written and before the scan starts.
pub fn env_with(files: &[(&str, usize)], setup: impl FnOnce(&Path)) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    for (rel, size) in files {
        write(&home.join(rel), *size);
    }
    setup(&home);
    let holding_dir = base.join("hold");
    let scanner = Scanner::new(2);
    let root = scanner.scan_root(&home);
    assert!(scanner.wait_idle(Duration::from_secs(20)), "scan timed out");
    let policy = Policy::new(&home, &holding_dir);
    let holding = Holding::open(holding_dir).unwrap();
    let free = volume::free_space(&home).unwrap();
    let mut app = App::new(
        scanner,
        root,
        policy,
        holding,
        Accounting::new(free),
        Glyphs::new(GlyphMode::Nerd),
        false,
    );
    // The app starts on Home; most tests exercise the explorer.
    app.update(Action::GoTo(Screen::Explorer));
    Env {
        _dir: dir,
        home,
        app,
    }
}

pub fn names(app: &App) -> Vec<String> {
    app.rows
        .iter()
        .map(|r| r.name.to_string_lossy().into_owned())
        .collect()
}

pub fn select(app: &mut App, name: &str) {
    app.cursor = app
        .rows
        .iter()
        .position(|r| r.name.to_string_lossy() == name)
        .unwrap_or_else(|| panic!("no row named {name}"));
    app.refresh_detail();
}

/// Tick until a running delete job has finished (the dialog leaves `Deleting`).
pub fn settle(app: &mut App) {
    for _ in 0..1000 {
        if !matches!(app.dialog, Some(crate::app::Dialog::Deleting(_))) {
            return;
        }
        app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("delete job did not finish");
}

/// Tick until the scanner has finished and the rows have been refreshed.
pub fn wait_scan(app: &mut App) {
    for _ in 0..2000 {
        app.update(Action::Tick);
        if app.scanner.is_idle() {
            app.update(Action::Tick);
            app.update(Action::Tick);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("scan did not finish");
}
