//! Talking to Docker Desktop: finding the `docker` program, the socket of
//! its `desktop-linux` context (never any other context), and reading the
//! Engine API over that socket with `curl`. Everything goes through the
//! `CommandRunner`, so tests use a fake and nothing real runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::bg::Work;
use crate::catalog::{CommandRunner, CommandSpec};
use crate::docker::analysis::Layers;
use crate::docker::model::*;

/// The only context this app ever talks to.
pub const CONTEXT: &str = "desktop-linux";
const CURL: &str = "/usr/bin/curl";
/// Where Docker Desktop keeps the VM disk, under the home folder.
const VM_DISK: &str = "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockerError {
    NotInstalled,
    NoDesktopContext,
    NotRunning,
    Api(String),
}

impl DockerError {
    /// What the page says.
    pub fn message(&self) -> String {
        match self {
            DockerError::NotInstalled => "Docker is not installed (no docker program found)".into(),
            DockerError::NoDesktopContext => {
                format!("Docker Desktop not found (no '{CONTEXT}' context)")
            }
            DockerError::NotRunning => {
                "Docker is not running (start Docker Desktop, then r)".into()
            }
            DockerError::Api(what) => format!("Docker did not answer: {what}"),
        }
    }
}

/// Everything one read of Docker produced.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub data: DockerData,
    pub layers: Layers,
}

#[derive(Clone)]
pub struct DockerCli {
    runner: Arc<dyn CommandRunner>,
    program: String,
    socket: PathBuf,
    home: PathBuf,
}

/// A `docker` program: on `PATH` (resolved to its full path when it can be)
/// or in one of the places Docker Desktop installs it.
fn find_docker(runner: &dyn CommandRunner) -> Option<String> {
    if runner.available("docker") {
        let on_path = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("docker"))
                .find(|c| crate::catalog::command::is_executable(c))
        });
        return Some(on_path.map_or_else(
            || "docker".to_string(),
            |p| p.to_string_lossy().into_owned(),
        ));
    }
    [
        "/Applications/Docker.app/Contents/Resources/bin/docker",
        "/usr/local/bin/docker",
    ]
    .into_iter()
    .find(|p| runner.available(p))
    .map(str::to_string)
}

impl DockerCli {
    pub fn connect(runner: Arc<dyn CommandRunner>, home: &Path) -> Result<DockerCli, DockerError> {
        let program = find_docker(runner.as_ref()).ok_or(DockerError::NotInstalled)?;
        let inspect = CommandSpec::new(&program, &["context", "inspect", CONTEXT])
            .with_cwd(home)
            .with_timeout(Duration::from_secs(20));
        let socket = runner
            .query(&inspect)
            .and_then(|json| crate::docker::api::parse_context_socket(&json).ok())
            .ok_or(DockerError::NoDesktopContext)?;
        let cli = DockerCli {
            runner,
            program,
            socket: PathBuf::from(socket),
            home: home.to_path_buf(),
        };
        match cli.api_get("/_ping") {
            Ok(body) if body.trim() == "OK" => Ok(cli),
            _ => Err(DockerError::NotRunning),
        }
    }

    /// A `docker` command pinned to the Desktop context, run from the home
    /// folder, with a generous time limit.
    pub fn cli(&self, args: &[&str]) -> CommandSpec {
        let mut all = vec![format!("--context={CONTEXT}")];
        all.extend(args.iter().map(|a| a.to_string()));
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        CommandSpec::new(&self.program, &refs)
            .with_cwd(&self.home)
            .with_timeout(Duration::from_secs(5 * 60))
    }

    /// GET an Engine API path (`/system/df`) and return the body.
    pub fn api_get(&self, path: &str) -> Result<String, DockerError> {
        let socket = self.socket.to_string_lossy();
        let url = format!("http://localhost{path}");
        let spec = CommandSpec::new(
            CURL,
            &["-s", "--fail", "--unix-socket", socket.as_ref(), &url],
        )
        .with_cwd(&self.home)
        .with_timeout(Duration::from_secs(120));
        self.runner
            .query(&spec)
            .ok_or_else(|| DockerError::Api(path.to_string()))
    }

