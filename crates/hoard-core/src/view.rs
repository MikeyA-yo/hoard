//! Bounded views over a [`Tree`] for UIs.
//!
//! A UI never receives the whole tree. It asks for a page of children, a
//! pruned treemap slice or a breadcrumb trail, and every view here has a hard
//! size limit (CLAUDE.md rule 5).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::classify::{category_for, Category};
use crate::size::SizeMode;
use crate::tree::{NodeId, NodeKind, SortKey, Tree};

/// Hard cap on items returned by any single view.
pub const MAX_VIEW_ITEMS: usize = 5_000;

/// A page of children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildrenPage {
    /// Total number of children (before paging).
    pub total: usize,
    /// The requested slice, in sort order.
    pub items: Vec<NodeId>,
}

/// Children of `id`, sorted, sliced to `offset..offset + limit`. `limit` is
/// clamped to [`MAX_VIEW_ITEMS`].
pub fn children_page(
    tree: &Tree,
    id: NodeId,
    sort: SortKey,
    mode: SizeMode,
    offset: usize,
    limit: usize,
) -> ChildrenPage {
    let sorted = tree.sorted_children(id, sort, mode);
    let total = sorted.len();
    let limit = limit.min(MAX_VIEW_ITEMS);
    let items = sorted.into_iter().skip(offset).take(limit).collect();
    ChildrenPage { total, items }
}

/// Node ids from the root down to `id`, inclusive. Empty if `id` is unknown.
pub fn ancestors(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        let Some(node) = tree.get(c) else {
            return Vec::new();
        };
        out.push(c);
        cur = node.parent;
    }
    out.reverse();
    out
}

/// Options for [`treemap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreemapOptions {
    /// Levels below the requested node to include.
    pub depth: usize,
    /// Children smaller than this are merged into one "smaller items" node.
    pub min_bytes: u64,
    /// Maximum nodes in the result, including merged nodes. Clamped to
    /// [`MAX_VIEW_ITEMS`].
    pub max_nodes: usize,
    /// Size mode used for sorting and sizes.
    pub mode: SizeMode,
}

impl Default for TreemapOptions {
    fn default() -> Self {
        TreemapOptions {
            depth: 3,
            min_bytes: 0,
            max_nodes: 4_000,
            mode: SizeMode::Allocated,
        }
    }
}

/// One node of a pruned treemap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreemapNode {
    /// Tree node id; `None` for a merged "smaller items" node.
    pub id: Option<NodeId>,
    /// Display name (lossy UTF-8). For merged nodes, e.g. "312 smaller items".
    pub name: String,
    /// Node kind; `None` for merged nodes.
    pub kind: Option<NodeKind>,
    /// Category, for coloring.
    pub category: Category,
    /// Size in the requested mode. For directories, the full aggregate, which
    /// may exceed the sum of the included children.
    pub size: u64,
    /// Files under this node.
    pub file_count: u32,
    /// Modification time, unix seconds (0 for merged nodes).
    pub mtime: i64,
    /// For merged nodes, how many entries were merged.
    pub merged_count: u32,
    /// True if this directory has children that were not expanded because of
    /// the depth limit (the UI can drill in).
    pub has_more: bool,
    /// Included children, largest first.
    pub children: Vec<TreemapNode>,
}

