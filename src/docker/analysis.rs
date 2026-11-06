//! What the raw Docker data means: the tree of what holds what, and exact
//! shared / unique / frees numbers worked out from layer identity (Docker's
//! own "unique size" can be negative, so it is never used).

use std::collections::{BTreeSet, HashMap};

use crate::docker::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Image,
    Container,
    Volume,
    CacheRecord,
    VmTrim,
}

/// Why something cannot be marked at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Held by containers that are running (or in a state that is not
    /// "stopped"); their names.
    Running(Vec<String>),
    /// A build step in use by a running build.
    CacheInUse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFacts {
    pub kind: ItemKind,
    /// A tag for an image, else a short id or the name.
    pub name: String,
    pub size: u64,
    /// What removing just this item frees inside Docker.
    pub frees_alone: u64,
    /// Bytes of its layers that other images also hold (stay behind).
    pub kept_by_others: u64,
    pub created: i64,
    pub last_used: i64,
    /// `last_used` is worked out from containers, not recorded by Docker.
    pub last_used_is_proxy: bool,
    /// Other things that hold this one: containers using an image or volume.
    pub holders: Vec<ItemId>,
    /// Stopped containers that must be removed first (Docker refuses
    /// otherwise): an image's containers, or a volume's.
    pub needs: Vec<ItemId>,
    pub removable: Result<(), Refusal>,
    /// May be taken by a group or section mark.
    pub bulk_ok: bool,
    /// Layer information was missing, so shared figures are a guess.
    pub approximate: bool,
    /// An image with no tag and no container.
    pub dangling: bool,
    /// Nothing uses it.
    pub unused: bool,
    /// For a cache record: how many records lie below it.
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub id: ItemId,
    pub children: Vec<TreeNode>,
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    /// Images, each holding its containers, each holding the volumes it is
    /// the first holder of.
    pub images: Vec<TreeNode>,
    /// Containers whose image no longer exists.
    pub orphan_containers: Vec<TreeNode>,
    /// Volumes no container uses.
    pub volumes: Vec<TreeNode>,
    /// Build cache records, children under their (first) parent.
    pub cache: Vec<TreeNode>,
    pub items: HashMap<ItemId, ItemFacts>,
}

pub type Layers = HashMap<String, Vec<LayerInfo>>;

pub fn image_item(id: &str) -> ItemId {
    format!("image:{id}")
}
pub fn container_item(id: &str) -> ItemId {
    format!("container:{id}")
}
pub fn volume_item(name: &str) -> ItemId {
    format!("volume:{name}")
}
pub fn cache_item(id: &str) -> ItemId {
    format!("cache:{id}")
}
pub const TRIM_ITEM: &str = "vm:trim";

/// Who holds each identified layer, and how big it is.
struct LayerIndex {
    /// diff id -> (size, ids of the images holding it)
    holders: HashMap<String, (u64, BTreeSet<String>)>,
}

impl LayerIndex {
    fn new(data: &DockerData, layers: &Layers) -> LayerIndex {
        let mut holders: HashMap<String, (u64, BTreeSet<String>)> = HashMap::new();
        for image in &data.images {
            for l in layers.get(&image.id).into_iter().flatten() {
                if let (Some(diff), true) = (&l.diff_id, l.size > 0) {
                    let e = holders.entry(diff.clone()).or_default();
                    e.0 = e.0.max(l.size);
                    e.1.insert(image.id.clone());
                }
            }
        }
        LayerIndex { holders }
    }
}

/// Identified layers of an image (id, size), each id once.
fn identified(image: &ImageInfo, layers: &Layers) -> Vec<(String, u64)> {
    let mut seen = BTreeSet::new();
    layers
        .get(&image.id)
        .into_iter()
        .flatten()
        .filter_map(|l| match (&l.diff_id, l.size > 0) {
            (Some(d), true) if seen.insert(d.clone()) => Some((d.clone(), l.size)),
            _ => None,
        })
        .collect()
}

/// Bytes of an image no identified layer accounts for (config, manifest,
/// layers that could not be matched): they go with the image alone.
fn unattributed(image: &ImageInfo, layers: &Layers) -> u64 {
    let known: u64 = identified(image, layers).iter().map(|(_, s)| s).sum();
    image.size.saturating_sub(known)
}

