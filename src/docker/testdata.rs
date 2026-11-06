//! Builders for Docker test data, shared by the unit tests.

use std::collections::BTreeMap;

use crate::docker::model::*;

pub fn image(id: &str, tags: &[&str], size: u64) -> ImageInfo {
    ImageInfo {
        id: id.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created: 1000,
        size,
        reported_shared: 0,
        labels: BTreeMap::new(),
    }
}

pub fn layer(diff: &str, size: u64) -> LayerInfo {
    LayerInfo {
        diff_id: Some(diff.to_string()),
        command: format!("RUN {diff}"),
        size,
        created: 0,
    }
}

pub fn container(id: &str, image: &str, state: ContainerState, volumes: &[&str]) -> ContainerInfo {
    ContainerInfo {
        id: id.to_string(),
        name: id.to_string(),
        image_id: image.to_string(),
        state,
        created: 2000,
        started: 3000,
        finished: 4000,
        volumes: volumes.iter().map(|v| v.to_string()).collect(),
    }
}

pub fn volume(name: &str, size: u64) -> VolumeInfo {
    VolumeInfo {
        name: name.to_string(),
        size,
        ref_count: 0,
        created: 500,
        mountpoint: String::new(),
        labels: BTreeMap::new(),
    }
}

pub fn record(id: &str, parent: Option<&str>, size: u64, in_use: bool) -> CacheRecord {
    CacheRecord {
        id: id.to_string(),
        parents: parent.map(|p| vec![p.to_string()]).unwrap_or_default(),
        kind: "regular".to_string(),
        description: format!("step {id}"),
        size,
        in_use,
        shared: false,
        created: 100,
        last_used: 200,
        usage_count: 1,
    }
}
