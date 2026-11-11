//! Arena-allocated scan tree, bottom-up aggregation and queries.
//!
//! Nodes live in one `Vec` and refer to each other by [`NodeId`]. Nodes are
//! appended in depth-first pre-order (every parent before its children), so
//! aggregation is a single reverse pass. Full paths are never stored; they are
//! rebuilt by walking parents.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::EntryMeta;
use crate::scan::ScanError;
use crate::size::SizeMode;

/// Index of a node inside a [`Tree`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The index into the node arena.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a node is on disk.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// A regular file.
    #[default]
    File,
    /// A directory.
    Dir,
    /// A symlink or Windows junction. Never followed.
    Symlink,
    /// Device, socket, FIFO or anything else.
    Other,
}

/// Bit flags describing a node's state.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct NodeFlags(u8);

impl NodeFlags {
    /// No flags set.
    pub const NONE: NodeFlags = NodeFlags(0);
    /// Reading this entry or its contents failed; its size is partly unknown.
    pub const ERROR: NodeFlags = NodeFlags(1);
    /// A hardlink whose inode was already counted elsewhere; contributes 0 bytes.
    pub const HARDLINK_DUP: NodeFlags = NodeFlags(1 << 1);
    /// Matches the protected list. Set by the cleanup engine (M2).
    pub const PROTECTED: NodeFlags = NodeFlags(1 << 2);
    /// Directory on another filesystem; not descended into.
    pub const OTHER_FS: NodeFlags = NodeFlags(1 << 3);
    /// Directory excluded by [`crate::scan::ScanOptions::skip`]; not descended into.
    pub const SKIPPED: NodeFlags = NodeFlags(1 << 4);
    /// Deleted by a cleanup run. Detached from its parent; kept in the arena
    /// so ids stay stable, but skipped by every query.
    pub const REMOVED: NodeFlags = NodeFlags(1 << 5);
    /// Contents changed on disk after the scan (e.g. a partially failed
    /// permanent delete). Sizes are no longer exact; rescan to refresh.
    pub const STALE: NodeFlags = NodeFlags(1 << 6);

    const NAMES: [(NodeFlags, &'static str); 7] = [
        (Self::ERROR, "error"),
        (Self::HARDLINK_DUP, "hardlink_dup"),
        (Self::PROTECTED, "protected"),
        (Self::OTHER_FS, "other_fs"),
        (Self::SKIPPED, "skipped"),
        (Self::REMOVED, "removed"),
        (Self::STALE, "stale"),
    ];

    /// True if every bit in `other` is set.
    pub fn contains(self, other: NodeFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// True if any bit in `other` is set.
    pub fn intersects(self, other: NodeFlags) -> bool {
        self.0 & other.0 != 0
    }

    /// Sets the bits in `other`.
    pub fn insert(&mut self, other: NodeFlags) {
        self.0 |= other.0;
    }

    /// True if no bits are set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Raw bits.
    pub fn bits(self) -> u8 {
        self.0
    }

    /// Lower-case names of the set flags, for display and JSON.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(f, _)| self.contains(*f))
            .map(|(_, n)| *n)
            .collect()
    }
}

impl std::ops::BitOr for NodeFlags {
    type Output = NodeFlags;
    fn bitor(self, rhs: NodeFlags) -> NodeFlags {
        NodeFlags(self.0 | rhs.0)
    }
}

impl fmt::Debug for NodeFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeFlags({})", self.names().join("|"))
    }
}

/// One file, directory, symlink or other entry.
///
/// For directories the size and count fields are aggregates over the whole
/// subtree (including the directory's own allocated blocks).
#[derive(Clone, Debug)]
pub struct Node {
    /// File name. For the root node, the full root path. Stored as `OsStr`
    /// rather than `str` so that non-UTF-8 names round-trip exactly; cleanup
    /// must be able to rebuild the real path.
    pub name: Box<OsStr>,
    /// Parent directory, `None` for the root.
    pub parent: Option<NodeId>,
    /// Children, empty for non-directories.
    pub children: Vec<NodeId>,
    /// Entry kind.
    pub kind: NodeKind,
    /// On-disk bytes (aggregated for directories).
    pub size_allocated: u64,
    /// Logical bytes (aggregated for directories).
    pub size_apparent: u64,
    /// Number of non-directory entries (1 for a file, aggregated for directories).
    pub file_count: u32,
    /// Last modification time, unix seconds.
    pub mtime: i64,
    /// Device id at scan time.
    pub dev: u64,
    /// Inode / file index at scan time.
    pub ino: u64,
    /// State flags.
    pub flags: NodeFlags,
}

