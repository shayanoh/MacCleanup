//! The Docker page as a flat list of lines: three coloured sections, the
//! tree of what holds what under IMAGES, layers under an image, and the
//! build cache as a flat list. Folding, sorting and the fzf-style filter
//! live here; drawing is `ui::docker`.

use std::collections::BTreeSet;

use crate::docker::analysis::{Analysis, ItemFacts, ItemKind, Layers, TreeNode};
use crate::docker::model::*;
use crate::filter::Query;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DSection {
    /// The VM disk file (one row: shrink it).
    VmDisk,
    Images,
    Volumes,
    BuildCache,
}

impl DSection {
    pub fn key(self) -> &'static str {
        match self {
            DSection::VmDisk => "section:vm",
            DSection::Images => "section:images",
            DSection::Volumes => "section:volumes",
            DSection::BuildCache => "section:cache",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DKind {
    Section(DSection),
    Image,
    /// Layer number `n` (0 = oldest) of the image above it.
    Layer(usize),
    Container,
    Volume,
    Cache,
    /// The "shrink the VM disk" row.
    Trim,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DLine {
    /// 0 for a section band, 1 for its items, deeper for what hangs under.
    pub depth: u8,
    /// The `ItemId` of an item, `layer:<image id>:<n>` of a layer, or the
    /// section key.
    pub key: String,
    pub kind: DKind,
    /// `Some(open)` when the line can be folded.
    pub open: Option<bool>,
}

/// The sort keys, in the order of the column headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DSort {
    Name,
    Size,
    Frees,
    LastUsed,
    Created,
}

impl DSort {
    /// The next one in header order (what `s` does).
    pub fn next(self) -> DSort {
        match self {
            DSort::Name => DSort::Size,
            DSort::Size => DSort::Frees,
            DSort::Frees => DSort::LastUsed,
            DSort::LastUsed => DSort::Created,
            DSort::Created => DSort::Name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DockerView {
    /// Keys of folded-open lines.
    pub open: BTreeSet<String>,
    pub sort: DSort,
}

pub fn layer_key(image_id: &str, n: usize) -> String {
    format!("layer:{image_id}:{n}")
}

/// What a line is filtered, sorted and drawn from.
struct Ctx<'a> {
    a: &'a Analysis,
    data: &'a DockerData,
    layers: &'a Layers,
    filter: &'a Query,
    filtering: bool,
}

fn kind_of(k: ItemKind) -> DKind {
    match k {
        ItemKind::Image => DKind::Image,
        ItemKind::Container => DKind::Container,
        ItemKind::Volume => DKind::Volume,
        ItemKind::CacheRecord => DKind::Cache,
        ItemKind::VmTrim => DKind::Trim,
    }
}

impl Ctx<'_> {
    /// The words a filter term can match for an item (its own, never its
    /// parents': those come from the caller).
    fn own_fields(&self, id: &str, facts: &ItemFacts) -> Vec<String> {
        let mut fields = vec![facts.name.clone()];
        if let Some(raw) = id.strip_prefix("image:")
            && let Some(img) = self.data.images.iter().find(|i| i.id == raw)
        {
            fields.extend(img.tags.iter().cloned());
            fields.push(
                raw.strip_prefix("sha256:")
                    .unwrap_or(raw)
                    .chars()
                    .take(12)
                    .collect(),
            );
        }
        fields
    }

    fn matches(&self, fields: &[String]) -> bool {
        let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
        !self.filtering || self.filter.matches_fields(&refs)
    }
}

impl DockerView {
    /// Images with containers start open, and so do those containers, so the
    /// connections are visible at once.
    pub fn new(a: &Analysis) -> DockerView {
        let mut open = BTreeSet::new();
        for n in a.images.iter().chain(&a.orphan_containers) {
            if !n.children.is_empty() {
                open.insert(n.id.clone());
            }
            for c in &n.children {
                if !c.children.is_empty() {
                    open.insert(c.id.clone());
                }
            }
        }
        DockerView {
            open,
            sort: DSort::Size,
        }
    }

    pub fn set_open(&mut self, key: &str, open: bool) {
        if open {
            self.open.insert(key.to_string());
        } else {
            self.open.remove(key);
        }
    }

    fn order(&self, a: &ItemFacts, b: &ItemFacts) -> std::cmp::Ordering {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        match self.sort {
            DSort::Name => by_name(),
            DSort::Size => b.size.cmp(&a.size),
            DSort::Frees => b.frees_alone.cmp(&a.frees_alone),
            DSort::LastUsed => b.last_used.cmp(&a.last_used),
            DSort::Created => b.created.cmp(&a.created),
        }
        .then_with(by_name)
    }

    fn sorted<'n>(
        &self,
        ctx: &Ctx,
        nodes: impl Iterator<Item = &'n TreeNode>,
    ) -> Vec<&'n TreeNode> {
        let mut v: Vec<&TreeNode> = nodes.collect();
        v.sort_by(|x, y| self.order(&ctx.a.items[&x.id], &ctx.a.items[&y.id]));
        v
    }

    /// Lines of one tree node and what hangs under it; false when nothing
    /// of it matches the filter.
    fn node_lines(
        &self,
        ctx: &Ctx,
        node: &TreeNode,
        depth: u8,
        ancestors: &[String],
        out: &mut Vec<DLine>,
    ) -> bool {
        let facts = &ctx.a.items[&node.id];
        let mut fields = ancestors.to_vec();
        fields.extend(ctx.own_fields(&node.id, facts));
        let own = ctx.matches(&fields);
        let mut below = Vec::new();
        let mut any_below = false;
        for child in self.sorted(ctx, node.children.iter()) {
            any_below |= self.node_lines(ctx, child, depth + 1, &fields, &mut below);
        }
        let mut has_children = !node.children.is_empty();
        if facts.kind == ItemKind::Image {
            let raw = node.id.strip_prefix("image:").unwrap_or(&node.id);
            for (n, l) in ctx.layers.get(raw).into_iter().flatten().enumerate() {
                has_children = true;
                let mut lf = fields.clone();
                lf.push(l.command.clone());
                if ctx.matches(&lf) {
                    any_below = true;
                    below.push(DLine {
                        depth: depth + 1,
                        key: layer_key(raw, n),
                        kind: DKind::Layer(n),
                        open: None,
                    });
                }
            }
        }
        if !own && !any_below {
            return false;
        }
        let expanded = ctx.filtering || self.open.contains(&node.id);
        out.push(DLine {
            depth,
            key: node.id.clone(),
            kind: kind_of(facts.kind),
            open: has_children.then_some(expanded),
        });
        if expanded {
            out.extend(below);
        }
        true
    }

    /// The visible lines. Filtering opens everything that matches.
    pub fn lines(
        &self,
        a: &Analysis,
        data: &DockerData,
        layers: &Layers,
        filter: &Query,
    ) -> Vec<DLine> {
        let ctx = Ctx {
            a,
            data,
            layers,
            filter,
            filtering: !filter.is_empty(),
        };
        let mut out = Vec::new();
        for section in [
            DSection::VmDisk,
            DSection::Images,
            DSection::Volumes,
            DSection::BuildCache,
        ] {
            let mut body = Vec::new();
            match section {
                DSection::VmDisk => {
                    if a.items.contains_key(crate::docker::analysis::TRIM_ITEM) {
                        let node = TreeNode {
                            id: crate::docker::analysis::TRIM_ITEM.to_string(),
                            children: Vec::new(),
                        };
                        self.node_lines(&ctx, &node, 1, &[], &mut body);
                    }
                }
                DSection::Images => {
                    for n in self.sorted(&ctx, a.images.iter().chain(&a.orphan_containers)) {
                        self.node_lines(&ctx, n, 1, &[], &mut body);
                    }
                }
                DSection::Volumes => {
                    for n in self.sorted(&ctx, a.volumes.iter()) {
                        self.node_lines(&ctx, n, 1, &[], &mut body);
                    }
                }
                DSection::BuildCache => {
                    // A flat list: each record shows its step; what it was
                    // built on is in the details.
                    let flat: Vec<TreeNode> = data
                        .cache
                        .iter()
                        .map(|r| TreeNode {
                            id: crate::docker::analysis::cache_item(&r.id),
                            children: Vec::new(),
                        })
                        .collect();
                    for n in self.sorted(&ctx, flat.iter()) {
                        self.node_lines(&ctx, n, 1, &[], &mut body);
                    }
                }
            }
            if !body.is_empty() {
                out.push(DLine {
                    depth: 0,
                    key: section.key().to_string(),
                    kind: DKind::Section(section),
                    open: None,
                });
                out.extend(body);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::analysis::analyse;
    use crate::docker::testdata::*;
    use crate::filter;

    struct Fixture {
        data: DockerData,
        layers: Layers,
        a: Analysis,
    }

    fn fixture() -> Fixture {
        let data = DockerData {
            images: vec![
                image("A", &["web:latest"], 1000),
                image("B", &["api:dev"], 3000),
                image("D", &[], 10),
            ],
            containers: vec![container(
                "web-1",
                "A",
                ContainerState::Stopped,
                &["pgdata"],
            )],
            volumes: vec![volume("pgdata", 66), volume("free", 500)],
            cache: vec![record("r1", None, 5, false), {
                let mut r = record("r2", None, 9, false);
                r.last_used = 100;
                r.created = 50;
                r
            }],
            ..DockerData::default()
        };
        let mut r1 = data.cache[0].clone();
        r1.last_used = 300;
        let mut data = data;
        data.cache[0] = r1;
        let layers: Layers = [("A".to_string(), vec![layer("base", 100), layer("a", 50)])].into();
        let a = analyse(&data, &layers);
        Fixture { data, layers, a }
    }

    fn keys(lines: &[DLine]) -> Vec<&str> {
        lines.iter().map(|l| l.key.as_str()).collect()
    }

    fn view_lines(f: &Fixture, v: &DockerView, q: &str) -> Vec<DLine> {
        v.lines(&f.a, &f.data, &f.layers, &filter::parse(q))
    }

    fn closed(f: &Fixture) -> DockerView {
        let mut v = DockerView::new(&f.a);
        v.open.clear();
        v
    }

    #[test]
    fn sections_come_in_a_fixed_order() {
        let f = fixture();
        let lines = view_lines(&f, &closed(&f), "");
        let sections: Vec<&str> = lines
            .iter()
            .filter(|l| matches!(l.kind, DKind::Section(_)))
            .map(|l| l.key.as_str())
            .collect();
        assert_eq!(
            sections,
            ["section:images", "section:volumes", "section:cache"]
        );
        assert!(lines.iter().filter(|l| l.depth == 0).count() == 3);
    }

    #[test]
    fn the_vm_disk_row_is_a_section_of_its_own_at_the_top_when_known() {
        let mut f = fixture();
        assert!(!keys(&view_lines(&f, &closed(&f), "")).contains(&"vm:trim"));
        f.data.vm_allocated = Some(5_000);
        f.a = analyse(&f.data, &f.layers);
        let lines = view_lines(&f, &closed(&f), "");
        assert_eq!(lines[0].key, "section:vm");
        assert_eq!(lines[1].key, "vm:trim");
        assert_eq!((lines[1].depth, lines[1].kind), (1, DKind::Trim));
        // The filter reaches it by its name.
        let filtered = view_lines(&f, &closed(&f), "shrink");
        assert_eq!(keys(&filtered), ["section:vm", "vm:trim"]);
    }

    #[test]
    fn empty_sections_are_left_out() {
        let mut f = fixture();
        f.data.cache.clear();
        f.a = analyse(&f.data, &f.layers);
        let lines = view_lines(&f, &closed(&f), "");
        assert!(!keys(&lines).contains(&"section:cache"));
    }

    #[test]
    fn children_are_hidden_until_opened_and_containers_come_before_layers() {
        let f = fixture();
        let mut v = closed(&f);
        let lines = view_lines(&f, &v, "");
        assert!(!keys(&lines).contains(&"container:web-1"));
        assert_eq!(
            lines.iter().find(|l| l.key == "image:A").unwrap().open,
            Some(false)
        );
        assert_eq!(
            lines.iter().find(|l| l.key == "image:D").unwrap().open,
            None
        );

        v.set_open("image:A", true);
        let lines = view_lines(&f, &v, "");
        let under: Vec<&str> = keys(&lines)
            .into_iter()
            .skip_while(|k| *k != "image:A")
            .skip(1)
            .take(3)
            .collect();
        assert_eq!(under, ["container:web-1", "layer:A:0", "layer:A:1"]);
        assert!(
            !keys(&lines).contains(&"volume:pgdata"),
            "the container is still folded"
        );
        v.set_open("container:web-1", true);
        assert!(keys(&view_lines(&f, &v, "")).contains(&"volume:pgdata"));
    }

    #[test]
    fn new_view_opens_images_with_containers_and_their_containers() {
        let f = fixture();
        let v = DockerView::new(&f.a);
        assert!(v.open.contains("image:A") && v.open.contains("container:web-1"));
        assert!(!v.open.contains("image:B"));
        let lines = view_lines(&f, &v, "");
        let k = keys(&lines);
        assert!(k.contains(&"volume:pgdata"));
        let depth = |key: &str| lines.iter().find(|l| l.key == key).unwrap().depth;
        assert_eq!(
            [
                depth("image:A"),
                depth("container:web-1"),
                depth("volume:pgdata")
            ],
            [1, 2, 3]
        );
        assert_eq!(depth(&layer_key("A", 0)), 2);
    }

    #[test]
    fn layers_are_listed_oldest_first() {
        let f = fixture();
        let mut v = closed(&f);
        v.set_open("image:A", true);
        let lines = view_lines(&f, &v, "");
        let layers: Vec<DKind> = lines
            .iter()
            .filter(|l| matches!(l.kind, DKind::Layer(_)))
            .map(|l| l.kind)
            .collect();
        assert_eq!(layers, [DKind::Layer(0), DKind::Layer(1)]);
    }

    #[test]
    fn sort_cycle_follows_the_column_headers() {
        let mut s = DSort::Name;
        let mut seen = Vec::new();
        for _ in 0..5 {
            s = s.next();
            seen.push(s);
        }
        assert_eq!(
            seen,
            [
                DSort::Size,
                DSort::Frees,
                DSort::LastUsed,
                DSort::Created,
                DSort::Name
            ]
        );
    }

    #[test]
    fn sorting_orders_siblings_at_every_level() {
        let f = fixture();
        let mut v = closed(&f);
        v.sort = DSort::Size;
        let lines = view_lines(&f, &v, "");
        let images: Vec<&str> = lines
            .iter()
            .filter(|l| l.kind == DKind::Image)
            .map(|l| l.key.as_str())
            .collect();
        assert_eq!(images, ["image:B", "image:A", "image:D"], "biggest first");
        v.sort = DSort::Name;
        let lines = view_lines(&f, &v, "");
        let images: Vec<&str> = lines
            .iter()
            .filter(|l| l.kind == DKind::Image)
            .map(|l| l.key.as_str())
            .collect();
        assert_eq!(
            images,
            ["image:B", "image:D", "image:A"],
            "api:dev, then the dangling image by its id, then web:latest"
        );
    }

    #[test]
    fn build_cache_is_a_flat_list_sorted_by_last_used() {
        let f = fixture();
        let mut v = closed(&f);
        v.sort = DSort::LastUsed;
        let lines = view_lines(&f, &v, "");
        let cache: Vec<&str> = lines
            .iter()
            .filter(|l| l.kind == DKind::Cache)
            .map(|l| l.key.as_str())
            .collect();
        assert_eq!(cache, ["cache:r1", "cache:r2"], "300 before 100");
        assert!(
            lines
                .iter()
                .filter(|l| l.kind == DKind::Cache)
                .all(|l| l.depth == 1)
        );
    }

    #[test]
    fn filter_on_a_child_keeps_its_parents_only() {
        let f = fixture();
        let lines = view_lines(&f, &closed(&f), "pgdata");
        let k = keys(&lines);
        assert!(
            k.contains(&"image:A")
                && k.contains(&"container:web-1")
                && k.contains(&"volume:pgdata")
        );
        assert!(!k.contains(&"image:B") && !k.contains(&"volume:free"));
        assert!(!k.contains(&"layer:A:0"));
    }

    #[test]
    fn filter_on_a_parent_shows_all_its_children() {
        let f = fixture();
        let lines = view_lines(&f, &closed(&f), "web:latest");
        let k = keys(&lines);
        for key in [
            "image:A",
            "container:web-1",
            "volume:pgdata",
            "layer:A:0",
            "layer:A:1",
        ] {
            assert!(k.contains(&key), "{key} in {k:?}");
        }
        assert!(!k.contains(&"image:B"));
    }

    #[test]
    fn filter_matches_a_layer_command() {
        let f = fixture();
        let lines = view_lines(&f, &closed(&f), "'RUN base");
        let k = keys(&lines);
        assert!(k.contains(&"layer:A:0") && k.contains(&"image:A"));
        assert!(!k.contains(&"layer:A:1") && !k.contains(&"container:web-1"));
    }

    #[test]
    fn exclusion_hides_what_it_names() {
        let f = fixture();
        let lines = view_lines(&f, &closed(&f), "!api");
        let k = keys(&lines);
        assert!(!k.contains(&"image:B"), "{k:?}");
        assert!(k.contains(&"image:A") && k.contains(&"image:D"), "{k:?}");
    }
}