    pub fn runner(&self) -> &Arc<dyn CommandRunner> {
        &self.runner
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Read images, containers, volumes, build cache and every image's
    /// layers. An image whose layers cannot be read is kept without them.
    pub fn load(&self, work: &Work) -> Result<Loaded, DockerError> {
        use crate::docker::api::*;
        let api = |e: ParseError| DockerError::Api(e.to_string());
        work.begin("reading Docker", 2);
        let df = parse_df(&self.api_get("/system/df")?).map_err(api)?;
        let mut containers =
            parse_containers(&self.api_get("/containers/json?all=1")?).map_err(api)?;
        work.stage(0, containers.len() + df.images.len(), "reading Docker");
        for c in &mut containers {
            if let Ok(json) = self.api_get(&format!("/containers/{}/json", c.id))
                && let Ok((started, finished)) = parse_container_times(&json)
            {
                c.started = started;
                c.finished = finished;
            }
            work.step();
        }
        let mut layers = Layers::new();
        for image in &df.images {
            let history = self.api_get(&format!("/images/{}/history", image.id));
            let inspect = self.api_get(&format!("/images/{}/json", image.id));
            if let (Ok(h), Ok(i)) = (history, inspect)
                && let Ok(list) = parse_image_layers(&h, &i)
            {
                layers.insert(image.id.clone(), list);
            }
            work.step();
        }
        Ok(Loaded {
            data: DockerData {
                images: df.images,
                containers,
                volumes: df.volumes,
                cache: df.cache,
                layers_size: df.layers_size,
                vm_allocated: vm_disk_allocated(&self.home),
            },
            layers,
        })
    }
}

/// Bytes `Docker.raw` really occupies on the Mac (allocated blocks, not the
/// sparse file's apparent size), if it exists.
pub fn vm_disk_allocated(home: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(home.join(VM_DISK))
        .ok()
        .map(|m| m.blocks() * 512)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;

    const DF: &str = include_str!("../../tests/fixtures/docker/system_df.json");
    const CONTAINERS: &str = include_str!("../../tests/fixtures/docker/containers.json");
    const CONTAINER_INSPECT: &str =
        include_str!("../../tests/fixtures/docker/container_inspect.json");
    const HISTORY: &str = include_str!("../../tests/fixtures/docker/history.json");
    const INSPECT: &str = include_str!("../../tests/fixtures/docker/inspect_image.json");
    const CONTEXT_JSON: &str = include_str!("../../tests/fixtures/docker/context_inspect.json");

    fn home() -> PathBuf {
        PathBuf::from("/Users/someone")
    }

    fn healthy() -> FakeRunner {
        FakeRunner::new(&["docker"]).answering(&[
            ("context inspect", CONTEXT_JSON),
            ("/_ping", "OK"),
            ("/system/df", DF),
            ("/containers/json?all=1", CONTAINERS),
            ("/containers/c0ffee1/json", CONTAINER_INSPECT),
            ("/images/sha256:aaa1/history", HISTORY),
            ("/images/sha256:aaa1/json", INSPECT),
        ])
    }

    #[test]
    fn connect_not_installed_when_no_program() {
        let r = Arc::new(FakeRunner::new(&[]));
        assert_eq!(
            DockerCli::connect(r, &home()).err(),
            Some(DockerError::NotInstalled)
        );
    }

    #[test]
    fn connect_no_desktop_context() {
        let r = Arc::new(FakeRunner::new(&["docker"]));
        assert_eq!(
            DockerCli::connect(r, &home()).err(),
            Some(DockerError::NoDesktopContext)
        );
    }

    #[test]
    fn connect_not_running_when_ping_fails() {
        let r =
            Arc::new(FakeRunner::new(&["docker"]).answering(&[("context inspect", CONTEXT_JSON)]));
        assert_eq!(
            DockerCli::connect(r, &home()).err(),
            Some(DockerError::NotRunning)
        );
    }

    #[test]
    fn connect_reads_the_socket_of_the_desktop_context() {
        let r = Arc::new(healthy());
        let cli = DockerCli::connect(r.clone(), &home()).unwrap();
        assert!(cli.program.ends_with("docker"));
        assert_eq!(
            cli.socket,
            PathBuf::from("/Users/someone/.docker/run/docker.sock")
        );
    }

    #[test]
    fn cli_spec_pins_context_and_cwd() {
        let cli = DockerCli::connect(Arc::new(healthy()), &home()).unwrap();
        let spec = cli.cli(&["image", "rm", "abc"]);
        assert_eq!(spec.args, ["--context=desktop-linux", "image", "rm", "abc"]);
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("/Users/someone")));
        assert!(spec.timeout.is_some());
        assert!(spec.stdin.is_none());
    }

    #[test]
    fn api_get_uses_the_socket_and_fails_when_nothing_answers() {
        let cli = DockerCli::connect(Arc::new(healthy()), &home()).unwrap();
        assert_eq!(cli.api_get("/_ping").as_deref(), Ok("OK"));
        assert!(matches!(cli.api_get("/nothing"), Err(DockerError::Api(_))));
    }

    #[test]
    fn load_builds_docker_data_and_layers() {
        let cli = DockerCli::connect(Arc::new(healthy()), &home()).unwrap();
        let work = Work::new();
        let loaded = cli.load(&work).unwrap();
        assert_eq!(loaded.data.images.len(), 2);
        assert_eq!(loaded.data.containers.len(), 3);
        assert_ne!(
            loaded.data.containers[0].started, 0,
            "times come from inspect"
        );
        assert_eq!(
            loaded.data.containers[1].started, 0,
            "no answer, no time, no failure"
        );
        assert_eq!(loaded.layers["sha256:aaa1"].len(), 5);
        assert!(
            !loaded.layers.contains_key("sha256:bbb2"),
            "an image whose layers cannot be read is kept without them"
        );
        assert_eq!(loaded.data.layers_size, 1500);
        let snap = work.snapshot();
        assert_eq!(snap.done, snap.total, "progress ends full");
    }

    #[test]
    fn load_fails_when_the_disk_usage_call_does() {
        let r = Arc::new(
            FakeRunner::new(&["docker"])
                .answering(&[("context inspect", CONTEXT_JSON), ("/_ping", "OK")]),
        );
        let cli = DockerCli::connect(r, &home()).unwrap();
        assert!(matches!(cli.load(&Work::new()), Err(DockerError::Api(_))));
    }

    #[test]
    fn vm_disk_allocated_uses_blocks_not_apparent_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(VM_DISK);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(200_000_000).unwrap();
        std::fs::write(&path, vec![1u8; 8192]).unwrap();
        f.set_len(200_000_000).unwrap();
        let allocated = vm_disk_allocated(dir.path()).unwrap();
        assert!((8192..50_000_000).contains(&allocated), "{allocated}");
        assert_eq!(vm_disk_allocated(&dir.path().join("missing")), None);
    }

    #[test]
    fn error_messages_name_the_problem() {
        assert!(DockerError::NotRunning.message().contains("not running"));
        assert!(
            DockerError::NoDesktopContext
                .message()
                .contains("desktop-linux")
        );
    }
}
