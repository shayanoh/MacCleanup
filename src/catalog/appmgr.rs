//! Apps that a package manager installed are removed with the manager's own
//! command (`brew uninstall --cask`, `port uninstall`), not by moving files.

use std::fs;
use std::path::{Path, PathBuf};

use super::apps::AppBundle;
use super::command::{CommandRunner, CommandSpec};
use super::runtimes::safe_name;

#[derive(Clone, Debug)]
pub struct ManagedApp {
    /// Id in the shared selection: `app:brew:<token>` / `app:port:<name>`.
    pub id: String,
    pub spec: CommandSpec,
}

/// Where the managers live; tests point these at a sandbox.
#[derive(Clone, Debug, Default)]
pub struct Managers {
    pub caskrooms: Vec<PathBuf>,
    pub brew: Vec<PathBuf>,
    pub port: Vec<PathBuf>,
}

impl Managers {
    pub fn system() -> Managers {
        Managers {
            caskrooms: vec![
                PathBuf::from("/opt/homebrew/Caskroom"),
                PathBuf::from("/usr/local/Caskroom"),
            ],
            brew: vec![
                PathBuf::from("/opt/homebrew/bin/brew"),
                PathBuf::from("/usr/local/bin/brew"),
            ],
            port: vec![PathBuf::from("/opt/local/bin/port")],
        }
    }
}

fn first_available(candidates: &[PathBuf], runner: &dyn CommandRunner) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|p| runner.available(&p.to_string_lossy()))
        .cloned()
}

/// The cask that installed this app: a `<token>/<version>/<App>.app` entry in
/// a Caskroom whose name is the bundle's file name.
fn cask_token(app: &Path, caskrooms: &[PathBuf]) -> Option<String> {
    let file = app.file_name()?;
    for room in caskrooms {
        let Ok(tokens) = fs::read_dir(room) else {
            continue;
        };
        for t in tokens.flatten() {
            let Ok(versions) = fs::read_dir(t.path()) else {
                continue;
            };
            for v in versions.flatten() {
                if fs::symlink_metadata(v.path().join(file)).is_ok() {
                    return Some(t.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

/// The uninstall command for an app that a manager installed, if any. A
/// missing manager program means "not a manager app".
pub fn detect(
    app: &AppBundle,
    home: &Path,
    managers: &Managers,
    runner: &dyn CommandRunner,
) -> Option<ManagedApp> {
    if app.owner == "macports"
        && let Some(port) = first_available(&managers.port, runner)
    {
        let out = runner.query(&CommandSpec::new(
            &port.to_string_lossy(),
            &["provides", &app.path.to_string_lossy()],
        ))?;
        let name = out.rsplit_once("is provided by:")?.1.trim().to_string();
        if safe_name(&name) && !name.contains('/') {
            return Some(ManagedApp {
                id: format!("app:port:{name}"),
                spec: CommandSpec::new(&port.to_string_lossy(), &["uninstall", &name])
                    .with_cwd(home),
            });
        }
        return None;
    }
    let brew = first_available(&managers.brew, runner)?;
    let token = cask_token(&app.path, &managers.caskrooms)?;
    (safe_name(&token) && !token.contains('/')).then(|| ManagedApp {
        id: format!("app:brew:{token}"),
        spec: CommandSpec::new(&brew.to_string_lossy(), &["uninstall", "--cask", &token])
            .with_cwd(home),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;

    fn app(path: &str, owner: &str) -> AppBundle {
        AppBundle {
            path: path.into(),
            id: "com.x.y".into(),
            name: "Foo".into(),
            version: String::new(),
            executable: PathBuf::new(),
            owner: owner.into(),
            last_used: 0,
        }
    }

    fn room(token: &str) -> (tempfile::TempDir, Managers) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(token).join("1.0/Foo.app")).unwrap();
        let m = Managers {
            caskrooms: vec![d.path().to_path_buf()],
            brew: vec!["/opt/homebrew/bin/brew".into()],
            port: vec!["/opt/local/bin/port".into()],
        };
        (d, m)
    }

    #[test]
    fn brew_cask_app_becomes_command_row() {
        let (_d, m) = room("foo");
        let r = FakeRunner::new(&["brew"]);
        let got = detect(
            &app("/Applications/Foo.app", "u"),
            Path::new("/home"),
            &m,
            &r,
        )
        .unwrap();
        assert_eq!(got.id, "app:brew:foo");
        assert_eq!(
            got.spec.full(),
            "/opt/homebrew/bin/brew uninstall --cask foo"
        );
    }

    #[test]
    fn port_owned_app_becomes_command_row() {
        let m = Managers::system();
        let r = FakeRunner::new(&["port"])
            .answering(&[("provides", "/Applications/Foo.app is provided by: foo-app")]);
        let got = detect(
            &app("/Applications/Foo.app", "macports"),
            Path::new("/h"),
            &m,
            &r,
        )
        .unwrap();
        assert_eq!(got.id, "app:port:foo-app");
        assert_eq!(got.spec.full(), "/opt/local/bin/port uninstall foo-app");
    }

    #[test]
    fn unsafe_token_is_refused() {
        let (_d, m) = room("foo; rm -rf");
        let r = FakeRunner::new(&["brew"]);
        assert!(detect(&app("/Applications/Foo.app", "u"), Path::new("/h"), &m, &r).is_none());
        let (_d2, m2) = room("--force");
        assert!(detect(&app("/Applications/Foo.app", "u"), Path::new("/h"), &m2, &r).is_none());
    }

    #[test]
    fn missing_brew_means_plain_folder_row() {
        let (_d, m) = room("foo");
        let r = FakeRunner::new(&[]);
        assert!(detect(&app("/Applications/Foo.app", "u"), Path::new("/h"), &m, &r).is_none());
    }

    #[test]
    fn app_not_in_any_caskroom_is_plain() {
        let (_d, m) = room("foo");
        let r = FakeRunner::new(&["brew"]);
        assert!(
            detect(
                &app("/Applications/Other.app", "u"),
                Path::new("/h"),
                &m,
                &r
            )
            .is_none()
        );
    }
}
