//! Our own tiny helper image, `maccleanup/vm-tools`, built from a Dockerfile
//! piped through stdin (no file, nothing else to trust). It does two jobs:
//! trimming the Docker VM disk and listing the files of a volume read-only.

use crate::catalog::CommandSpec;
use crate::docker::cli::DockerCli;

pub const IMAGE: &str = "maccleanup/vm-tools";
/// Bumped whenever [`DOCKERFILE`] changes, so an old image is rebuilt.
pub const VERSION: &str = "1";
/// Marks everything we create, so it is recognisable and cleanable.
pub const LABEL: &str = "com.maccleanup=1";
const VERSION_LABEL: &str = "com.maccleanup.version";

pub const DOCKERFILE: &str = "\
FROM alpine:3
LABEL com.maccleanup=1
LABEL com.maccleanup.version=1
RUN apk add --no-cache findutils
ENTRYPOINT [\"nsenter\", \"-t\", \"1\", \"-m\", \"-u\", \"-n\", \"-i\", \"fstrim\", \"-v\", \"/var/lib/docker\"]
";

/// What to look inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowseSource {
    Volume(String),
    Image(String),
}

/// `docker build -q -t maccleanup/vm-tools -` with the Dockerfile on stdin.
pub fn build_spec(cli: &DockerCli) -> CommandSpec {
    cli.cli(&["build", "-q", "-t", IMAGE, "-"])
        .with_stdin(DOCKERFILE)
}

/// Runs `fstrim` inside the Docker VM so freed space returns to the Mac.
pub fn trim_spec(cli: &DockerCli) -> CommandSpec {
    cli.cli(&[
        "run",
        "--rm",
        "--pull=never",
        "--privileged",
        "--pid=host",
        IMAGE,
    ])
}

/// A folder inside the source: an absolute path without `..`, else the root.
fn clean_dir(dir: &str) -> String {
    let ok = dir.starts_with('/') && !dir.split('/').any(|part| part == "..");
    if ok {
        dir.trim_end_matches('/').to_string()
    } else {
        String::new()
    }
}

/// The command that lists one folder of a volume or image, read-only and
/// without network. `dir` is a path inside it; anything that is not an
/// absolute path without `..` is treated as `/`.
pub fn browse_spec(cli: &DockerCli, source: &BrowseSource, dir: &str) -> CommandSpec {
    let dir = clean_dir(dir);
    match source {
        BrowseSource::Volume(name) => {
            let mount = format!("{name}:/data:ro");
            let target = format!("/data{dir}");
            cli.cli(&[
                "run",
                "--rm",
                "--pull=never",
                "--network",
                "none",
                "--label",
                LABEL,
                "-v",
                &mount,
                "--entrypoint",
                "find",
                IMAGE,
                &target,
                "-mindepth",
                "1",
                "-maxdepth",
                "1",
                "-printf",
                "%y\t%s\t%T@\t%f\0",
            ])
        }
        BrowseSource::Image(id) => {
            let target = if dir.is_empty() { "/" } else { dir.as_str() };
            cli.cli(&[
                "run",
                "--rm",
                "--pull=never",
                "--network",
                "none",
                "--read-only",
                "--label",
                LABEL,
                "--entrypoint",
                "ls",
                id,
                "-1Ap",
                target,
            ])
        }
    }
}