impl Node {
    /// Size in the given mode.
    pub fn size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Allocated => self.size_allocated,
            SizeMode::Apparent => self.size_apparent,
        }
    }

    /// True if this is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }

    /// The name as a display string (lossy for non-UTF-8 names).
    pub fn display_name(&self) -> String {
        self.name.to_string_lossy().into_owned()
    }
}

/// Sort order for children listings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// Largest first.
    #[default]
    Size,
    /// Most files first.
    Count,
    /// Most recently modified first.
    Modified,
    /// Alphabetical, case-insensitive.
    Name,
}

/// What [`Tree::largest`] should return.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LargestKind {
    /// Non-directory entries.
    Files,
    /// Directories, excluding the root.
    Dirs,
}

/// The result of a scan.
#[derive(Debug)]
pub struct Tree {
    nodes: Vec<Node>,
    root: NodeId,
    root_path: PathBuf,
    errors: Vec<ScanError>,
    complete: bool,
    size_mode: SizeMode,
}

impl Tree {
    /// The root node id.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The absolute path that was scanned.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True if the tree has no nodes (never the case for a finished scan).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Looks up a node. `None` if the id does not belong to this tree.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// Children of a node (empty for files or unknown ids).
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.get(id).map_or(&[], |n| n.children.as_slice())
    }

    /// Errors recorded during the scan.
    pub fn errors(&self) -> &[ScanError] {
        &self.errors
    }

    /// False if the scan was cancelled and the tree is partial.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// The size mode requested for this scan (both sizes are always recorded).
    pub fn size_mode(&self) -> SizeMode {
        self.size_mode
    }

    /// Total size of the root in the given mode.
    pub fn total(&self, mode: SizeMode) -> u64 {
        self.get(self.root).map_or(0, |n| n.size(mode))
    }

    /// Number of directories, including the root.
    pub fn dir_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.is_dir() && !n.flags.contains(NodeFlags::REMOVED))
            .count()
    }

    /// Rebuilds the full path of a node by walking its parents.
    pub fn path(&self, id: NodeId) -> Option<PathBuf> {
        let mut parts: Vec<&OsStr> = Vec::new();
        let mut cur = self.get(id)?;
        while let Some(parent) = cur.parent {
            parts.push(&cur.name);
            cur = self.get(parent)?;
        }
        let mut path = self.root_path.clone();
        for part in parts.iter().rev() {
            path.push(part);
        }
        Some(path)
    }

    /// Children of `id` sorted by `key` (sizes compared in `mode`).
    pub fn sorted_children(&self, id: NodeId, key: SortKey, mode: SizeMode) -> Vec<NodeId> {
        let mut ids = self.children(id).to_vec();
        let nodes = &self.nodes;
        let n = |i: &NodeId| &nodes[i.index()];
        match key {
            SortKey::Size => ids.sort_by_key(|i| Reverse(n(i).size(mode))),
            SortKey::Count => ids.sort_by_key(|i| Reverse(n(i).file_count)),
            SortKey::Modified => ids.sort_by_key(|i| Reverse(n(i).mtime)),
            SortKey::Name => ids.sort_by_cached_key(|i| n(i).name.to_string_lossy().to_lowercase()),
        }
        ids
    }

    /// The `limit` largest files or directories in the whole tree, largest first.
    pub fn largest(&self, kind: LargestKind, limit: usize, mode: SizeMode) -> Vec<NodeId> {
        if limit == 0 {
            return Vec::new();
        }
        let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(limit + 1);
        for (i, node) in self.nodes.iter().enumerate() {
            if node.flags.contains(NodeFlags::REMOVED) {
                continue;
            }
            let wanted = match kind {
                LargestKind::Files => !node.is_dir(),
                LargestKind::Dirs => node.is_dir() && node.parent.is_some(),
            };
            if !wanted {
                continue;
            }
            // Arena length is bounded by u32 when nodes are pushed.
            let id = NodeId(i as u32);
            heap.push(Reverse((node.size(mode), id)));
            if heap.len() > limit {
                heap.pop();
            }
        }
        let mut out: Vec<(u64, NodeId)> = heap.into_iter().map(|Reverse(x)| x).collect();
        out.sort_by(|a, b| b.cmp(a));
        out.into_iter().map(|(_, id)| id).collect()
    }
}

