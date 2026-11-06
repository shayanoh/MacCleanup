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
    for flag in ["--ascii", "--no-animation", "--holding-dir", "[PATH]"] {
        assert!(text.contains(flag), "missing {flag}:\n{text}");
    }
    assert!(!text.contains("--root"), "the old flag is hidden:\n{text}");
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
    let out = run(home.path(), &["/definitely/not/here"]);
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
    let out = run(home.path(), &[file.to_str().unwrap()]);
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
    let out = run(home.path(), &[home.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("terminal"), "{}", stderr(&out));
    assert!(
        !home.path().join("ToBeDeleted").exists(),
        "must not create the holding folder before the UI can start"
    );
}

#[test]
fn the_old_root_flag_is_still_understood() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["--root", "/definitely/not/here"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("/definitely/not/here"), "{err}");
    assert!(
        !err.contains("unexpected argument"),
        "must not be a flag error: {err}"
    );
}

#[test]
fn dot_means_the_current_folder() {
    let home = tempfile::tempdir().unwrap();
    // `.` is resolved against the working directory. With no terminal the app
    // stops at the terminal check, which only happens once the path was
    // accepted as a real folder.
    let out = Command::new(env!("CARGO_BIN_EXE_maccleanup"))
        .arg(".")
        .current_dir(home.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("terminal"), "the path was accepted: {err}");
    assert!(!err.contains("cannot read"), "{err}");
}

#[test]
fn a_relative_path_that_does_not_exist_is_reported_by_name() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_maccleanup"))
        .arg("no-such-folder")
        .current_dir(home.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("no-such-folder"), "{}", stderr(&out));
}