fn short_id(id: &str) -> String {
    id.strip_prefix("sha256:")
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

fn is_stopped(c: &ContainerInfo) -> bool {
    c.state == ContainerState::Stopped
}

pub fn analyse(data: &DockerData, layers: &Layers) -> Analysis {
    let index = LayerIndex::new(data, layers);
    let mut items: HashMap<ItemId, ItemFacts> = HashMap::new();

    // Containers by name, so every "first holder" rule is deterministic.
    let mut containers: Vec<&ContainerInfo> = data.containers.iter().collect();
    containers.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    let newest = |list: &[&ContainerInfo]| -> Option<i64> {
        list.iter()
            .map(|c| c.started.max(c.finished).max(c.created))
            .max()
    };
    let busy = |list: &[&ContainerInfo]| -> Result<(), Refusal> {
        let names: Vec<String> = list
            .iter()
            .filter(|c| !is_stopped(c))
            .map(|c| c.name.clone())
            .collect();
        if names.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Running(names))
        }
    };

    for image in &data.images {
        let mine: Vec<&ContainerInfo> = containers
            .iter()
            .copied()
            .filter(|c| c.image_id == image.id)
            .collect();
        let known = identified(image, layers);
        let kept: u64 = known
            .iter()
            .filter(|(d, _)| index.holders[d].1.len() > 1)
            .map(|(_, s)| s)
            .sum::<u64>()
            .min(image.size);
        let approximate = layers.get(&image.id).is_none_or(|l| l.is_empty())
            || layers
                .get(&image.id)
                .into_iter()
                .flatten()
                .any(|l| l.size > 0 && l.diff_id.is_none());
        let dangling = image.tags.is_empty() && mine.is_empty();
        let removable = busy(&mine);
        items.insert(
            image_item(&image.id),
            ItemFacts {
                kind: ItemKind::Image,
                name: image
                    .tags
                    .first()
                    .cloned()
                    .unwrap_or_else(|| short_id(&image.id)),
                size: image.size,
                frees_alone: image.size - kept,
                kept_by_others: kept,
                created: image.created,
                last_used: newest(&mine).unwrap_or(image.created),
                last_used_is_proxy: true,
                holders: mine.iter().map(|c| container_item(&c.id)).collect(),
                needs: mine
                    .iter()
                    .filter(|c| is_stopped(c))
                    .map(|c| container_item(&c.id))
                    .collect(),
                bulk_ok: dangling && removable.is_ok(),
                removable,
                approximate,
                dangling,
                unused: mine.is_empty(),
                depth: 0,
            },
        );
    }

    for c in &containers {
        let t = c.started.max(c.finished).max(c.created);
        items.insert(
            container_item(&c.id),
            ItemFacts {
                kind: ItemKind::Container,
                name: c.name.clone(),
                size: 0,
                frees_alone: 0,
                kept_by_others: 0,
                created: c.created,
                last_used: t,
                last_used_is_proxy: false,
                holders: Vec::new(),
                needs: Vec::new(),
                removable: busy(&[*c]),
                bulk_ok: false,
                approximate: false,
                dangling: false,
                unused: false,
                depth: 0,
            },
        );
    }

    // Volumes: holders are the containers mounting them, by name.
    let holders_of = |name: &str| -> Vec<&ContainerInfo> {
        containers
            .iter()
            .copied()
            .filter(|c| c.volumes.iter().any(|v| v == name))
            .collect()
    };
    for v in &data.volumes {
        let mine = holders_of(&v.name);
        items.insert(
            volume_item(&v.name),
            ItemFacts {
                kind: ItemKind::Volume,
                name: v.name.clone(),
                size: v.size,
                frees_alone: v.size,
                kept_by_others: 0,
                created: v.created,
                last_used: newest(&mine).unwrap_or(v.created),
                last_used_is_proxy: true,
                holders: mine.iter().map(|c| container_item(&c.id)).collect(),
                needs: mine
                    .iter()
                    .filter(|c| is_stopped(c))
                    .map(|c| container_item(&c.id))
                    .collect(),
                removable: busy(&mine),
                bulk_ok: false,
                approximate: false,
                dangling: false,
                unused: mine.is_empty(),
                depth: 0,
            },
        );
    }

    // Build cache: depth along the first-parent chain.
    let by_id: HashMap<&str, &CacheRecord> =
        data.cache.iter().map(|r| (r.id.as_str(), r)).collect();
    let first_parent = |r: &CacheRecord| -> Option<&CacheRecord> {
        r.parents
            .first()
            .and_then(|p| by_id.get(p.as_str()).copied())
    };
    for r in &data.cache {
        let (mut depth, mut cur) = (0u32, r);
        while let Some(p) = first_parent(cur) {
            depth += 1;
            cur = p;
            if depth > 10_000 {
                break; // a loop in the data
            }
        }
        items.insert(
            cache_item(&r.id),
            ItemFacts {
                kind: ItemKind::CacheRecord,
                name: r.description.clone(),
                size: r.size,
                frees_alone: r.size,
                kept_by_others: 0,
                created: r.created,
                last_used: if r.last_used > 0 {
                    r.last_used
                } else {
                    r.created
                },
                last_used_is_proxy: false,
                holders: Vec::new(),
                needs: Vec::new(),
                removable: if r.in_use {
                    Err(Refusal::CacheInUse)
                } else {
                    Ok(())
                },
                bulk_ok: !r.in_use,
                approximate: false,
                dangling: false,
                unused: !r.in_use,
                depth,
            },
        );
    }

    // The VM disk file: what a trim can give back, at most.
    if let Some(raw) = data.vm_allocated {
        items.insert(
            TRIM_ITEM.to_string(),
            ItemFacts {
                kind: ItemKind::VmTrim,
                name: "Shrink VM disk".to_string(),
                size: raw,
                frees_alone: raw.saturating_sub(data.in_use()),
                kept_by_others: 0,
                created: 0,
                last_used: 0,
                last_used_is_proxy: false,
                holders: Vec::new(),
                needs: Vec::new(),
                removable: Ok(()),
                bulk_ok: false,
                approximate: false,
                dangling: false,
                unused: false,
                depth: 0,
            },
        );
    }

    // The trees.
    let volume_node = |name: &str| TreeNode {
        id: volume_item(name),
        children: Vec::new(),
    };
    let first_holder_is = |volume: &str, container: &ContainerInfo| -> bool {
        holders_of(volume)
            .first()
            .is_some_and(|c| c.id == container.id)
    };
    let container_node = |c: &ContainerInfo| TreeNode {
        id: container_item(&c.id),
        children: {
            let mut vols: Vec<&String> = c
                .volumes
                .iter()
                .filter(|v| data.volumes.iter().any(|x| &x.name == *v) && first_holder_is(v, c))
                .collect();
            vols.sort();
            vols.dedup();
            vols.into_iter().map(|v| volume_node(v)).collect()
        },
    };
    let mut images: Vec<&ImageInfo> = data.images.iter().collect();
    images.sort_by_key(|i| (items[&image_item(&i.id)].name.to_lowercase(), i.id.clone()));
    let image_ids: BTreeSet<&str> = data.images.iter().map(|i| i.id.as_str()).collect();
    let mut analysis = Analysis {
        images: images
            .iter()
            .map(|i| TreeNode {
                id: image_item(&i.id),
                children: containers
                    .iter()
                    .filter(|c| c.image_id == i.id)
                    .map(|c| container_node(c))
                    .collect(),
            })
            .collect(),
        orphan_containers: containers
            .iter()
            .filter(|c| !image_ids.contains(c.image_id.as_str()))
            .map(|c| container_node(c))
            .collect(),
        volumes: {
            let mut free: Vec<&VolumeInfo> = data
                .volumes
                .iter()
                .filter(|v| holders_of(&v.name).is_empty())
                .collect();
            free.sort_by(|a, b| a.name.cmp(&b.name));
            free.into_iter().map(|v| volume_node(&v.name)).collect()
        },
        cache: Vec::new(),
        items,
    };

    fn cache_node(r: &CacheRecord, data: &DockerData) -> TreeNode {
        let mut kids: Vec<&CacheRecord> = data
            .cache
            .iter()
            .filter(|c| c.parents.first() == Some(&r.id))
            .collect();
        kids.sort_by(|a, b| a.id.cmp(&b.id));
        TreeNode {
            id: cache_item(&r.id),
            children: kids.into_iter().map(|k| cache_node(k, data)).collect(),
        }
    }
    let mut roots: Vec<&CacheRecord> = data
        .cache
        .iter()
        .filter(|r| first_parent(r).is_none())
        .collect();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    analysis.cache = roots.into_iter().map(|r| cache_node(r, data)).collect();
    analysis
}