/// Builds a pruned subtree rooted at `id` for treemap layout.
///
/// Expansion is breadth-first, so when the node budget runs out the shallow
/// levels are complete and only deep detail is lost. At each directory,
/// children below `min_bytes` (and any that do not fit the budget) are merged
/// into a single node.
pub fn treemap(tree: &Tree, id: NodeId, opts: TreemapOptions) -> Option<TreemapNode> {
    let max_nodes = opts.max_nodes.clamp(1, MAX_VIEW_ITEMS);
    let mode = opts.mode;
    let make = |nid: NodeId| -> Option<TreemapNode> {
        let n = tree.get(nid)?;
        Some(TreemapNode {
            id: Some(nid),
            name: n.display_name(),
            kind: Some(n.kind),
            category: category_for(n.kind, &n.name),
            size: n.size(mode),
            file_count: n.file_count,
            mtime: n.mtime,
            merged_count: 0,
            has_more: !n.children.is_empty(),
            children: Vec::new(),
        })
    };

    // Flat arena of output nodes; children referenced by index.
    let mut flat: Vec<(TreemapNode, Vec<usize>)> = vec![(make(id)?, Vec::new())];
    let mut queue: VecDeque<(usize, NodeId, usize)> = VecDeque::from([(0, id, 0)]);
    let mut budget = max_nodes - 1;

    while let Some((fi, nid, depth)) = queue.pop_front() {
        if depth >= opts.depth || budget == 0 {
            continue;
        }
        let kids = tree.sorted_children(nid, SortKey::Size, mode);
        flat[fi].0.has_more = false;
        let mut merged_size: u64 = 0;
        let mut merged_count: u32 = 0;
        let mut merged_files: u32 = 0;
        for kid in kids {
            let Some(n) = tree.get(kid) else { continue };
            let size = n.size(mode);
            // Keep one slot free for the merged node.
            if size > 0 && size >= opts.min_bytes && budget > 1 {
                let Some(node) = make(kid) else { continue };
                flat.push((node, Vec::new()));
                let idx = flat.len() - 1;
                flat[fi].1.push(idx);
                budget -= 1;
                if n.is_dir() && !n.children.is_empty() {
                    queue.push_back((idx, kid, depth + 1));
                }
            } else {
                merged_size = merged_size.saturating_add(size);
                merged_count = merged_count.saturating_add(1);
                merged_files = merged_files.saturating_add(n.file_count);
            }
        }
        if merged_count > 0 && budget > 0 {
            flat.push((
                TreemapNode {
                    id: None,
                    name: format!(
                        "{} smaller item{}",
                        merged_count,
                        if merged_count == 1 { "" } else { "s" }
                    ),
                    kind: None,
                    category: Category::Other,
                    size: merged_size,
                    file_count: merged_files,
                    mtime: 0,
                    merged_count,
                    has_more: false,
                    children: Vec::new(),
                },
                Vec::new(),
            ));
            let idx = flat.len() - 1;
            flat[fi].1.push(idx);
            budget -= 1;
        }
    }

    Some(assemble(&mut flat, 0))
}

fn assemble(flat: &mut [(TreemapNode, Vec<usize>)], idx: usize) -> TreemapNode {
    let kids = std::mem::take(&mut flat[idx].1);
    let children: Vec<TreemapNode> = kids.into_iter().map(|k| assemble(flat, k)).collect();
    let mut node = std::mem::replace(
        &mut flat[idx].0,
        TreemapNode {
            id: None,
            name: String::new(),
            kind: None,
            category: Category::Other,
            size: 0,
            file_count: 0,
            mtime: 0,
            merged_count: 0,
            has_more: false,
            children: Vec::new(),
        },
    );
    node.children = children;
    node
}