/// The helper image exists at the current version.
pub fn is_present(cli: &DockerCli) -> bool {
    let format = format!("{{{{index .Config.Labels \"{VERSION_LABEL}\"}}}}");
    let spec = cli.cli(&["image", "inspect", "--format", &format, IMAGE]);
    cli.runner().query(&spec).as_deref() == Some(VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use std::path::PathBuf;
    use std::sync::Arc;

    const CONTEXT_JSON: &str = include_str!("../../tests/fixtures/docker/context_inspect.json");

    fn cli_with(extra: &[(&str, &str)]) -> DockerCli {
        let mut answers = vec![("context inspect", CONTEXT_JSON), ("/_ping", "OK")];
        answers.extend_from_slice(extra);
        let r = Arc::new(FakeRunner::new(&["docker"]).answering(&answers));
        DockerCli::connect(r, &PathBuf::from("/Users/someone")).unwrap()
    }

    #[test]
    fn dockerfile_has_label_version_and_entrypoint() {
        assert!(DOCKERFILE.starts_with("FROM alpine:3\n"));
        assert!(DOCKERFILE.contains(&format!("LABEL {LABEL}\n")));
        assert!(DOCKERFILE.contains(&format!("LABEL {VERSION_LABEL}={VERSION}\n")));
        assert!(DOCKERFILE.contains(
            "ENTRYPOINT [\"nsenter\", \"-t\", \"1\", \"-m\", \"-u\", \"-n\", \"-i\", \"fstrim\", \"-v\", \"/var/lib/docker\"]"
        ));
    }

    #[test]
    fn build_spec_passes_the_dockerfile_on_stdin() {
        let spec = build_spec(&cli_with(&[]));
        assert_eq!(
            spec.args,
            ["--context=desktop-linux", "build", "-q", "-t", IMAGE, "-"]
        );
        assert_eq!(spec.stdin.as_deref(), Some(DOCKERFILE));
    }

    #[test]
    fn trim_spec_is_privileged_pid_host_and_removes_itself() {
        let spec = trim_spec(&cli_with(&[]));
        assert_eq!(
            spec.args,
            [
                "--context=desktop-linux",
                "run",
                "--rm",
                "--pull=never",
                "--privileged",
                "--pid=host",
                IMAGE
            ]
        );
    }

    #[test]
    fn browse_volume_mounts_read_only_without_network() {
        let spec = browse_spec(
            &cli_with(&[]),
            &BrowseSource::Volume("pgdata".into()),
            "/sub dir",
        );
        let joined = spec.args.join(" ");
        assert!(
            joined.contains("run --rm --pull=never --network none"),
            "{joined}"
        );
        assert!(joined.contains("-v pgdata:/data:ro"), "{joined}");
        assert!(joined.contains(&format!("--label {LABEL}")), "{joined}");
        assert!(joined.contains("--entrypoint find"), "{joined}");
        // The folder is a single argument, spaces and all.
        assert!(
            spec.args.contains(&"/data/sub dir".to_string()),
            "{:?}",
            spec.args
        );
        assert!(spec.args.contains(&"-mindepth".to_string()));
    }

    #[test]
    fn browse_image_runs_only_ls_read_only_without_network() {
        let spec = browse_spec(
            &cli_with(&[]),
            &BrowseSource::Image("sha256:abc".into()),
            "/etc",
        );
        let joined = spec.args.join(" ");
        assert!(joined.contains("--network none"), "{joined}");
        assert!(joined.contains("--read-only"), "{joined}");
        assert!(
            joined.contains("--entrypoint ls sha256:abc -1Ap /etc"),
            "{joined}"
        );
        assert!(joined.contains("--pull=never"), "{joined}");
    }

    #[test]
    fn browse_dir_cannot_escape_the_mount() {
        let v = BrowseSource::Volume("v".into());
        for bad in ["", "relative", "/a/../../etc", "/..", "../x"] {
            let spec = browse_spec(&cli_with(&[]), &v, bad);
            assert!(
                spec.args.contains(&"/data".to_string()),
                "{bad:?} -> {:?}",
                spec.args
            );
        }
        let spec = browse_spec(&cli_with(&[]), &v, "/ok/deeper");
        assert!(spec.args.contains(&"/data/ok/deeper".to_string()));
    }

    #[test]
    fn image_present_checks_the_version_label() {
        assert!(is_present(&cli_with(&[("image inspect", "1")])));
        assert!(
            !is_present(&cli_with(&[("image inspect", "0")])),
            "old version"
        );
        assert!(!is_present(&cli_with(&[])), "no image");
    }
}
