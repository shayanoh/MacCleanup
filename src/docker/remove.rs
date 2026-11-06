//! The commands that remove Docker items. Docker is always cleaned through
//! its own commands, never by touching files in the VM disk. Nothing here
//! adds `--force`: if Docker refuses, the user is told.

use crate::catalog::CommandSpec;
use crate::docker::cli::DockerCli;
use crate::docker::model::DockerData;

/// The command that removes the item `id` (`container:..`, `image:..`,
/// `volume:..`, `cache:..`), or `None` for an id that is not one of those
/// or no longer exists in `data`.
pub fn removal_spec(cli: &DockerCli, data: &DockerData, id: &str) -> Option<CommandSpec> {
    if let Some(raw) = id.strip_prefix("container:") {
        data.containers.iter().find(|c| c.id == raw)?;
        return Some(cli.cli(&["container", "rm", raw]));
    }
    if let Some(raw) = id.strip_prefix("image:") {
        let image = data.images.iter().find(|i| i.id == raw)?;
        let mut args = vec!["image", "rm"];
        if image.tags.is_empty() {
            args.push(raw);
        } else {
            // By tag: readable in the review, and an id with several tags is
            // refused unless forced. Removing every tag removes the image
            // when the last one goes.
            args.extend(image.tags.iter().map(String::as_str));
        }
        return Some(cli.cli(&args));
    }
    if let Some(name) = id.strip_prefix("volume:") {
        data.volumes.iter().find(|v| v.name == name)?;
        return Some(cli.cli(&["volume", "rm", name]));
    }
    if let Some(raw) = id.strip_prefix("cache:") {
        data.cache.iter().find(|r| r.id == raw)?;
        let filter = format!("id={raw}");
        return Some(cli.cli(&["builder", "prune", "-f", "--filter", &filter]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::command::FakeRunner;
    use crate::docker::model::ContainerState;
    use crate::docker::testdata::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    const CONTEXT_JSON: &str = include_str!("../../tests/fixtures/docker/context_inspect.json");

    fn cli() -> DockerCli {
        let r = Arc::new(
            FakeRunner::new(&["docker"])
                .answering(&[("context inspect", CONTEXT_JSON), ("/_ping", "OK")]),
        );
        DockerCli::connect(r, &PathBuf::from("/Users/someone")).unwrap()
    }

    fn data() -> DockerData {
        DockerData {
            images: vec![
                image("sha256:one", &["a:1"], 1),
                image("sha256:many", &["b:1", "b:latest"], 1),
                image("sha256:none", &[], 1),
            ],
            containers: vec![container("c1", "sha256:one", ContainerState::Stopped, &[])],
            volumes: vec![volume("data", 1)],
            cache: vec![record("rec1", None, 1, false)],
            ..DockerData::default()
        }
    }

    fn args(id: &str) -> Vec<String> {
        removal_spec(&cli(), &data(), id).unwrap().args
    }

    #[test]
    fn every_command_is_pinned_to_the_desktop_context() {
        for id in [
            "container:c1",
            "image:sha256:one",
            "volume:data",
            "cache:rec1",
        ] {
            assert_eq!(args(id)[0], "--context=desktop-linux", "{id}");
        }
    }

    #[test]
    fn containers_volumes_and_single_tag_images_are_removed_by_name_or_id() {
        assert_eq!(args("container:c1")[1..], ["container", "rm", "c1"]);
        assert_eq!(args("volume:data")[1..], ["volume", "rm", "data"]);
        assert_eq!(
            args("image:sha256:one")[1..],
            ["image", "rm", "a:1"],
            "a tagged image is removed by its tag: readable in the review"
        );
        assert_eq!(
            args("image:sha256:none")[1..],
            ["image", "rm", "sha256:none"],
            "a dangling one has only its id"
        );
    }

    #[test]
    fn an_image_with_several_tags_is_removed_through_all_its_tags() {
        assert_eq!(
            args("image:sha256:many")[1..],
            ["image", "rm", "b:1", "b:latest"],
            "Docker refuses an id that has several tags unless forced"
        );
    }

    #[test]
    fn a_cache_record_is_pruned_by_its_id_without_asking() {
        assert_eq!(
            args("cache:rec1")[1..],
            ["builder", "prune", "-f", "--filter", "id=rec1"]
        );
    }

    #[test]
    fn no_command_ever_forces() {
        for id in ["container:c1", "image:sha256:many", "volume:data"] {
            let a = args(id);
            assert!(!a.iter().any(|x| x == "-f" || x == "--force"), "{a:?}");
        }
    }

    #[test]
    fn unknown_things_have_no_command() {
        assert!(removal_spec(&cli(), &data(), "image:gone").is_none());
        assert!(removal_spec(&cli(), &data(), "cache:gone").is_none());
        assert!(removal_spec(&cli(), &data(), "volume:gone").is_none());
        assert!(removal_spec(&cli(), &data(), "container:gone").is_none());
        assert!(removal_spec(&cli(), &data(), "vm:trim").is_none());
        assert!(removal_spec(&cli(), &data(), "pnpm").is_none());
        assert!(removal_spec(&cli(), &data(), "").is_none());
    }
}