impl Tree {
    /// True if any bit of `flags` is set on `id` or any node below it.
    pub fn subtree_has_flag(&self, id: NodeId, flags: NodeFlags) -> bool {
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            let Some(n) = self.get(cur) else { continue };
            if n.flags.intersects(flags) {
                return true;
            }
            stack.extend_from_slice(&n.children);
        }
        false
    }

    /// True if `ancestor` is `id` or one of its ancestors.
    pub fn is_ancestor_or_self(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.get(c).and_then(|n| n.parent);
        }
        false
    }

    /// Adds flags to a node. Returns false for an unknown id.
    pub fn set_flag(&mut self, id: NodeId, flag: NodeFlags) -> bool {
        match self.nodes.get_mut(id.index()) {
            Some(n) => {
                n.flags.insert(flag);
                true
            }
            None => false,
        }
    }

    /// Removes a node and everything below it after a successful deletion:
    /// detaches it from its parent, subtracts its sizes and file count from
    /// every ancestor, and marks the whole subtree [`NodeFlags::REMOVED`].
    ///
    /// Returns the removed `(allocated, apparent, files)`, or `None` for the
    /// root, an unknown id or an already removed node.
    pub fn remove_subtree(&mut self, id: NodeId) -> Option<(u64, u64, u32)> {
        let node = self.get(id)?;
        let parent = node.parent?;
        if node.flags.contains(NodeFlags::REMOVED) {
            return None;
        }
        let removed = (node.size_allocated, node.size_apparent, node.file_count);

        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            if let Some(n) = self.nodes.get_mut(cur.index()) {
                n.flags.insert(NodeFlags::REMOVED);
                stack.extend_from_slice(&n.children);
            }
        }
        if let Some(p) = self.nodes.get_mut(parent.index()) {
            p.children.retain(|c| *c != id);
        }
        let mut cur = Some(parent);
        while let Some(c) = cur {
            let Some(n) = self.nodes.get_mut(c.index()) else {
                break;
            };
            n.size_allocated = n.size_allocated.saturating_sub(removed.0);
            n.size_apparent = n.size_apparent.saturating_sub(removed.1);
            n.file_count = n.file_count.saturating_sub(removed.2);
            cur = n.parent;
        }
        Some(removed)
    }
}

/// Incrementally builds a [`Tree`] during a scan.
pub(crate) struct TreeBuilder {
    nodes: Vec<Node>,
    root_path: PathBuf,
    size_mode: SizeMode,
    seen_hardlinks: HashSet<(u64, u64)>,
}

impl TreeBuilder {
    /// Starts a tree whose root node describes `root_path`.
    pub(crate) fn new(root_path: PathBuf, root: &EntryMeta, size_mode: SizeMode) -> Self {
        let mut b = TreeBuilder {
            nodes: Vec::new(),
            root_path: root_path.clone(),
            size_mode,
            seen_hardlinks: HashSet::new(),
        };
        let node = b.make_node(root_path.into_os_string(), None, root);
        b.nodes.push(node);
        b
    }

    pub(crate) fn root(&self) -> NodeId {
        NodeId(0)
    }

    fn make_node(&mut self, name: OsString, parent: Option<NodeId>, meta: &EntryMeta) -> Node {
        let mut flags = NodeFlags::NONE;
        let mut size_allocated = meta.size_allocated;
        let mut size_apparent = meta.size_apparent;
        let is_dir = meta.kind == NodeKind::Dir;
        // Track (dev, ino) only for multiply linked non-directories. The first
        // link seen is counted; later links contribute nothing.
        if !is_dir
            && meta.nlink > 1
            && meta.ino != 0
            && !self.seen_hardlinks.insert((meta.dev, meta.ino))
        {
            flags.insert(NodeFlags::HARDLINK_DUP);
            size_allocated = 0;
            size_apparent = 0;
        }
        Node {
            name: name.into_boxed_os_str(),
            parent,
            children: Vec::new(),
            kind: meta.kind,
            size_allocated,
            size_apparent,
            file_count: u32::from(!is_dir),
            mtime: meta.mtime,
            dev: meta.dev,
            ino: meta.ino,
            flags,
        }
    }

    /// Appends a child of `parent`. Returns `None` if the arena is full
    /// (more than `u32::MAX` nodes) or `parent` is invalid.
    pub(crate) fn push(
        &mut self,
        parent: NodeId,
        name: OsString,
        meta: &EntryMeta,
        flags: NodeFlags,
    ) -> Option<NodeId> {
        let id = NodeId(u32::try_from(self.nodes.len()).ok()?);
        if parent.index() >= self.nodes.len() {
            return None;
        }
        let mut node = self.make_node(name, Some(parent), meta);
        node.flags.insert(flags);
        self.nodes.push(node);
        self.nodes[parent.index()].children.push(id);
        Some(id)
    }

