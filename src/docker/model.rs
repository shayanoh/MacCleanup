//! Plain data for what Docker holds. Times are Unix seconds (0 = unknown),
//! sizes are exact bytes.

use std::collections::BTreeMap;

/// Names one thing on the Docker page and in the shared selection:
/// `image:<id>`, `container:<id>`, `volume:<name>`, `cache:<id>` or
/// `vm:trim`.
pub type ItemId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// Full id, `sha256:...`.
    pub id: String,
    /// `name:tag` strings; empty for a dangling image.
    pub tags: Vec<String>,
    pub created: i64,
    pub size: u64,
    /// Docker's own shared-size figure (not trusted; kept for comparison).
    pub reported_shared: i64,
    pub labels: BTreeMap<String, String>,
}

/// One layer of an image, oldest first in a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerInfo {
    /// The layer's content id (`sha256:...`), when it could be matched.
    pub diff_id: Option<String>,
    /// The Dockerfile step that made it.
    pub command: String,
    pub size: u64,
    pub created: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Stopped,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image_id: String,
    pub state: ContainerState,
    pub created: i64,
    pub started: i64,
    pub finished: i64,
    /// Named volumes it mounts.
    pub volumes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeInfo {
    pub name: String,
    /// Bytes, 0 when Docker did not measure it.
    pub size: u64,
    /// Containers using it; -1 when Docker did not say.
    pub ref_count: i64,
    pub created: i64,
    pub mountpoint: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRecord {
    pub id: String,
    pub parents: Vec<String>,
    pub kind: String,
    /// The build step that made it (`[4/8] RUN pnpm install`).
    pub description: String,
    pub size: u64,
    pub in_use: bool,
    pub shared: bool,
    pub created: i64,
    pub last_used: i64,
    pub usage_count: u64,
}

/// Everything the page needs from one read of Docker.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DockerData {
    pub images: Vec<ImageInfo>,
    pub containers: Vec<ContainerInfo>,
    pub volumes: Vec<VolumeInfo>,
    pub cache: Vec<CacheRecord>,
    /// Bytes Docker reports its layers take.
    pub layers_size: u64,
    /// Bytes `Docker.raw` really occupies on the Mac, when it could be read.
    pub vm_allocated: Option<u64>,
}

impl DockerData {
    /// Bytes Docker says are in use inside the VM (images + volumes + cache).
    pub fn in_use(&self) -> u64 {
        self.layers_size
            + self.volumes.iter().map(|v| v.size).sum::<u64>()
            + self.cache.iter().map(|c| c.size).sum::<u64>()
    }
}
