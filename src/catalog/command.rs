//! Running other programs: cleanup commands and short discovery queries.
//! Commands never go through a shell, never read stdin, and are killed when
//! they run too long (a corepack shim waiting to download must not hang us).

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    /// Extra arguments for a forced run (`--force`); empty when the command
    /// has no such variant.
    pub force_args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
            force_args: Vec::new(),
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

    /// The arguments that force this command (e.g. `--force`).
    pub fn with_force_args(mut self, args: &[&str]) -> CommandSpec {
        self.force_args = args.iter().map(|a| a.to_string()).collect();
        self
    }

    /// Does this command have a force variant?
    pub fn can_force(&self) -> bool {
        !self.force_args.is_empty()
    }

    /// The same command with its force arguments added (itself if it has none).
    pub fn forced(&self) -> CommandSpec {
        let mut spec = self.clone();
        spec.args.extend(std::mem::take(&mut spec.force_args));
        spec
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

/// What the UI can see of a running command, and the way to stop it: the
/// latest line it printed (uv says "waiting for other uv processes" there),
/// how long it has been running, and a cancel switch.
pub struct RunControl {
    cancel: AtomicBool,
    line: Mutex<String>,
    started: Mutex<Option<Instant>>,
}

impl RunControl {
    pub fn new() -> Arc<RunControl> {
        Arc::new(RunControl {
            cancel: AtomicBool::new(false),
            line: Mutex::new(String::new()),
            started: Mutex::new(None),
        })
    }

    /// Ask the running command to stop (and everything it started).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// The most recent line of output (stdout or stderr), or empty.
    pub fn last_line(&self) -> String {
        self.line.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// How long the current command has been running (`None` before the first).
    pub fn elapsed(&self) -> Option<Duration> {
        self.started
            .lock()
            .ok()
            .and_then(|s| s.map(|t| t.elapsed()))
    }

    /// A new command starts: restart the clock and forget the old output.
    /// A cancel request stays in force.
    pub fn begin(&self) {
        if let Ok(mut s) = self.started.lock() {
            *s = Some(Instant::now());
        }
        self.set_line("");
    }

    /// Record a line of output (the runner does this as output arrives).
    pub fn set_line(&self, text: &str) {
        if let Ok(mut l) = self.line.lock() {
            *l = text.to_string();
        }
    }
}

/// Did a command stop or stall waiting for a lock another process holds?
/// (uv: "Cache is currently in-use, waiting for other uv processes" and
/// "Timeout (300s) when waiting for lock on ...".)
pub fn is_lock_wait(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("waiting for other uv processes")
        || t.contains("waiting for lock")
        || (t.contains("timeout") && t.contains("lock"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub ok: bool,
    /// Stopped on request (the user pressed cancel).
    pub cancelled: bool,
    /// The end of what the command printed (or why it could not start).
    pub output_tail: String,
}

/// The seam between the app and the outside world; tests use a fake.
pub trait CommandRunner: Send + Sync {
    /// Run a cleanup command to the end, or until `ctl` is cancelled.
    fn run(&self, spec: &CommandSpec, ctl: &RunControl) -> CommandResult;
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
    cancelled: bool,
    stdout: String,
    stderr: String,
}

fn tail(s: &str) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(TAIL_CHARS)).collect()
}

/// Read a stream to the end, keeping everything for the report and handing
/// each finished line (and any unfinished one) to `ctl` as it arrives.
fn read_stream(mut r: impl Read, ctl: Option<&RunControl>) -> String {
    let mut all = Vec::new();
    let mut line: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        all.extend_from_slice(&chunk[..n]);
        for &b in &chunk[..n] {
            // A carriage return redraws a progress line: it ends a line too.
            if b == b'\n' || b == b'\r' {
                if let (false, Some(ctl)) = (line.is_empty(), ctl) {
                    ctl.set_line(String::from_utf8_lossy(&line).trim());
                }
                line.clear();
            } else {
                line.push(b);
            }
        }
        // Output that has not ended its line yet (a prompt) still counts.
        if let (false, Some(ctl)) = (line.is_empty(), ctl) {
            ctl.set_line(String::from_utf8_lossy(&line).trim());
        }
    }
    String::from_utf8_lossy(&all).into_owned()
}

/// Stop a command and everything it started: ask politely, then insist.
fn stop(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    if pid > 1 {
        // The child leads its own process group, so this reaches its children
        // and never our own process.
        unsafe { libc::killpg(pid, libc::SIGTERM) };
    }
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    if pid > 1 {
        unsafe { libc::killpg(pid, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn exec(spec: &CommandSpec, timeout: Duration, ctl: Option<&RunControl>) -> Outcome {
    let failed = |why: String| Outcome {
        ok: false,
        timed_out: false,
        cancelled: false,
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
        .stderr(Stdio::piped())
        // Its own process group: cancelling reaches everything it started,
        // and a Ctrl-C meant for this app does not hit it behind our back.
        .process_group(0);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    if let Some(ctl) = ctl {
        ctl.begin();
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("could not start {}: {e}", spec.program)),
    };
    let (out, err) = (child.stdout.take(), child.stderr.take());
    thread::scope(|scope| {
        let out = out.map(|o| scope.spawn(move || read_stream(o, ctl)));
        let err = err.map(|e| scope.spawn(move || read_stream(e, ctl)));
        let started = Instant::now();
        let (mut ok, mut timed_out, mut cancelled) = (false, false, false);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    ok = status.success();
                    break;
                }
                Ok(None) if ctl.is_some_and(RunControl::cancelled) => {
                    stop(&mut child);
                    cancelled = true;
                    break;
                }
                Ok(None) if started.elapsed() >= timeout => {
                    stop(&mut child);
                    timed_out = true;
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(15)),
                Err(_) => break,
            }
        }
        let join = |h: Option<thread::ScopedJoinHandle<'_, String>>| {
            h.and_then(|h| h.join().ok()).unwrap_or_default()
        };
        Outcome {
            ok,
            timed_out,
            cancelled,
            stdout: join(out),
            stderr: join(err),
        }
    })
}

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec, ctl: &RunControl) -> CommandResult {
        let o = exec(spec, RUN_TIMEOUT, Some(ctl));
        let mut text = format!("{}{}", o.stdout, o.stderr);
        if o.timed_out {
            text.push_str("\n(stopped: it ran too long)");
        }
        if o.cancelled {
            text.push_str("\n(cancelled)");
        }
        CommandResult {
            ok: o.ok,
            cancelled: o.cancelled,
            output_tail: tail(&text),
        }
    }

    fn query(&self, spec: &CommandSpec) -> Option<String> {
        let o = exec(spec, QUERY_TIMEOUT, None);
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
    type Probe = Box<dyn Fn(&CommandSpec) + Send + Sync>;

    pub struct FakeRunner {
        programs: Vec<String>,
        failing: Vec<String>,
        answers: Vec<(String, String)>,
        calls: Mutex<Vec<CommandSpec>>,
        delay: std::time::Duration,
        probe: Option<Probe>,
        wait_for_cancel: bool,
        prints: Option<String>,
        fail_outputs: Vec<(String, String)>,
    }

    impl FakeRunner {
        pub fn new(programs: &[&str]) -> FakeRunner {
            FakeRunner {
                programs: programs.iter().map(|p| p.to_string()).collect(),
                failing: Vec::new(),
                answers: Vec::new(),
                calls: Mutex::new(Vec::new()),
                delay: std::time::Duration::ZERO,
                probe: None,
                wait_for_cancel: false,
                prints: None,
                fail_outputs: Vec::new(),
            }
        }

        /// Every `run` takes this long (to prove the UI does not wait).
        pub fn slow(mut self, delay: std::time::Duration) -> FakeRunner {
            self.delay = delay;
            self
        }

        /// Every command "prints" this line as soon as it starts.
        pub fn printing(mut self, line: &str) -> FakeRunner {
            self.prints = Some(line.to_string());
            self
        }

        /// Commands whose `display()` is the key fail with that output.
        pub fn failing_with(mut self, outputs: &[(&str, &str)]) -> FakeRunner {
            self.fail_outputs = outputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self
        }

        /// Every `run` blocks until it is cancelled (a command stuck on a lock).
        pub fn waiting_for_cancel(mut self) -> FakeRunner {
            self.wait_for_cancel = true;
            self
        }

        /// Called with each command just before it "runs".
        pub fn probing(mut self, f: impl Fn(&CommandSpec) + Send + Sync + 'static) -> FakeRunner {
            self.probe = Some(Box::new(f));
            self
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
        fn run(&self, spec: &CommandSpec, ctl: &RunControl) -> CommandResult {
            ctl.begin();
            if let Some(line) = &self.prints {
                ctl.set_line(line);
            }
            self.calls.lock().unwrap().push(spec.clone());
            if let Some(p) = &self.probe {
                p(spec);
            }
            // Sleep in small steps so a cancel is noticed at once.
            let until = Instant::now() + self.delay;
            while (self.wait_for_cancel || Instant::now() < until) && !ctl.cancelled() {
                thread::sleep(Duration::from_millis(10));
            }
            if ctl.cancelled() {
                return CommandResult {
                    ok: false,
                    cancelled: true,
                    output_tail: "(cancelled)".to_string(),
                };
            }
            if let Some((_, out)) = self.fail_outputs.iter().find(|(k, _)| *k == spec.display()) {
                return CommandResult {
                    ok: false,
                    cancelled: false,
                    output_tail: out.clone(),
                };
            }
            let ok = !self.failing.contains(&spec.display());
            CommandResult {
                ok,
                cancelled: false,
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

    /// Run with a fresh, never-cancelled control.
    trait RunExt {
        fn go(&self, spec: &CommandSpec) -> CommandResult;
    }

    impl<T: CommandRunner + ?Sized> RunExt for T {
        fn go(&self, spec: &CommandSpec) -> CommandResult {
            self.run(spec, &RunControl::new())
        }
    }

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
        assert!(r.go(&spec("true", &[])).ok);
        assert!(!r.go(&spec("false", &[])).ok);
    }

    #[test]
    fn system_runner_reports_a_missing_program_as_a_failure() {
        let res = SystemRunner.go(&spec("definitely-not-a-program-xyz", &[]));
        assert!(!res.ok);
        assert!(!res.output_tail.is_empty());
    }

    #[test]
    fn system_runner_never_uses_a_shell() {
        let res = SystemRunner.go(&spec("echo", &["; echo hi"]));
        assert!(res.ok);
        assert_eq!(res.output_tail.trim(), "; echo hi");
    }

    #[test]
    fn system_runner_keeps_only_the_tail_of_long_output() {
        let long = "x".repeat(5000);
        let res = SystemRunner.go(&spec("echo", &[&long]));
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
            SystemRunner.go(&s).output_tail.trim(),
            real.to_string_lossy()
        );
        let s = spec("sh", &["-c", "printf %s \"$MC_TEST_VAR\""]).with_env("MC_TEST_VAR", "hello");
        assert_eq!(SystemRunner.go(&s).output_tail, "hello");
    }

    #[test]
    fn system_runner_gives_the_command_no_stdin() {
        // `cat` would wait forever on a terminal; with no stdin it ends at once.
        let res = SystemRunner.go(&spec("cat", &[]));
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
            None,
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
        assert!(f.go(&spec("pnpm", &["store", "prune"])).ok);
        assert!(!f.go(&spec("brew", &["cleanup"])).ok);
        assert_eq!(f.calls().len(), 2);
    }

    #[test]
    fn a_command_can_offer_a_force_variant() {
        let plain = spec("uv", &["cache", "clean"]);
        assert!(!plain.can_force());
        let forceable = plain.clone().with_force_args(&["--force"]);
        assert!(forceable.can_force());
        let forced = forceable.forced();
        assert_eq!(forced.display(), "uv cache clean --force");
        assert!(!forced.can_force(), "forcing twice is not a thing");
        assert_eq!(plain.forced(), plain, "no variant: unchanged");
    }

    #[test]
    fn lock_waits_are_recognised_from_uvs_messages() {
        for text in [
            "Cache is currently in-use, waiting for other uv processes to finish (use `--force` to override)",
            "error: Timeout (300s) when waiting for lock on `/Users/x/.cache/uv` at `/Users/x/.cache/uv/.lock`, is another uv process running?",
        ] {
            assert!(is_lock_wait(text), "{text}");
        }
        for text in ["", "done", "Removed 12 files", "error: permission denied"] {
            assert!(!is_lock_wait(text), "{text}");
        }
    }

    #[test]
    fn cancelling_stops_a_running_command_at_once() {
        let ctl = RunControl::new();
        let c = std::sync::Arc::clone(&ctl);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c.cancel();
        });
        let started = Instant::now();
        let res = SystemRunner.run(&spec("sleep", &["30"]), &ctl);
        canceller.join().unwrap();
        assert!(res.cancelled && !res.ok);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn cancelling_kills_the_commands_own_children_too() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let ctl = RunControl::new();
        let c = std::sync::Arc::clone(&ctl);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            c.cancel();
        });
        // A shell that starts a grandchild and waits for it.
        let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
        let res = SystemRunner.run(&spec("sh", &["-c", &script]), &ctl);
        canceller.join().unwrap();
        assert!(res.cancelled);
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the grandchild {pid} must be gone");
    }

    #[test]
    fn a_command_that_finishes_is_not_marked_cancelled() {
        let ctl = RunControl::new();
        let res = SystemRunner.run(&spec("true", &[]), &ctl);
        assert!(res.ok && !res.cancelled);
        // Cancelling afterwards changes nothing about an earlier result.
        ctl.cancel();
        assert!(res.ok);
    }

    #[test]
    fn the_last_line_of_output_is_available_while_the_command_runs() {
        let ctl = RunControl::new();
        let c = std::sync::Arc::clone(&ctl);
        let watcher = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(50));
                let l = c.last_line();
                if !l.is_empty() && seen.last() != Some(&l) {
                    seen.push(l);
                }
                if seen.len() >= 2 {
                    break;
                }
            }
            c.cancel();
            seen
        });
        let script = "echo one; sleep 0.3; echo two >&2; sleep 30";
        let res = SystemRunner.run(&spec("sh", &["-c", script]), &ctl);
        let seen = watcher.join().unwrap();
        assert!(res.cancelled);
        assert_eq!(seen, ["one", "two"], "stdout and stderr both show up");
    }

    #[test]
    fn carriage_return_progress_lines_show_the_latest_state() {
        let ctl = RunControl::new();
        let script = "printf 'a\\rb\\rc'; sleep 0.3";
        SystemRunner.run(&spec("sh", &["-c", script]), &ctl);
        assert_eq!(ctl.last_line(), "c");
    }

    #[test]
    fn elapsed_time_counts_from_the_start_of_the_command() {
        let ctl = RunControl::new();
        assert_eq!(ctl.elapsed(), None, "nothing is running yet");
        SystemRunner.run(&spec("sleep", &["0.3"]), &ctl);
        let e = ctl.elapsed().unwrap();
        assert!(e >= Duration::from_millis(250), "{e:?}");
    }

    #[test]
    fn a_fresh_control_forgets_the_previous_commands_output() {
        let ctl = RunControl::new();
        SystemRunner.run(&spec("echo", &["old"]), &ctl);
        assert_eq!(ctl.last_line(), "old");
        ctl.begin();
        assert_eq!(ctl.last_line(), "");
    }

    #[test]
    fn the_fake_runner_can_wait_until_it_is_cancelled() {
        let f = std::sync::Arc::new(FakeRunner::new(&[]).waiting_for_cancel());
        let ctl = RunControl::new();
        let (f2, c2) = (std::sync::Arc::clone(&f), std::sync::Arc::clone(&ctl));
        let h = std::thread::spawn(move || f2.run(&spec("x", &[]), &c2));
        std::thread::sleep(Duration::from_millis(100));
        ctl.cancel();
        let res = h.join().unwrap();
        assert!(res.cancelled && !res.ok);
    }

    /// Real uv against a throwaway cache whose lock is held: the waiting
    /// message must reach `last_line`, and cancelling must really kill uv.
    /// Needs uv and python3: `cargo test real_uv -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_uv_waits_on_a_held_lock_and_cancel_kills_it() {
        let Some(uv) = [
            "~/.local/bin/uv",
            "/usr/local/bin/uv",
            "/opt/homebrew/bin/uv",
        ]
        .iter()
        .map(|p| p.replace('~', &std::env::var("HOME").unwrap()))
        .find(|p| std::path::Path::new(p).exists()) else {
            return;
        };
        let cache = tempfile::tempdir().unwrap();
        let lock = cache.path().join(".lock");
        std::fs::write(&lock, "").unwrap();
        // Another "uv process" holding the cache lock (a shared flock).
        let mut holder = std::process::Command::new("python3")
            .args([
                "-I",
                "-c",
                "import fcntl,os,sys,time; fd=os.open(sys.argv[1],os.O_RDONLY); \
                 fcntl.flock(fd, fcntl.LOCK_SH); print('held', flush=True); time.sleep(120)",
                lock.to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(holder.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        assert_eq!(line.trim(), "held");

        let ctl = RunControl::new();
        let c = Arc::clone(&ctl);
        let watcher = thread::spawn(move || {
            let mut seen = String::new();
            for _ in 0..100 {
                thread::sleep(Duration::from_millis(100));
                seen = c.last_line();
                if !seen.is_empty() {
                    break;
                }
            }
            let elapsed = c.elapsed();
            c.cancel();
            (seen, elapsed)
        });
        let s =
            spec(&uv, &["cache", "clean"]).with_env("UV_CACHE_DIR", cache.path().to_str().unwrap());
        let started = Instant::now();
        let res = SystemRunner.run(&s, &ctl);
        let (seen, elapsed) = watcher.join().unwrap();
        println!(
            "uv said: {seen:?} after {elapsed:?}; stopped after {:?}",
            started.elapsed()
        );
        let _ = holder.kill();
        let _ = holder.wait();
        assert!(res.cancelled, "{res:?}");
        assert!(
            seen.contains("in-use") || seen.contains("waiting"),
            "{seen:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}
