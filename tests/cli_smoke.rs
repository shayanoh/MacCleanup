use std::path::Path;
use std::process::{Command, Output};

/// Run the binary with a throwaway HOME so it can never touch real files.
fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maccleanup"))
        .args(args)
        .env("HOME", home)
        .output()
        .expect("failed to run maccleanup")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn help_lists_flags() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--help"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    for flag in ["--ascii", "--no-animation", "--root", "--holding-dir"] {
        assert!(text.contains(flag), "missing {flag}:\n{text}");
    }
}

#[test]
fn version_flag_works() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--version"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("maccleanup"));
}

#[test]
fn bad_root_exits_2() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--root", "/definitely/not/here"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("/definitely/not/here"),
        "{}",
        stderr(&out)
    );
    assert!(!home.path().join("ToBeDeleted").exists());
}

#[test]
fn root_that_is_a_file_exits_2() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("a.txt");
    std::fs::write(&file, "x").unwrap();
    let out = run(home.path(), &["--root", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("not a folder"), "{}", stderr(&out));
}

#[test]
fn missing_home_exits_2() {
    let out = Command::new(env!("CARGO_BIN_EXE_maccleanup"))
        .env_remove("HOME")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("HOME"), "{}", stderr(&out));
}

#[test]
fn non_terminal_exits_2_without_side_effects() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--root", home.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("terminal"), "{}", stderr(&out));
    assert!(
        !home.path().join("ToBeDeleted").exists(),
        "must not create the holding folder before the UI can start"
    );
}