    /// Adds flags to an existing node.
    pub(crate) fn flag(&mut self, id: NodeId, flags: NodeFlags) {
        if let Some(n) = self.nodes.get_mut(id.index()) {
            n.flags.insert(flags);
        }
    }

    /// Replaces a directory's own allocated size (its index, not its
    /// contents) and returns the previous value. Used when the size is only
    /// known once the directory itself has been opened for listing.
    pub(crate) fn set_own_allocated(&mut self, id: NodeId, size: u64) -> u64 {
        match self.nodes.get_mut(id.index()) {
            Some(n) if n.kind == NodeKind::Dir => std::mem::replace(&mut n.size_allocated, size),
            _ => 0,
        }
    }

    /// Own (non-aggregated) sizes of a node.
    pub(crate) fn own_sizes(&self, id: NodeId) -> (u64, u64) {
        self.nodes
            .get(id.index())
            .map_or((0, 0), |n| (n.size_allocated, n.size_apparent))
    }

    /// Aggregates sizes bottom-up and returns the finished tree.
    pub(crate) fn finish(mut self, errors: Vec<ScanError>, complete: bool) -> Tree {
        // Children always have higher indices than their parents, so one
        // reverse pass visits every child before its parent.
        for i in (1..self.nodes.len()).rev() {
            let (parent, alloc, apparent, files) = {
                let n = &self.nodes[i];
                (n.parent, n.size_allocated, n.size_apparent, n.file_count)
            };
            if let Some(p) = parent {
                let p = &mut self.nodes[p.index()];
                p.size_allocated = p.size_allocated.saturating_add(alloc);
                p.size_apparent = p.size_apparent.saturating_add(apparent);
                p.file_count = p.file_count.saturating_add(files);
            }
        }
        Tree {
            nodes: self.nodes,
            root: NodeId(0),
            root_path: self.root_path,
            errors,
            complete,
            size_mode: self.size_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(kind: NodeKind, alloc: u64, apparent: u64) -> EntryMeta {
        EntryMeta {
            kind,
            size_allocated: alloc,
            size_apparent: apparent,
            nlink: 1,
            ..EntryMeta::default()
        }
    }

    fn sample() -> Tree {
        let root = meta(NodeKind::Dir, 4096, 0);
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &root, SizeMode::Allocated);
        let r = b.root();
        let a = b
            .push(
                r,
                "a".into(),
                &meta(NodeKind::Dir, 4096, 0),
                NodeFlags::NONE,
            )
            .unwrap();
        b.push(
            a,
            "big".into(),
            &meta(NodeKind::File, 8192, 8000),
            NodeFlags::NONE,
        )
        .unwrap();
        b.push(
            a,
            "small".into(),
            &meta(NodeKind::File, 4096, 10),
            NodeFlags::NONE,
        )
        .unwrap();
        b.push(r, "z".into(), &meta(NodeKind::File, 0, 0), NodeFlags::NONE)
            .unwrap();
        b.finish(Vec::new(), true)
    }

    #[test]
    fn aggregates_bottom_up() {
        let t = sample();
        let root = t.get(t.root()).unwrap();
        assert_eq!(root.size_allocated, 4096 + 4096 + 8192 + 4096);
        assert_eq!(root.size_apparent, 8010);
        assert_eq!(root.file_count, 3);
        let a = t.get(NodeId(1)).unwrap();
        assert_eq!(a.size_allocated, 4096 + 8192 + 4096);
        assert_eq!(a.file_count, 2);
        assert_eq!(t.dir_count(), 2);
    }

    #[test]
    fn hardlink_dup_contributes_zero() {
        let root = meta(NodeKind::Dir, 0, 0);
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &root, SizeMode::Allocated);
        let r = b.root();
        let linked = EntryMeta {
            nlink: 2,
            dev: 1,
            ino: 42,
            ..meta(NodeKind::File, 4096, 4000)
        };
        let first = b.push(r, "x".into(), &linked, NodeFlags::NONE).unwrap();
        let second = b.push(r, "y".into(), &linked, NodeFlags::NONE).unwrap();
        let t = b.finish(Vec::new(), true);
        assert!(!t
            .get(first)
            .unwrap()
            .flags
            .contains(NodeFlags::HARDLINK_DUP));
        assert!(t
            .get(second)
            .unwrap()
            .flags
            .contains(NodeFlags::HARDLINK_DUP));
        assert_eq!(t.total(SizeMode::Allocated), 4096);
        assert_eq!(t.total(SizeMode::Apparent), 4000);
        assert_eq!(t.get(t.root()).unwrap().file_count, 2);
    }