/// Counts nodes in a treemap result (for tests and limits).
pub fn treemap_len(node: &TreemapNode) -> usize {
    1 + node.children.iter().map(treemap_len).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::platform::EntryMeta;
    use crate::tree::{NodeFlags, TreeBuilder};

    fn file(size: u64) -> EntryMeta {
        EntryMeta {
            kind: NodeKind::File,
            size_allocated: size,
            size_apparent: size,
            nlink: 1,
            ..EntryMeta::default()
        }
    }

    fn dir() -> EntryMeta {
        EntryMeta {
            kind: NodeKind::Dir,
            nlink: 1,
            ..EntryMeta::default()
        }
    }

    /// root/{big: [b0..b9 of 1000..], small_0..small_99 (1 byte each), d/sub/deep}
    fn sample() -> Tree {
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &dir(), SizeMode::Allocated);
        let r = b.root();
        let big = b.push(r, "big".into(), &dir(), NodeFlags::NONE).unwrap();
        for i in 0..10u64 {
            b.push(
                big,
                format!("b{i}.mp4").into(),
                &file(1000 + i),
                NodeFlags::NONE,
            )
            .unwrap();
        }
        for i in 0..100 {
            b.push(r, format!("small_{i}").into(), &file(1), NodeFlags::NONE)
                .unwrap();
        }
        let d = b.push(r, "d".into(), &dir(), NodeFlags::NONE).unwrap();
        let sub = b.push(d, "sub".into(), &dir(), NodeFlags::NONE).unwrap();
        b.push(sub, "deep.bin".into(), &file(500), NodeFlags::NONE)
            .unwrap();
        b.finish(Vec::new(), true)
    }

    #[test]
    fn pages_children() {
        let t = sample();
        let p = children_page(&t, t.root(), SortKey::Size, SizeMode::Allocated, 0, 2);
        assert_eq!(p.total, 102);
        assert_eq!(p.items.len(), 2);
        assert_eq!(t.get(p.items[0]).unwrap().display_name(), "big");
        let p2 = children_page(&t, t.root(), SortKey::Size, SizeMode::Allocated, 101, 50);
        assert_eq!(p2.items.len(), 1);
        let p3 = children_page(
            &t,
            t.root(),
            SortKey::Size,
            SizeMode::Allocated,
            0,
            usize::MAX,
        );
        assert_eq!(p3.items.len(), 102);
    }

    #[test]
    fn ancestors_root_to_node() {
        let t = sample();
        let deep = *t
            .children(
                *t.children(*t.children(t.root()).last().unwrap())
                    .first()
                    .unwrap(),
            )
            .first()
            .unwrap();
        let names: Vec<String> = ancestors(&t, deep)
            .iter()
            .map(|i| t.get(*i).unwrap().display_name())
            .collect();
        assert_eq!(names, vec!["/r", "d", "sub", "deep.bin"]);
        assert!(ancestors(&t, NodeId(9999)).is_empty());
    }

    #[test]
    fn treemap_merges_small_items() {
        let t = sample();
        let opts = TreemapOptions {
            depth: 1,
            min_bytes: 100,
            ..TreemapOptions::default()
        };
        let tm = treemap(&t, t.root(), opts).unwrap();
        let names: Vec<&str> = tm.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["big", "d", "100 smaller items"]);
        let merged = tm.children.last().unwrap();
        assert_eq!(merged.id, None);
        assert_eq!(merged.size, 100);
        assert_eq!(merged.merged_count, 100);
        // Depth limit: "big" was not expanded but can be drilled into.
        assert!(tm.children[0].children.is_empty());
        assert!(tm.children[0].has_more);
        assert_eq!(tm.children[0].category, Category::Folder);
    }

    #[test]
    fn treemap_respects_depth() {
        let t = sample();
        let opts = TreemapOptions {
            depth: 3,
            min_bytes: 100,
            ..TreemapOptions::default()
        };
        let tm = treemap(&t, t.root(), opts).unwrap();
        let d = tm.children.iter().find(|c| c.name == "d").unwrap();
        let sub = &d.children[0];
        assert_eq!(sub.children[0].name, "deep.bin");
        assert!(!sub.has_more);
        let big = &tm.children[0];
        assert_eq!(big.children.len(), 10);
        assert_eq!(big.children[0].name, "b9.mp4");
        assert_eq!(big.children[0].category, Category::Video);
    }

    #[test]
    fn treemap_never_exceeds_budget() {
        let t = sample();
        for max_nodes in [1, 2, 3, 5, 8, 50] {
            let opts = TreemapOptions {
                depth: 5,
                min_bytes: 0,
                max_nodes,
                mode: SizeMode::Allocated,
            };
            let tm = treemap(&t, t.root(), opts).unwrap();
            assert!(
                treemap_len(&tm) <= max_nodes,
                "max_nodes={max_nodes} got {}",
                treemap_len(&tm)
            );
        }
    }

    #[test]
    fn treemap_budget_fills_shallow_levels_first() {
        let t = sample();
        let opts = TreemapOptions {
            depth: 5,
            min_bytes: 0,
            max_nodes: 6,
            mode: SizeMode::Allocated,
        };
        let tm = treemap(&t, t.root(), opts).unwrap();
        // Root + 4 top-level children + 1 merged node; nothing deeper.
        assert_eq!(tm.children.len(), 5);
        assert!(tm.children.last().unwrap().id.is_none());
        assert!(tm.children.iter().all(|c| c.children.is_empty()));
    }

    #[test]
    fn treemap_of_unknown_id_is_none() {
        let t = sample();
        assert!(treemap(&t, NodeId(12345), TreemapOptions::default()).is_none());
    }
}