/// What removing everything in `marked` frees inside Docker, as a set: a
/// layer held by two images frees only when both are in the set.
pub fn frees_of_set(
    a: &Analysis,
    data: &DockerData,
    layers: &Layers,
    marked: &BTreeSet<ItemId>,
) -> u64 {
    let index = LayerIndex::new(data, layers);
    let marked_images: BTreeSet<String> = data
        .images
        .iter()
        .filter(|i| marked.contains(&image_item(&i.id)))
        .map(|i| i.id.clone())
        .collect();
    let mut total = 0u64;
    let mut freed_layers: BTreeSet<String> = BTreeSet::new();
    for image in data.images.iter().filter(|i| marked_images.contains(&i.id)) {
        total += unattributed(image, layers);
        for (diff, _) in identified(image, layers) {
            if index.holders[&diff].1.is_subset(&marked_images) {
                freed_layers.insert(diff);
            }
        }
    }
    total += freed_layers.iter().map(|d| index.holders[d].0).sum::<u64>();
    for id in marked {
        if let Some(f) = a.items.get(id)
            && matches!(f.kind, ItemKind::Volume | ItemKind::CacheRecord)
        {
            total += f.size;
        }
    }
    total
}

/// Marked things whose required containers are not all marked too (Docker
/// refuses to remove them otherwise): `(item, containers still needed)`.
pub fn blockers(a: &Analysis, marked: &BTreeSet<ItemId>) -> Vec<(ItemId, Vec<ItemId>)> {
    marked
        .iter()
        .filter_map(|id| {
            let missing: Vec<ItemId> = a
                .items
                .get(id)?
                .needs
                .iter()
                .filter(|n| !marked.contains(*n))
                .cloned()
                .collect();
            (!missing.is_empty()).then(|| (id.clone(), missing))
        })
        .collect()
}