    #[test]
    fn single_link_files_are_not_tracked() {
        let root = meta(NodeKind::Dir, 0, 0);
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &root, SizeMode::Allocated);
        let r = b.root();
        // Same (dev, ino) but nlink == 1: e.g. Windows fallback stat with ino 0.
        let m = EntryMeta {
            dev: 1,
            ino: 7,
            ..meta(NodeKind::File, 10, 10)
        };
        b.push(r, "x".into(), &m, NodeFlags::NONE).unwrap();
        b.push(r, "y".into(), &m, NodeFlags::NONE).unwrap();
        assert_eq!(b.finish(Vec::new(), true).total(SizeMode::Apparent), 20);
    }

    #[test]
    fn paths_are_rebuilt_from_parents() {
        let t = sample();
        assert_eq!(t.path(t.root()).unwrap(), PathBuf::from("/r"));
        assert_eq!(
            t.path(NodeId(2)).unwrap(),
            Path::new("/r").join("a").join("big")
        );
        assert!(t.path(NodeId(99)).is_none());
    }

    #[test]
    fn sorting_and_largest() {
        let t = sample();
        let by_size = t.sorted_children(t.root(), SortKey::Size, SizeMode::Allocated);
        assert_eq!(by_size, vec![NodeId(1), NodeId(4)]);
        let by_name = t.sorted_children(NodeId(1), SortKey::Name, SizeMode::Allocated);
        assert_eq!(by_name, vec![NodeId(2), NodeId(3)]);
        assert_eq!(
            t.largest(LargestKind::Files, 2, SizeMode::Apparent),
            vec![NodeId(2), NodeId(3)]
        );
        assert_eq!(
            t.largest(LargestKind::Dirs, 10, SizeMode::Allocated),
            vec![NodeId(1)]
        );
        assert!(t
            .largest(LargestKind::Files, 0, SizeMode::Allocated)
            .is_empty());
    }

    #[test]
    fn remove_subtree_updates_ancestors_and_queries() {
        let mut t = sample();
        // Remove "a" (contains big + small).
        let removed = t.remove_subtree(NodeId(1)).unwrap();
        assert_eq!(removed, (4096 + 8192 + 4096, 8010, 2));
        let root = t.get(t.root()).unwrap();
        assert_eq!(root.size_allocated, 4096);
        assert_eq!(root.size_apparent, 0);
        assert_eq!(root.file_count, 1);
        assert_eq!(t.children(t.root()), &[NodeId(4)]);
        assert!(t.get(NodeId(2)).unwrap().flags.contains(NodeFlags::REMOVED));
        assert_eq!(
            t.largest(LargestKind::Files, 10, SizeMode::Allocated),
            vec![NodeId(4)]
        );
        assert_eq!(t.dir_count(), 1);
        // Idempotent, and the root can never be removed.
        assert!(t.remove_subtree(NodeId(1)).is_none());
        assert!(t.remove_subtree(t.root()).is_none());
        assert!(t.remove_subtree(NodeId(999)).is_none());
    }

    #[test]
    fn subtree_flag_and_ancestry() {
        let mut t = sample();
        assert!(!t.subtree_has_flag(NodeId(1), NodeFlags::ERROR));
        t.set_flag(NodeId(3), NodeFlags::ERROR);
        assert!(t.subtree_has_flag(NodeId(1), NodeFlags::ERROR));
        assert!(t.subtree_has_flag(t.root(), NodeFlags::ERROR));
        assert!(!t.subtree_has_flag(NodeId(4), NodeFlags::ERROR));
        assert!(t.is_ancestor_or_self(NodeId(1), NodeId(3)));
        assert!(t.is_ancestor_or_self(NodeId(3), NodeId(3)));
        assert!(!t.is_ancestor_or_self(NodeId(3), NodeId(1)));
        assert!(!t.is_ancestor_or_self(NodeId(4), NodeId(2)));
    }

    #[test]
    fn flag_names() {
        let f = NodeFlags::ERROR | NodeFlags::SKIPPED;
        assert_eq!(f.names(), vec!["error", "skipped"]);
        assert!(NodeFlags::NONE.is_empty());
    }
}
