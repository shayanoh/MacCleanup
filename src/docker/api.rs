//! Reading the Docker Engine API's JSON into the model. Every parser returns
//! an error on malformed input instead of panicking, and tolerates the
//! nulls, negative numbers and missing fields Docker really produces.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::docker::model::*;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("unreadable Docker JSON: {0}")]
    Json(String),
}

/// What `/system/df` holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DfData {
    pub images: Vec<ImageInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub cache: Vec<CacheRecord>,
    pub layers_size: u64,
}

fn bad(e: impl std::fmt::Display) -> ParseError {
    ParseError::Json(e.to_string())
}

/// Docker uses -1 for "not measured": sizes below zero count as zero.
fn bytes(v: i64) -> u64 {
    v.max(0) as u64
}

fn time(text: Option<&str>) -> i64 {
    text.and_then(parse_rfc3339).unwrap_or(0)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDf {
    #[serde(rename = "LayersSize")]
    layers_size: i64,
    #[serde(rename = "Images")]
    images: Option<Vec<RawImage>>,
    #[serde(rename = "Volumes")]
    volumes: Option<Vec<RawVolume>>,
    #[serde(rename = "BuildCache")]
    cache: Option<Vec<RawCache>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawImage {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "RepoTags")]
    tags: Option<Vec<String>>,
    #[serde(rename = "Created")]
    created: i64,
    #[serde(rename = "Size")]
    size: i64,
    #[serde(rename = "SharedSize")]
    shared: i64,
    #[serde(rename = "Labels")]
    labels: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawVolume {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Mountpoint")]
    mountpoint: String,
    #[serde(rename = "CreatedAt")]
    created: Option<String>,
    #[serde(rename = "Labels")]
    labels: Option<BTreeMap<String, String>>,
    #[serde(rename = "UsageData")]
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawUsage {
    #[serde(rename = "RefCount")]
    ref_count: i64,
    #[serde(rename = "Size")]
    size: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCache {
    #[serde(rename = "ID")]
    id: String,
    // Docker really spells this key with a leading space.
    #[serde(rename = " Parents", alias = "Parents")]
    parents: Option<Vec<String>>,
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Description")]
    description: String,
    #[serde(rename = "InUse")]
    in_use: bool,
    #[serde(rename = "Shared")]
    shared: bool,
    #[serde(rename = "Size")]
    size: i64,
    #[serde(rename = "CreatedAt")]
    created: Option<String>,
    #[serde(rename = "LastUsedAt")]
    last_used: Option<String>,
    #[serde(rename = "UsageCount")]
    usage_count: i64,
}

pub fn parse_df(json: &str) -> Result<DfData, ParseError> {
    let raw: RawDf = serde_json::from_str(json).map_err(bad)?;
    Ok(DfData {
        layers_size: bytes(raw.layers_size),
        images: raw
            .images
            .unwrap_or_default()
            .into_iter()
            .map(|i| ImageInfo {
                id: i.id,
                tags: i.tags.unwrap_or_default(),
                created: i.created,
                size: bytes(i.size),
                reported_shared: i.shared,
                labels: i.labels.unwrap_or_default(),
            })
            .collect(),
        volumes: raw
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| VolumeInfo {
                size: v.usage.as_ref().map_or(0, |u| bytes(u.size)),
                ref_count: v.usage.as_ref().map_or(-1, |u| u.ref_count),
                created: time(v.created.as_deref()),
                name: v.name,
                mountpoint: v.mountpoint,
                labels: v.labels.unwrap_or_default(),
            })
            .collect(),
        cache: raw
            .cache
            .unwrap_or_default()
            .into_iter()
            .map(|c| CacheRecord {
                id: c.id,
                parents: c.parents.unwrap_or_default(),
                kind: c.kind,
                description: c.description,
                size: bytes(c.size),
                in_use: c.in_use,
                shared: c.shared,
                created: time(c.created.as_deref()),
                last_used: time(c.last_used.as_deref()),
                usage_count: bytes(c.usage_count),
            })
            .collect(),
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawContainer {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names")]
    names: Option<Vec<String>>,
    #[serde(rename = "ImageID")]
    image_id: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Created")]
    created: i64,
    #[serde(rename = "Mounts")]
    mounts: Option<Vec<RawMount>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMount {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Name")]
    name: String,
}

pub fn parse_containers(json: &str) -> Result<Vec<ContainerInfo>, ParseError> {
    let raw: Vec<RawContainer> = serde_json::from_str(json).map_err(bad)?;
    Ok(raw
        .into_iter()
        .map(|c| ContainerInfo {
            name: c
                .names
                .as_deref()
                .and_then(<[String]>::first)
                .map(|n| n.trim_start_matches('/').to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| c.id.clone()),
            state: match c.state.as_str() {
                "running" => ContainerState::Running,
                "exited" | "created" | "dead" => ContainerState::Stopped,
                _ => ContainerState::Other,
            },
            image_id: c.image_id,
            created: c.created,
            started: 0,
            finished: 0,
            volumes: c
                .mounts
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.kind == "volume" && !m.name.is_empty())
                .map(|m| m.name)
                .collect(),
            id: c.id,
        })
        .collect())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawInspect {
    #[serde(rename = "State")]
    state: RawState,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawState {
    #[serde(rename = "StartedAt")]
    started: Option<String>,
    #[serde(rename = "FinishedAt")]
    finished: Option<String>,
}

/// `(started, finished)` from a container's inspect JSON.
pub fn parse_container_times(json: &str) -> Result<(i64, i64), ParseError> {
    let raw: RawInspect = serde_json::from_str(json).map_err(bad)?;
    Ok((
        time(raw.state.started.as_deref()),
        time(raw.state.finished.as_deref()),
    ))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStep {
    #[serde(rename = "Created")]
    created: i64,
    #[serde(rename = "CreatedBy")]
    command: String,
    #[serde(rename = "Size")]
    size: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawImageInspect {
    #[serde(rename = "RootFS")]
    root_fs: RawRootFs,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRootFs {
    #[serde(rename = "Layers")]
    layers: Option<Vec<String>>,
}

/// An image's layers, oldest first, from its history and inspect JSON. The
/// steps that made a layer (size above zero) are matched with the image's
/// layer ids in order; if the counts differ nothing is matched.
pub fn parse_image_layers(
    history_json: &str,
    inspect_json: &str,
) -> Result<Vec<LayerInfo>, ParseError> {
    let inspect: RawImageInspect = serde_json::from_str(inspect_json).map_err(bad)?;
    let mut steps: Vec<RawStep> = serde_json::from_str(history_json).map_err(bad)?;
    steps.reverse();
    let ids = inspect.root_fs.layers.unwrap_or_default();
    let real = steps.iter().filter(|s| s.size > 0).count();
    let matched = real == ids.len();
    let mut next = ids.into_iter();
    Ok(steps
        .into_iter()
        .map(|s| LayerInfo {
            diff_id: if matched && s.size > 0 {
                next.next()
            } else {
                None
            },
            command: s.command,
            size: bytes(s.size),
            created: s.created,
        })
        .collect())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawContext {
    #[serde(rename = "Endpoints")]
    endpoints: RawEndpoints,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEndpoints {
    docker: RawHost,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawHost {
    #[serde(rename = "Host")]
    host: String,
}

/// The unix socket path of `docker context inspect` output.
pub fn parse_context_socket(json: &str) -> Result<String, ParseError> {
    let list: Vec<RawContext> = serde_json::from_str(json).map_err(bad)?;
    let host = list
        .first()
        .map(|c| c.endpoints.docker.host.as_str())
        .unwrap_or_default();
    match host.strip_prefix("unix://") {
        Some(path) if path.starts_with('/') => Ok(path.to_string()),
        _ => Err(bad(format!("not a local unix socket: {host:?}"))),
    }
}

/// `2026-10-23T15:00:23.42436762Z` or with a `+03:30` offset, as Unix
/// seconds; `None` when it is not RFC 3339 or is Go's zero time.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if year < 1970 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        rest = &frac[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && matches!(rest.as_bytes()[0], b'+' | b'-') => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let h: i64 = rest.get(1..3)?.parse().ok()?;
            let m: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (h * 3600 + m * 60)
        }
        _ => return None,
    };
    // Days since 1970-01-01 (Howard Hinnant's civil-from-days, inverted).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DF: &str = include_str!("../../tests/fixtures/docker/system_df.json");
    const CONTAINERS: &str = include_str!("../../tests/fixtures/docker/containers.json");
    const CONTAINER_INSPECT: &str =
        include_str!("../../tests/fixtures/docker/container_inspect.json");
    const HISTORY: &str = include_str!("../../tests/fixtures/docker/history.json");
    const INSPECT: &str = include_str!("../../tests/fixtures/docker/inspect_image.json");
    const CONTEXT: &str = include_str!("../../tests/fixtures/docker/context_inspect.json");

    #[test]
    fn df_gives_exact_bytes() {
        let d = parse_df(DF).unwrap();
        assert_eq!(d.layers_size, 1500);
        assert_eq!(d.images.len(), 2);
        let web = &d.images[0];
        assert_eq!(web.id, "sha256:aaa1");
        assert_eq!(web.tags, ["web:latest"]);
        assert_eq!(
            (web.size, web.created, web.reported_shared),
            (1000, 1792000000, 400)
        );
        assert_eq!(web.labels["org.opencontainers.image.title"], "Web");
        // Dangling: null tags and labels become empty, a negative figure is kept as is.
        let dangling = &d.images[1];
        assert!(dangling.tags.is_empty() && dangling.labels.is_empty());
        assert_eq!(dangling.reported_shared, -1);
        let pg = &d.volumes[0];
        assert_eq!((pg.size, pg.ref_count), (66363194, 1));
        assert_eq!(pg.created, parse_rfc3339("2026-09-10T23:16:21Z").unwrap());
        assert_eq!(pg.labels["com.docker.compose.project"], "shop");
    }

    #[test]
    fn unmeasured_volume_has_zero_size_and_unknown_refs() {
        let v = &parse_df(DF).unwrap().volumes[1];
        assert_eq!((v.size, v.ref_count), (0, -1));
    }

    #[test]
    fn cache_parents_key_with_leading_space() {
        let c = parse_df(DF).unwrap().cache;
        assert_eq!(c[1].parents, ["root1"], "Docker spells the key ' Parents'");
        assert!(c[0].parents.is_empty());
        assert_eq!(
            c[1].description,
            "mount / from exec /bin/sh -c apt-get update"
        );
        assert_eq!((c[1].size, c[1].usage_count), (1297818308, 2));
        assert!(c[0].shared && !c[1].shared);
    }

    #[test]
    fn last_used_parses_nanosecond_rfc3339_and_null_is_zero() {
        let c = parse_df(DF).unwrap().cache;
        assert_eq!(
            c[0].last_used,
            parse_rfc3339("2026-10-23T15:00:23Z").unwrap()
        );
        assert_eq!(c[2].last_used, 0);
        assert!(c[2].in_use);
    }

    #[test]
    fn rfc3339_handles_offsets_fractions_and_zero_time() {
        assert_eq!(
            parse_rfc3339("2026-10-23T15:00:23Z"),
            Some(1_792_767_623),
            "checked against `date -u -j`"
        );
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(parse_rfc3339("1970-01-01T03:30:00+03:30"), Some(0));
        assert_eq!(
            parse_rfc3339("2026-10-23T15:00:23.999999999Z"),
            parse_rfc3339("2026-10-23T15:00:23Z")
        );
        assert_eq!(parse_rfc3339("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("not a time"), None);
        assert_eq!(parse_rfc3339(""), None);
    }

    #[test]
    fn containers_parse_state_and_named_volumes_only() {
        let c = parse_containers(CONTAINERS).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].name, "web-1");
        assert_eq!(c[0].state, ContainerState::Running);
        assert_eq!(c[0].volumes, ["pgdata"], "bind mounts are not volumes");
        assert_eq!(c[0].image_id, "sha256:aaa1");
        assert_eq!(c[1].state, ContainerState::Stopped);
        assert_eq!(c[2].state, ContainerState::Other);
        assert_eq!(c[2].name, c[2].id, "a nameless container shows its id");
        assert!(c[2].volumes.is_empty(), "null mounts are fine");
    }

    #[test]
    fn container_times_come_from_inspect() {
        let (started, finished) = parse_container_times(CONTAINER_INSPECT).unwrap();
        assert_eq!(started, parse_rfc3339("2026-11-06T10:00:00Z").unwrap());
        assert_eq!(finished, 0, "Go's zero time means never");
    }

    #[test]
    fn history_layers_align_with_diff_ids() {
        let layers = parse_image_layers(HISTORY, INSPECT).unwrap();
        // Oldest first, every history step kept, empty steps without a diff id.
        let cmds: Vec<&str> = layers.iter().map(|l| l.command.as_str()).collect();
        assert_eq!(
            cmds,
            [
                "ADD alpine-minirootfs.tar.gz / # buildkit",
                "COPY package.json ./ # buildkit",
                "WORKDIR /app",
                "RUN /bin/sh -c pnpm install # buildkit",
                "CMD [\"node\"]"
            ]
        );
        let ids: Vec<Option<&str>> = layers.iter().map(|l| l.diff_id.as_deref()).collect();
        assert_eq!(
            ids,
            [
                Some("sha256:l1"),
                Some("sha256:l2"),
                None,
                Some("sha256:l3"),
                None
            ]
        );
        assert_eq!(layers[0].size, 7_000_000);
    }

    #[test]
    fn mismatched_layer_counts_give_no_diff_ids() {
        let inspect = r#"{"RootFS":{"Layers":["sha256:only-one"]}}"#;
        let layers = parse_image_layers(HISTORY, inspect).unwrap();
        assert!(layers.iter().all(|l| l.diff_id.is_none()));
        assert_eq!(layers.len(), 5);
    }

    #[test]
    fn context_socket_path() {
        assert_eq!(
            parse_context_socket(CONTEXT).unwrap(),
            "/Users/someone/.docker/run/docker.sock"
        );
        let tcp = r#"[{"Endpoints":{"docker":{"Host":"tcp://10.0.0.1:2376"}}}]"#;
        assert!(
            parse_context_socket(tcp).is_err(),
            "only unix sockets are local"
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        for bad in ["", "{", "[]", "null", "42", "{\"Images\": 3}"] {
            let _ = parse_df(bad);
            let _ = parse_containers(bad);
            let _ = parse_container_times(bad);
            let _ = parse_image_layers(bad, bad);
            let _ = parse_context_socket(bad);
        }
        assert!(parse_df("").is_err());
        assert!(parse_containers("{").is_err());
        assert!(parse_image_layers("[]", "").is_err());
    }
}