/// The marked Docker items in the order they must be removed: containers,
/// then images, then volumes, then cache records (children first), then the
/// VM trim.
pub fn removal_order(a: &Analysis, marked: &BTreeSet<ItemId>) -> Vec<ItemId> {
    let rank = |id: &str| -> Option<(u8, std::cmp::Reverse<u32>)> {
        if id == TRIM_ITEM {
            return Some((4, std::cmp::Reverse(0)));
        }
        let f = a.items.get(id)?;
        Some(match f.kind {
            ItemKind::Container => (0, std::cmp::Reverse(0)),
            ItemKind::Image => (1, std::cmp::Reverse(0)),
            ItemKind::Volume => (2, std::cmp::Reverse(0)),
            ItemKind::CacheRecord => (3, std::cmp::Reverse(f.depth)),
            ItemKind::VmTrim => (4, std::cmp::Reverse(0)),
        })
    };
    let mut out: Vec<(ItemId, (u8, std::cmp::Reverse<u32>))> = marked
        .iter()
        .filter_map(|id| rank(id).map(|r| (id.clone(), r)))
        .collect();
    out.sort_by(|(ia, ra), (ib, rb)| ra.cmp(rb).then_with(|| ia.cmp(ib)));
    out.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::testdata::*;

    /// Two images sharing a 100-byte base layer.
    fn two_images() -> (DockerData, Layers) {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 160), image("B", &["b:1"], 135)],
            ..DockerData::default()
        };
        let layers: Layers = [
            ("A".to_string(), vec![layer("base", 100), layer("a", 50)]),
            ("B".to_string(), vec![layer("base", 100), layer("b", 30)]),
        ]
        .into();
        (data, layers)
    }

    fn set(ids: &[&str]) -> BTreeSet<ItemId> {
        ids.iter().map(|i| i.to_string()).collect()
    }

    #[test]
    fn frees_alone_is_unique_layers_only() {
        let (data, layers) = two_images();
        let a = analyse(&data, &layers);
        let fa = &a.items["image:A"];
        // 50 unique layer + 10 bytes of config no layer accounts for.
        assert_eq!(fa.frees_alone, 60);
        assert_eq!(a.items["image:B"].frees_alone, 35);
        assert!(!fa.approximate);
    }

    #[test]
    fn layer_shared_by_two_images_frees_only_when_both_marked() {
        let (data, layers) = two_images();
        let a = analyse(&data, &layers);
        assert_eq!(frees_of_set(&a, &data, &layers, &set(&["image:A"])), 60);
        assert_eq!(frees_of_set(&a, &data, &layers, &set(&["image:B"])), 35);
        assert_eq!(
            frees_of_set(&a, &data, &layers, &set(&["image:A", "image:B"])),
            195,
            "the shared 100 is freed once both are gone"
        );
        assert_eq!(frees_of_set(&a, &data, &layers, &set(&[])), 0);
    }

    #[test]
    fn identity_size_equals_frees_plus_kept_by_others() {
        let (data, layers) = two_images();
        let a = analyse(&data, &layers);
        for id in ["image:A", "image:B"] {
            let f = &a.items[id];
            assert_eq!(f.size, f.frees_alone + f.kept_by_others, "{id}");
        }
        assert_eq!(a.items["image:A"].kept_by_others, 100);
    }

    #[test]
    fn missing_layer_information_is_approximate_and_counts_everything_as_unique() {
        let (data, _) = two_images();
        let a = analyse(&data, &Layers::new());
        assert!(a.items["image:A"].approximate);
        assert_eq!(a.items["image:A"].frees_alone, 160);
    }

    #[test]
    fn layers_without_ids_are_unique_and_approximate() {
        let (data, mut layers) = two_images();
        layers.get_mut("A").unwrap()[1].diff_id = None;
        let a = analyse(&data, &layers);
        assert!(a.items["image:A"].approximate);
        assert_eq!(a.items["image:A"].frees_alone, 60);
    }

    #[test]
    fn dangling_image_is_bulk_ok_but_tagged_is_not() {
        let data = DockerData {
            images: vec![image("D", &[], 10), image("T", &["t:1"], 10)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert!(a.items["image:D"].dangling && a.items["image:D"].bulk_ok);
        assert!(!a.items["image:T"].dangling && !a.items["image:T"].bulk_ok);
        assert!(a.items["image:T"].unused);
    }

    #[test]
    fn image_with_running_container_is_refused() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10)],
            containers: vec![container("web", "A", ContainerState::Running, &[])],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(
            a.items["image:A"].removable,
            Err(Refusal::Running(vec!["web".to_string()]))
        );
        assert_eq!(
            a.items["container:web"].removable,
            Err(Refusal::Running(vec!["web".to_string()]))
        );
    }

    #[test]
    fn image_with_a_stopped_container_needs_it_removed_first() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10)],
            containers: vec![container("old", "A", ContainerState::Stopped, &[])],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(a.items["image:A"].removable, Ok(()));
        assert_eq!(a.items["image:A"].needs, ["container:old"]);
        assert_eq!(
            blockers(&a, &set(&["image:A"])),
            [("image:A".to_string(), vec!["container:old".to_string()])]
        );
        assert!(blockers(&a, &set(&["image:A", "container:old"])).is_empty());
    }

    #[test]
    fn volume_used_by_a_stopped_container_is_not_bulk_ok_and_needs_it() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10)],
            containers: vec![container("old", "A", ContainerState::Stopped, &["data"])],
            volumes: vec![volume("data", 500), volume("free", 7)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        let v = &a.items["volume:data"];
        assert!(!v.bulk_ok && !v.unused);
        assert_eq!(v.needs, ["container:old"]);
        assert_eq!(v.holders, ["container:old"]);
        assert!(a.items["volume:free"].unused);
        assert!(
            !a.items["volume:free"].bulk_ok,
            "volumes are never bulk-marked"
        );
    }

    #[test]
    fn volume_tree_parent_is_the_first_container_by_name() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10), image("B", &["b:1"], 10)],
            containers: vec![
                container("zeta", "A", ContainerState::Stopped, &["shared"]),
                container("alpha", "B", ContainerState::Stopped, &["shared"]),
            ],
            volumes: vec![volume("shared", 1)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        let under = |image: &str| -> Vec<&str> {
            let node = a.images.iter().find(|n| n.id == image).unwrap();
            node.children
                .iter()
                .flat_map(|c| c.children.iter().map(|v| v.id.as_str()))
                .collect()
        };
        assert_eq!(under("image:B"), ["volume:shared"], "alpha sorts first");
        assert!(under("image:A").is_empty());
        assert!(
            a.volumes.is_empty(),
            "a held volume is not listed on its own"
        );
        assert_eq!(
            a.items["volume:shared"].holders,
            ["container:alpha", "container:zeta"]
        );
    }

    #[test]
    fn unattached_volume_is_listed_on_its_own_and_orphan_container_too() {
        let data = DockerData {
            containers: vec![container("lost", "GONE", ContainerState::Stopped, &[])],
            volumes: vec![volume("free", 7)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(a.volumes[0].id, "volume:free");
        assert_eq!(a.orphan_containers[0].id, "container:lost");
    }

    #[test]
    fn image_last_used_proxy_is_newest_container_time_else_created() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10), image("B", &["b:1"], 10)],
            containers: vec![container("c", "A", ContainerState::Stopped, &[])],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(a.items["image:A"].last_used, 4000);
        assert!(a.items["image:A"].last_used_is_proxy);
        assert_eq!(a.items["image:B"].last_used, 1000, "its own creation time");
        assert!(a.items["image:B"].last_used_is_proxy);
    }

    #[test]
    fn cache_last_used_is_real_and_in_use_records_are_refused() {
        let data = DockerData {
            cache: vec![record("r1", None, 5, false), record("r2", None, 5, true)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(a.items["cache:r1"].last_used, 200);
        assert!(!a.items["cache:r1"].last_used_is_proxy);
        assert!(a.items["cache:r1"].bulk_ok);
        assert_eq!(a.items["cache:r2"].removable, Err(Refusal::CacheInUse));
        assert!(!a.items["cache:r2"].bulk_ok);
    }

    #[test]
    fn removal_order_is_containers_images_volumes_cache() {
        let data = DockerData {
            images: vec![image("A", &["a:1"], 10)],
            containers: vec![container("old", "A", ContainerState::Stopped, &["data"])],
            volumes: vec![volume("data", 5)],
            cache: vec![
                record("p", None, 1, false),
                record("c", Some("p"), 1, false),
            ],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        let all = set(&[
            "cache:p",
            "volume:data",
            "image:A",
            "container:old",
            "cache:c",
            TRIM_ITEM,
        ]);
        assert_eq!(
            removal_order(&a, &all),
            [
                "container:old",
                "image:A",
                "volume:data",
                "cache:c",
                "cache:p",
                TRIM_ITEM
            ]
        );
    }

    #[test]
    fn cache_children_hang_under_their_first_parent() {
        let data = DockerData {
            cache: vec![
                record("p", None, 1, false),
                record("c", Some("p"), 1, false),
                record("orphan", Some("missing"), 1, false),
            ],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        let roots: Vec<&str> = a.cache.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(roots, ["cache:orphan", "cache:p"]);
        let p = a.cache.iter().find(|n| n.id == "cache:p").unwrap();
        assert_eq!(p.children[0].id, "cache:c");
        assert_eq!(a.items["cache:c"].depth, 1);
        assert_eq!(a.items["cache:p"].depth, 0);
    }

    #[test]
    fn the_vm_trim_item_exists_only_when_the_vm_disk_is_known() {
        let mut data = DockerData {
            volumes: vec![volume("data", 40)],
            layers_size: 10,
            ..DockerData::default()
        };
        assert!(!analyse(&data, &Layers::new()).items.contains_key(TRIM_ITEM));
        data.vm_allocated = Some(100);
        let a = analyse(&data, &Layers::new());
        let t = &a.items[TRIM_ITEM];
        assert_eq!(t.kind, ItemKind::VmTrim);
        assert_eq!(
            (t.size, t.frees_alone),
            (100, 50),
            "100 on disk, 10 + 40 in use"
        );
        assert_eq!(t.removable, Ok(()));
        assert!(!t.bulk_ok, "never taken by a bulk mark");
        // An upper bound is never negative, and it frees nothing by itself.
        data.vm_allocated = Some(30);
        assert_eq!(
            analyse(&data, &Layers::new()).items[TRIM_ITEM].frees_alone,
            0
        );
        assert_eq!(
            frees_of_set(&a, &data, &Layers::new(), &set(&[TRIM_ITEM])),
            0
        );
    }

    #[test]
    fn frees_of_set_adds_volumes_and_cache_and_ignores_unknown_ids() {
        let data = DockerData {
            volumes: vec![volume("data", 5)],
            cache: vec![record("p", None, 7, false)],
            ..DockerData::default()
        };
        let a = analyse(&data, &Layers::new());
        assert_eq!(
            frees_of_set(
                &a,
                &data,
                &Layers::new(),
                &set(&["volume:data", "cache:p", "nope"])
            ),
            12
        );
    }
}
