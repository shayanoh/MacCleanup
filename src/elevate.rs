//! Moving root-owned items into the ToBeDeleted folder through the system's
//! own password prompt. This app never asks for, reads or stores a password:
//! `osascript` shows macOS's dialog, and we only learn whether it succeeded.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::catalog::{CommandRunner, CommandSpec, RunControl};
use crate::safety::{Policy, Safety};

/// One item to move with administrator rights.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElevatedMove {
    pub from: PathBuf,
    /// The holding slot's payload path (see `Holding::reserve`).
    pub to: PathBuf,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ElevateError {
    #[error("the password prompt was cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

/// The seam for the privileged step; tests use a fake.
pub trait Elevator: Send + Sync {
    /// Move every item, then give the moved copies to the current user.
    /// `Ok` means the batch ran; the caller checks each item afterwards.
    fn move_all(&self, moves: &[ElevatedMove]) -> Result<(), ElevateError>;
}

/// Used when no elevation is wanted (plain `DeleteJob::start`).
pub struct NoElevator;

impl Elevator for NoElevator {
    fn move_all(&self, _: &[ElevatedMove]) -> Result<(), ElevateError> {
        Err(ElevateError::Failed(
            "needs administrator rights".to_string(),
        ))
    }
}

/// The login name of the current user.
pub fn current_user() -> Option<String> {
    // SAFETY: getpwuid returns static storage or null; copied out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// The script is constant. The data arrives as separate `argv` items and is
/// shell-quoted with AppleScript's `quoted form of`, so no name can break out.
const SCRIPT: &str = r#"on run argv
set owner to item 1 of argv
set cmds to {}
repeat with i from 2 to (count of argv) by 2
set f to item i of argv
set t to item (i + 1) of argv
set end of cmds to "/bin/mv -- " & quoted form of f & " " & quoted form of t & " && /usr/sbin/chown -R " & quoted form of owner & " " & quoted form of t
end repeat
set AppleScript's text item delimiters to "; "
do shell script (cmds as text) with administrator privileges
end run"#;

pub struct OsascriptElevator {
    pub runner: Arc<dyn CommandRunner>,
    pub policy: Arc<Policy>,
    /// `<holding>/items`: every destination must be `<items>/<id>/payload`.
    pub items_dir: PathBuf,
}

fn plain_absolute(p: &Path) -> bool {
    p.is_absolute() && !p.components().any(|c| matches!(c, Component::ParentDir))
}

fn safe_user(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

impl OsascriptElevator {
    /// Last look before root is involved. A failure refuses the whole batch.
    fn validate(&self, m: &ElevatedMove) -> Result<(), ElevateError> {
        let bad = |why: &str| ElevateError::Failed(format!("refused {:?}: {why}", m.from));
        if !plain_absolute(&m.from) || !plain_absolute(&m.to) {
            return Err(bad("paths must be absolute without .."));
        }
        if self.policy.classify_real(&m.from) == Safety::Protected {
            return Err(bad("protected"));
        }
        let ok_to = m.to.parent().and_then(Path::parent) == Some(self.items_dir.as_path())
            && m.to.file_name().is_some_and(|n| n == "payload");
        if !ok_to {
            return Err(bad("destination is not a holding slot"));
        }
        Ok(())
    }
}

impl Elevator for OsascriptElevator {
    fn move_all(&self, moves: &[ElevatedMove]) -> Result<(), ElevateError> {
        if moves.is_empty() {
            return Ok(());
        }
        for m in moves {
            self.validate(m)?;
        }
        let user = current_user()
            .filter(|u| safe_user(u))
            .ok_or_else(|| ElevateError::Failed("cannot tell who is logged in".to_string()))?;
        let mut args: Vec<String> = vec!["-e".into(), SCRIPT.into(), user];
        for m in moves {
            args.push(m.from.to_string_lossy().into_owned());
            args.push(m.to.to_string_lossy().into_owned());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        // The user may take a while to type the password.
        let spec =
            CommandSpec::new("/usr/bin/osascript", &refs).with_timeout(Duration::from_secs(600));
        let res = self.runner.run(&spec, &RunControl::new());
        if res.ok {
            Ok(())
        } else if res.cancelled
            || res.output_tail.contains("-128")
            || res.output_tail.to_lowercase().contains("user canceled")
        {
            Err(ElevateError::Cancelled)
        } else {
            Err(ElevateError::Failed(res.output_tail))
        }
    }
}

#[cfg(test)]
pub use fake::FakeElevator;

#[cfg(test)]
mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Does the moves itself (as the test user) and records each batch.
    pub struct FakeElevator {
        pub batches: Mutex<Vec<Vec<ElevatedMove>>>,
        pub result: Mutex<Result<(), ElevateError>>,
        pub really_move: bool,
    }

    impl FakeElevator {
        pub fn moving() -> FakeElevator {
            FakeElevator {
                batches: Mutex::new(Vec::new()),
                result: Mutex::new(Ok(())),
                really_move: true,
            }
        }

        pub fn failing(e: ElevateError) -> FakeElevator {
            FakeElevator {
                batches: Mutex::new(Vec::new()),
                result: Mutex::new(Err(e)),
                really_move: false,
            }
        }
    }

    impl Elevator for FakeElevator {
        fn move_all(&self, moves: &[ElevatedMove]) -> Result<(), ElevateError> {
            self.batches.lock().unwrap().push(moves.to_vec());
            let r = self.result.lock().unwrap().clone();
            if r.is_ok() && self.really_move {
                for m in moves {
                    // Act like root: the parent folder is not writable for us.
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(parent) = m.from.parent() {
                        let _ = std::fs::set_permissions(
                            parent,
                            std::fs::Permissions::from_mode(0o755),
                        );
                    }
                    std::fs::rename(&m.from, &m.to)
                        .map_err(|e| ElevateError::Failed(e.to_string()))?;
                }
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;

    fn elevator(runner: Arc<FakeRunner>) -> (tempfile::TempDir, OsascriptElevator, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().canonicalize().unwrap();
        let home = base.join("home");
        let hold = base.join("hold");
        std::fs::create_dir_all(home.join("x")).unwrap();
        std::fs::create_dir_all(hold.join("items/1")).unwrap();
        let e = OsascriptElevator {
            runner,
            policy: Arc::new(Policy::new(&home, &hold)),
            items_dir: hold.join("items"),
        };
        (d, e, base)
    }

    #[test]
    fn script_receives_paths_as_argv() {
        let runner = Arc::new(FakeRunner::new(&[]));
        let (_d, e, base) = elevator(runner.clone());
        let weird = PathBuf::from("/Applications/It's \"a\" $(rm -rf ~) `x`\nApp.app");
        let m = ElevatedMove {
            from: weird.clone(),
            to: base.join("hold/items/1/payload"),
        };
        e.move_all(std::slice::from_ref(&m)).unwrap();
        let call = &runner.calls()[0];
        assert_eq!(call.program, "/usr/bin/osascript");
        assert!(call.args.contains(&weird.to_string_lossy().into_owned()));
        assert!(call.args.contains(&m.to.to_string_lossy().into_owned()));
        // The script text is constant and does not contain the data.
        let script = &call.args[1];
        assert!(!script.contains("rm -rf") && !script.contains("Applications"));
        assert!(script.contains("with administrator privileges"));
        assert!(script.contains("quoted form of"));
    }

    #[test]
    fn revalidation_refuses_protected_or_relative_paths() {
        let runner = Arc::new(FakeRunner::new(&[]));
        let (_d, e, base) = elevator(runner.clone());
        let to = base.join("hold/items/1/payload");
        for from in [
            "relative/Foo.app",
            "/Applications/Safari.app",
            "/etc/hosts",
            "/Applications/../etc/hosts",
        ] {
            let m = ElevatedMove {
                from: from.into(),
                to: to.clone(),
            };
            assert!(e.move_all(&[m]).is_err(), "{from}");
        }
        let bad_to = ElevatedMove {
            from: "/Applications/Foo.app".into(),
            to: "/tmp/elsewhere".into(),
        };
        assert!(e.move_all(&[bad_to]).is_err());
        assert!(runner.calls().is_empty(), "nothing reached osascript");
    }

    /// A runner whose every command fails with this text.
    struct Failing(&'static str);

    impl CommandRunner for Failing {
        fn run(&self, _: &CommandSpec, _: &RunControl) -> crate::catalog::CommandResult {
            crate::catalog::CommandResult {
                ok: false,
                cancelled: false,
                output_tail: self.0.to_string(),
            }
        }
        fn query(&self, _: &CommandSpec) -> Option<String> {
            None
        }
        fn available(&self, _: &str) -> bool {
            true
        }
    }

    #[test]
    fn cancel_maps_to_cancelled_and_other_errors_to_failed() {
        for (text, want) in [
            (
                "execution error: User canceled. (-128)",
                ElevateError::Cancelled,
            ),
            (
                "mv: Permission denied",
                ElevateError::Failed("mv: Permission denied".into()),
            ),
        ] {
            let (_d, mut e, base) = elevator(Arc::new(FakeRunner::new(&[])));
            e.runner = Arc::new(Failing(text));
            let m = ElevatedMove {
                from: "/Applications/Foo.app".into(),
                to: base.join("hold/items/1/payload"),
            };
            assert_eq!(e.move_all(&[m]), Err(want));
        }
    }
}
