//! Running other programs: cleanup commands and short discovery queries.
//! Commands never go through a shell, never read stdin, and are killed when
//! they run too long (a corepack shim waiting to download must not hang us).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a cleanup command may run.
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long a discovery query (`pnpm store path`) may run.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// How much of a command's output is kept for the report.
const TAIL_CHARS: usize = 2000;

/// A program with explicit arguments. `program` is a name looked up on
/// `PATH` or an absolute path (discovery pins the exact install).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn with_cwd(mut self, cwd: &Path) -> CommandSpec {
        self.cwd = Some(cwd.to_path_buf());
        self
    }

    pub fn with_env(mut self, key: &str, value: &str) -> CommandSpec {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// `pnpm store prune`: the program's file name and its arguments.
    pub fn display(&self) -> String {
        let name = Path::new(&self.program).file_name().map_or_else(
            || self.program.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
        join(&name, &self.args)
    }

    /// The same with the program as given (the full path when pinned).
    pub fn full(&self) -> String {
        join(&self.program, &self.args)
    }
}

fn join(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub ok: bool,
    /// The end of what the command printed (or why it could not start).
    pub output_tail: String,
}

/// The seam between the app and the outside world; tests use a fake.
pub trait CommandRunner: Send + Sync {
    /// Run a cleanup command to the end.
    fn run(&self, spec: &CommandSpec) -> CommandResult;
    /// Run a short read-only command and return its trimmed stdout, or
    /// `None` if it failed, timed out or could not start.
    fn query(&self, spec: &CommandSpec) -> Option<String>;
    /// Is this program (a name on `PATH`, or an absolute path) runnable?
    fn available(&self, program: &str) -> bool;
}

pub struct SystemRunner;

struct Outcome {
    ok: bool,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

fn tail(s: &str) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(TAIL_CHARS)).collect()
}

fn read_all(mut r: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn exec(spec: &CommandSpec, timeout: Duration) -> Outcome {
    let failed = |why: String| Outcome {
        ok: false,
        timed_out: false,
        stdout: String::new(),
        stderr: why,
    };
    if spec.program.is_empty() {
        return failed("no program given".to_string());
    }
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("could not start {}: {e}", spec.program)),
    };
    let out = child.stdout.take().map(read_all);
    let err = child.stderr.take().map(read_all);
    let started = Instant::now();
    let (mut ok, mut timed_out) = (false, false);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                ok = status.success();
                break;
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(_) => break,
        }
    }
    let join =
        |h: Option<thread::JoinHandle<String>>| h.and_then(|h| h.join().ok()).unwrap_or_default();
    Outcome {
        ok,
        timed_out,
        stdout: join(out),
        stderr: join(err),
    }
}

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> CommandResult {
        let o = exec(spec, RUN_TIMEOUT);
        let mut text = format!("{}{}", o.stdout, o.stderr);
        if o.timed_out {
            text.push_str("\n(stopped: it ran too long)");
        }
        CommandResult {
            ok: o.ok,
            output_tail: tail(&text),
        }
    }

    fn query(&self, spec: &CommandSpec) -> Option<String> {
        let o = exec(spec, QUERY_TIMEOUT);
        o.ok.then(|| o.stdout.trim().to_string())
    }

    fn available(&self, program: &str) -> bool {
        if program.is_empty() || program.contains("..") {
            return false;
        }
        if program.contains('/') {
            return is_executable(Path::new(program));
        }
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| is_executable(&d.join(program))))
    }
}

pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
pub(crate) use fake::FakeRunner;

