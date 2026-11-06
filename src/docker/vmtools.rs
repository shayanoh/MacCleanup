//! Our own tiny helper image, `maccleanup/vm-tools`, built from a Dockerfile
//! piped through stdin (no file, nothing else to trust). It does two jobs:
//! trimming the Docker VM disk and listing the files of a volume read-only.

use crate::catalog::CommandSpec;
use crate::docker::cli::DockerCli;

pub const IMAGE: &str = "maccleanup/vm-tools";
/// Bumped whenever [`DOCKERFILE`] changes, so an old image is rebuilt.
pub const VERSION: &str = "2";
/// Marks everything we create, so it is recognisable and cleanable.
pub const LABEL: &str = "com.maccleanup=1";
const VERSION_LABEL: &str = "com.maccleanup.version";

pub const DOCKERFILE: &str = "\
FROM busybox:musl
LABEL com.maccleanup=1
LABEL com.maccleanup.version=2
ENTRYPOINT [\"nsenter\", \"-t\", \"1\", \"-m\", \"-u\", \"-n\", \"-i\", \"fstrim\", \"-v\", \"/var/lib/docker\"]
";

/// Lists the folder given as `$1`: one record per entry, `kind TAB size TAB
/// modified TAB name NUL`. A folder's size is the total of everything below
/// it (`du`), like the Explorer's; a file's is its own. Plain POSIX shell
/// with busybox-or-GNU `stat` and `du`, so it runs in any image that has a
/// shell, and the NUL ending keeps every name safe. The folder is passed as
/// an argument, never pasted into the script.
pub const LIST_SCRIPT: &str = "\
cd \"$1\" 2>/dev/null || exit 3
for f in ./* ./.[!.]* ./..?*; do
  if [ ! -e \"$f\" ] && [ ! -L \"$f\" ]; then continue; fi
  if [ -d \"$f\" ] && [ ! -L \"$f\" ]; then
    t=d
    s=$(du -sk \"$f\" 2>/dev/null | cut -f1)
    s=$(( ${s:-0} * 1024 ))
  else
    t=f
    s=$(stat -c %s \"$f\" 2>/dev/null)
  fi
  m=$(stat -c %Y \"$f\" 2>/dev/null)
  printf '%s\\t%s\\t%s\\t%s\\0' \"$t\" \"${s:-0}\" \"${m:-0}\" \"${f#./}\"
done
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
/// without network, with [`LIST_SCRIPT`]. `dir` is a path inside it;
/// anything that is not an absolute path without `..` is treated as `/`.
/// A volume is mounted read-only into our helper; an image runs only its
/// own shell with the script (its entrypoint replaced, no network, a
/// read-only root), so none of its own programs start.
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
                "sh",
                IMAGE,
                "-c",
                LIST_SCRIPT,
                "sh",
                &target,
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
                "sh",
                id,
                "-c",
                LIST_SCRIPT,
                "sh",
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
        assert!(DOCKERFILE.starts_with("FROM busybox:musl\n"));
        assert!(DOCKERFILE.contains(&format!("LABEL {LABEL}\n")));
        assert!(DOCKERFILE.contains(&format!("LABEL {VERSION_LABEL}={VERSION}\n")));
        assert!(
            !DOCKERFILE.contains("RUN "),
            "busybox has every tool: nothing to install, so building needs no package download"
        );
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
    fn browse_volume_mounts_read_only_without_network_and_runs_the_listing_script() {
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
        assert!(
            joined.contains("--entrypoint sh maccleanup/vm-tools -c"),
            "{joined}"
        );
        // The script is one argument, then $0, then the folder as its own
        // argument (never pasted into the script), spaces and all.
        let n = spec.args.len();
        assert_eq!(spec.args[n - 3], LIST_SCRIPT);
        assert_eq!(spec.args[n - 2], "sh");
        assert_eq!(spec.args[n - 1], "/data/sub dir");
    }

    #[test]
    fn the_listing_script_prints_nul_ended_records_with_folder_totals() {
        assert!(LIST_SCRIPT.contains("du -sk"), "folders are summed");
        assert!(LIST_SCRIPT.contains("stat -c %Y"), "modification time");
        assert!(LIST_SCRIPT.contains("printf '%s\\t%s\\t%s\\t%s\\0'"));
        assert!(
            LIST_SCRIPT.contains("cd \"$1\""),
            "the folder comes in as $1"
        );
    }

    #[test]
    fn browse_image_runs_only_the_listing_script_read_only_without_network() {
        let spec = browse_spec(
            &cli_with(&[]),
            &BrowseSource::Image("sha256:abc".into()),
            "/etc",
        );
        let joined = spec.args.join(" ");
        assert!(joined.contains("--network none"), "{joined}");
        assert!(joined.contains("--read-only"), "{joined}");
        assert!(joined.contains("--pull=never"), "{joined}");
        assert!(joined.contains("--entrypoint sh sha256:abc -c"), "{joined}");
        let n = spec.args.len();
        assert_eq!(spec.args[n - 3], LIST_SCRIPT);
        assert_eq!(spec.args[n - 1], "/etc");
        assert!(!joined.contains("-v "), "no volume is mounted");
    }

    #[test]
    fn browse_dir_cannot_escape_the_mount() {
        let v = BrowseSource::Volume("v".into());
        for bad in ["", "relative", "/a/../../etc", "/..", "../x"] {
            let spec = browse_spec(&cli_with(&[]), &v, bad);
            assert_eq!(spec.args.last().unwrap(), "/data", "{bad:?}");
        }
        let spec = browse_spec(&cli_with(&[]), &v, "/ok/deeper");
        assert_eq!(spec.args.last().unwrap(), "/data/ok/deeper");
    }

    #[test]
    fn image_present_checks_the_version_label() {
        assert!(is_present(&cli_with(&[("image inspect", VERSION)])));
        assert!(
            !is_present(&cli_with(&[("image inspect", "1")])),
            "old version"
        );
        assert!(!is_present(&cli_with(&[])), "no image");
    }
}