#[cfg(test)]
mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Records every command and never touches the system. `answers` are
    /// matched as substrings of `CommandSpec::full()` for queries.
    pub struct FakeRunner {
        programs: Vec<String>,
        failing: Vec<String>,
        answers: Vec<(String, String)>,
        calls: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        pub fn new(programs: &[&str]) -> FakeRunner {
            FakeRunner {
                programs: programs.iter().map(|p| p.to_string()).collect(),
                failing: Vec::new(),
                answers: Vec::new(),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Commands whose `display()` equals one of these fail.
        pub fn failing(mut self, displays: &[&str]) -> FakeRunner {
            self.failing = displays.iter().map(|d| d.to_string()).collect();
            self
        }

        pub fn answering(mut self, answers: &[(&str, &str)]) -> FakeRunner {
            self.answers = answers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self
        }

        pub fn calls(&self) -> Vec<CommandSpec> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> CommandResult {
            self.calls.lock().unwrap().push(spec.clone());
            let ok = !self.failing.contains(&spec.display());
            CommandResult {
                ok,
                output_tail: if ok { "done" } else { "failed" }.to_string(),
            }
        }

        fn query(&self, spec: &CommandSpec) -> Option<String> {
            let full = spec.full();
            self.answers
                .iter()
                .find(|(k, _)| full.contains(k.as_str()))
                .map(|(_, v)| v.clone())
        }

        fn available(&self, program: &str) -> bool {
            let name = Path::new(program)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.programs.contains(&name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec::new(program, args)
    }

    #[test]
    fn command_spec_display_joins_program_and_args() {
        assert_eq!(
            spec("pnpm", &["store", "prune"]).display(),
            "pnpm store prune"
        );
        assert_eq!(spec("brew", &[]).display(), "brew");
    }

    #[test]
    fn system_runner_reports_success_and_failure() {
        let r = SystemRunner;
        assert!(r.run(&spec("true", &[])).ok);
        assert!(!r.run(&spec("false", &[])).ok);
    }

    #[test]
    fn system_runner_reports_a_missing_program_as_a_failure() {
        let res = SystemRunner.run(&spec("definitely-not-a-program-xyz", &[]));
        assert!(!res.ok);
        assert!(!res.output_tail.is_empty());
    }

    #[test]
    fn system_runner_never_uses_a_shell() {
        let res = SystemRunner.run(&spec("echo", &["; echo hi"]));
        assert!(res.ok);
        assert_eq!(res.output_tail.trim(), "; echo hi");
    }

    #[test]
    fn system_runner_keeps_only_the_tail_of_long_output() {
        let long = "x".repeat(5000);
        let res = SystemRunner.run(&spec("echo", &[&long]));
        assert!(res.output_tail.chars().count() <= 2000);
    }

    #[test]
    fn available_finds_programs_on_path_only() {
        assert!(SystemRunner.available("echo"));
        assert!(!SystemRunner.available("definitely-not-a-program-xyz"));
        assert!(!SystemRunner.available("../echo"));
    }

    #[test]
    fn system_runner_applies_cwd_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        let s = spec("pwd", &[]).with_cwd(&real);
        assert_eq!(
            SystemRunner.run(&s).output_tail.trim(),
            real.to_string_lossy()
        );
        let s = spec("sh", &["-c", "printf %s \"$MC_TEST_VAR\""]).with_env("MC_TEST_VAR", "hello");
        assert_eq!(SystemRunner.run(&s).output_tail, "hello");
    }

    #[test]
    fn system_runner_gives_the_command_no_stdin() {
        // `cat` would wait forever on a terminal; with no stdin it ends at once.
        let res = SystemRunner.run(&spec("cat", &[]));
        assert!(res.ok);
    }

    #[test]
    fn query_returns_stdout_and_none_on_failure() {
        assert_eq!(
            SystemRunner.query(&spec("echo", &["hi"])).as_deref(),
            Some("hi")
        );
        assert_eq!(SystemRunner.query(&spec("false", &[])), None);
        assert_eq!(
            SystemRunner.query(&spec("definitely-not-a-program-xyz", &[])),
            None
        );
    }

    #[test]
    fn a_hanging_command_is_killed_at_the_timeout() {
        let started = std::time::Instant::now();
        let out = exec(
            &spec("sleep", &["30"]),
            std::time::Duration::from_millis(200),
        );
        assert!(!out.ok);
        assert!(out.timed_out);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn full_shows_the_whole_program_path() {
        let s = spec("/opt/x/bin/pnpm", &["store", "prune"]);
        assert_eq!(s.display(), "pnpm store prune");
        assert_eq!(s.full(), "/opt/x/bin/pnpm store prune");
    }

    #[test]
    fn fake_runner_records_calls_and_can_fail_by_label() {
        let f = FakeRunner::new(&["pnpm"]).failing(&["brew cleanup"]);
        assert!(f.available("pnpm"));
        assert!(!f.available("brew"));
        assert!(f.run(&spec("pnpm", &["store", "prune"])).ok);
        assert!(!f.run(&spec("brew", &["cleanup"])).ok);
        assert_eq!(f.calls().len(), 2);
    }
}
