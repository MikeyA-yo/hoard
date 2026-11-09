//! Snapshots and diffs (PRD 6.6).
//!
//! A snapshot keeps every directory down to depth 6 plus any deeper
//! directory over 50 MB, with both sizes and file counts. Stored as postcard
//! compressed with LZ4 (pure Rust, so cross-target checks need no C
//! toolchain). One folder per scan root, newest first, pruned to a limit.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::size::SizeMode;
use crate::tree::{NodeFlags, NodeId, Tree};

/// Directories at or above this depth are always kept.
pub const SNAPSHOT_DEPTH: usize = 6;
/// Deeper directories are kept if at least this big (on disk).
pub const SNAPSHOT_MIN_BYTES: u64 = 50 * 1024 * 1024;
/// Snapshots kept per root.
pub const KEEP_PER_ROOT: usize = 30;
const FORMAT_VERSION: u32 = 1;
const EXTENSION: &str = "hsnap";

/// One directory in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapEntry {
    /// Path relative to the root with `/` separators; empty for the root.
    pub path: String,
    /// Depth below the root (root = 0).
    pub depth: u16,
    /// On-disk bytes.
    pub size_allocated: u64,
    /// Logical bytes.
    pub size_apparent: u64,
    /// Files below.
    pub files: u32,
}

/// A saved summary of one scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Format version.
    pub version: u32,
    /// The scanned root, as text.
    pub root: String,
    /// Unix seconds.
    pub taken_at: i64,
    /// False if the scan was cancelled.
    pub complete: bool,
    /// Directories, parents before children.
    pub entries: Vec<SnapEntry>,
}

impl Snapshot {
    /// Builds a snapshot of `tree` at time `now`.
    pub fn take(tree: &Tree, now: i64) -> Snapshot {
        let mut entries = Vec::new();
        // (node, relative path, depth)
        let mut stack: Vec<(NodeId, String, usize)> = vec![(tree.root(), String::new(), 0)];
        while let Some((id, path, depth)) = stack.pop() {
            let Some(n) = tree.get(id) else { continue };
            if !n.is_dir() || n.flags.contains(NodeFlags::REMOVED) {
                continue;
            }
            if depth <= SNAPSHOT_DEPTH || n.size_allocated >= SNAPSHOT_MIN_BYTES {
                entries.push(SnapEntry {
                    path: path.clone(),
                    depth: u16::try_from(depth).unwrap_or(u16::MAX),
                    size_allocated: n.size_allocated,
                    size_apparent: n.size_apparent,
                    files: n.file_count,
                });
            } else {
                // Below the depth limit only big folders matter, and a small
                // folder cannot contain a big one.
                continue;
            }
            for &c in &n.children {
                if let Some(child) = tree.get(c) {
                    if child.is_dir() {
                        let name = child.name.to_string_lossy();
                        let p = if path.is_empty() {
                            name.into_owned()
                        } else {
                            format!("{path}/{name}")
                        };
                        stack.push((c, p, depth + 1));
                    }
                }
            }
        }
        entries.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.path.cmp(&b.path)));
        Snapshot {
            version: FORMAT_VERSION,
            root: tree.root_path().to_string_lossy().into_owned(),
            taken_at: now,
            complete: tree.is_complete(),
            entries,
        }
    }

    /// Total of the root entry.
    pub fn total(&self, mode: SizeMode) -> u64 {
        self.entries
            .iter()
            .find(|e| e.path.is_empty())
            .map_or(0, |e| match mode {
                SizeMode::Allocated => e.size_allocated,
                SizeMode::Apparent => e.size_apparent,
            })
    }

    /// Serializes to the on-disk format.
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let raw = postcard::to_allocvec(self).map_err(io::Error::other)?;
        Ok(lz4_flex::compress_prepend_size(&raw))
    }

    /// Parses the on-disk format.
    pub fn decode(bytes: &[u8]) -> io::Result<Snapshot> {
        let raw = lz4_flex::decompress_size_prepended(bytes).map_err(io::Error::other)?;
        let snap: Snapshot = postcard::from_bytes(&raw).map_err(io::Error::other)?;
        if snap.version != FORMAT_VERSION {
            return Err(io::Error::other(format!(
                "unsupported snapshot version {}",
                snap.version
            )));
        }
        Ok(snap)
    }
}

/// A saved snapshot on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// File holding it.
    pub path: PathBuf,
    /// Unix seconds.
    pub taken_at: i64,
}

/// Snapshot files under one directory, one subfolder per scan root.
#[derive(Clone, Debug)]
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    /// A store rooted at `dir` (created on first save).
    pub fn new(dir: PathBuf) -> Self {
        SnapshotStore { dir }
    }

    fn root_dir(&self, root: &str) -> PathBuf {
        let key = blake3::hash(root.to_lowercase().as_bytes()).to_hex();
        self.dir.join(&key.as_str()[..16])
    }

    /// Saves `snap` and prunes old snapshots of the same root.
    pub fn save(&self, snap: &Snapshot) -> io::Result<PathBuf> {
        let dir = self.root_dir(&snap.root);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.{EXTENSION}", snap.taken_at));
        // Write then rename, so a crash never leaves a half-written snapshot.
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, snap.encode()?)?;
        fs::rename(&tmp, &path)?;
        for old in self.list(&snap.root)?.into_iter().skip(KEEP_PER_ROOT) {
            let _ = fs::remove_file(old.path);
        }
        Ok(path)
    }

    /// Snapshots of `root`, newest first.
    pub fn list(&self, root: &str) -> io::Result<Vec<SnapshotInfo>> {
        let dir = self.root_dir(root);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out: Vec<SnapshotInfo> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|p| {
                let taken_at = p.file_stem()?.to_str()?.parse().ok()?;
                Some(SnapshotInfo { path: p, taken_at })
            })
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.taken_at));
        Ok(out)
    }

    /// Loads one snapshot.
    pub fn load(&self, path: &Path) -> io::Result<Snapshot> {
        Snapshot::decode(&fs::read(path)?)
    }
}

/// How a directory changed between two snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Bigger than before.
    Grew,
    /// Smaller than before.
    Shrank,
    /// Did not exist before.
    New,
    /// No longer exists.
    Removed,
}

/// One changed directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    /// Path relative to the root (`/` separators, empty = root).
    pub path: String,
    /// Size before (None = new).
    pub before: Option<u64>,
    /// Size after (None = removed).
    pub after: Option<u64>,
}

impl DiffEntry {
    /// Signed change in bytes.
    pub fn change(&self) -> i64 {
        let b = i64::try_from(self.before.unwrap_or(0)).unwrap_or(i64::MAX);
        let a = i64::try_from(self.after.unwrap_or(0)).unwrap_or(i64::MAX);
        a - b
    }

    /// Grew, shrank, new or removed.
    pub fn kind(&self) -> ChangeKind {
        match (self.before, self.after) {
            (None, _) => ChangeKind::New,
            (_, None) => ChangeKind::Removed,
            _ if self.change() >= 0 => ChangeKind::Grew,
            _ => ChangeKind::Shrank,
        }
    }
}

/// Changes from `old` to `new`, largest absolute change first. Directories
/// deeper than [`SNAPSHOT_DEPTH`] are only kept when over 50 MB, so a deep
/// one missing from one side is compared only if present in both (it may
/// simply have crossed the size threshold).
pub fn diff(old: &Snapshot, new: &Snapshot, mode: SizeMode) -> Vec<DiffEntry> {
    let size = |e: &SnapEntry| match mode {
        SizeMode::Allocated => e.size_allocated,
        SizeMode::Apparent => e.size_apparent,
    };
    let before: HashMap<&str, &SnapEntry> =
        old.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let after: HashMap<&str, &SnapEntry> =
        new.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let shallow = |e: &SnapEntry| usize::from(e.depth) <= SNAPSHOT_DEPTH;

    let mut out: Vec<DiffEntry> = Vec::new();
    for e in &new.entries {
        match before.get(e.path.as_str()) {
            Some(b) if size(b) != size(e) => out.push(DiffEntry {
                path: e.path.clone(),
                before: Some(size(b)),
                after: Some(size(e)),
            }),
            Some(_) => {}
            None if shallow(e) => out.push(DiffEntry {
                path: e.path.clone(),
                before: None,
                after: Some(size(e)),
            }),
            None => {}
        }
    }
    for e in &old.entries {
        if !after.contains_key(e.path.as_str()) && shallow(e) {
            out.push(DiffEntry {
                path: e.path.clone(),
                before: Some(size(e)),
                after: None,
            });
        }
    }
    out.sort_by(|a, b| {
        b.change()
            .unsigned_abs()
            .cmp(&a.change().unsigned_abs())
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// The most specific growers: biggest positive changes, skipping a folder
/// when one child explains at least 80% of its growth (the child is shown
/// instead), and skipping the root.
pub fn top_growers(changes: &[DiffEntry], limit: usize) -> Vec<DiffEntry> {
    let growth: HashMap<&str, i64> = changes
        .iter()
        .map(|d| (d.path.as_str(), d.change()))
        .collect();
    let explained_by_child = |d: &DiffEntry| {
        let prefix = if d.path.is_empty() {
            String::new()
        } else {
            format!("{}/", d.path)
        };
        growth.iter().any(|(p, &c)| {
            p.len() > prefix.len()
                && p.starts_with(&prefix)
                && !p[prefix.len()..].contains('/')
                && c * 10 >= d.change() * 8
        })
    };
    changes
        .iter()
        .filter(|d| d.change() > 0 && !d.path.is_empty() && !explained_by_child(d))
        .take(limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> SnapEntry {
        SnapEntry {
            path: path.into(),
            depth: if path.is_empty() {
                0
            } else {
                path.matches('/').count() as u16 + 1
            },
            size_allocated: size,
            size_apparent: size,
            files: 1,
        }
    }

    fn snap(t: i64, entries: Vec<SnapEntry>) -> Snapshot {
        Snapshot {
            version: FORMAT_VERSION,
            root: "C:\\Users\\me".into(),
            taken_at: t,
            complete: true,
            entries,
        }
    }

    #[test]
    fn diff_classifies_and_sorts() {
        let old = snap(
            1,
            vec![
                entry("", 1000),
                entry("a", 600),
                entry("a/x", 500),
                entry("b", 300),
                entry("gone", 100),
            ],
        );
        let new = snap(
            2,
            vec![
                entry("", 1500),
                entry("a", 1100),
                entry("a/x", 1000),
                entry("b", 250),
                entry("fresh", 150),
            ],
        );
        let d = diff(&old, &new, SizeMode::Allocated);
        let summary: Vec<(String, ChangeKind, i64)> = d
            .iter()
            .map(|e| (e.path.clone(), e.kind(), e.change()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("".into(), ChangeKind::Grew, 500),
                ("a".into(), ChangeKind::Grew, 500),
                ("a/x".into(), ChangeKind::Grew, 500),
                ("fresh".into(), ChangeKind::New, 150),
                ("gone".into(), ChangeKind::Removed, -100),
                ("b".into(), ChangeKind::Shrank, -50),
            ]
        );
        // a's growth is explained by a/x, so a/x is the grower shown.
        let top: Vec<String> = top_growers(&d, 5).into_iter().map(|e| e.path).collect();
        assert_eq!(top, vec!["a/x", "fresh"]);
    }

    #[test]
    fn deep_entries_crossing_the_threshold_are_not_reported_as_new() {
        let mut deep = entry("1/2/3/4/5/6/7", SNAPSHOT_MIN_BYTES);
        deep.depth = 7;
        let old = snap(1, vec![entry("", 10)]);
        let new = snap(2, vec![entry("", 10), deep]);
        assert!(diff(&old, &new, SizeMode::Allocated).is_empty());
    }

    #[test]
    fn take_keeps_shallow_dirs_and_big_deep_ones() {
        use crate::platform::EntryMeta;
        use crate::tree::{NodeFlags, NodeKind, TreeBuilder};
        let dir = EntryMeta {
            kind: NodeKind::Dir,
            nlink: 1,
            ..EntryMeta::default()
        };
        let file = |size| EntryMeta {
            kind: NodeKind::File,
            size_allocated: size,
            size_apparent: size,
            nlink: 1,
            ..EntryMeta::default()
        };
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &dir, SizeMode::Allocated);
        // A chain of 9 nested dirs; a big file at depth 9 and a small one at depth 8.
        let mut cur = b.root();
        let mut chain = Vec::new();
        for i in 1..=9 {
            cur = b
                .push(cur, format!("d{i}").into(), &dir, NodeFlags::NONE)
                .unwrap();
            chain.push(cur);
        }
        b.push(
            cur,
            "big".into(),
            &file(SNAPSHOT_MIN_BYTES),
            NodeFlags::NONE,
        )
        .unwrap();
        let side = b
            .push(chain[6], "small".into(), &dir, NodeFlags::NONE)
            .unwrap();
        b.push(side, "f".into(), &file(10), NodeFlags::NONE)
            .unwrap();
        let tree = b.finish(Vec::new(), true);

        let s = Snapshot::take(&tree, 42);
        let paths: Vec<&str> = s.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&""), "root kept");
        assert!(paths.contains(&"d1/d2/d3/d4/d5/d6"), "depth 6 kept");
        assert!(
            paths.contains(&"d1/d2/d3/d4/d5/d6/d7/d8/d9"),
            "big deep dir kept"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("/small")),
            "small deep dir dropped: {paths:?}"
        );
        assert_eq!(s.total(SizeMode::Allocated), SNAPSHOT_MIN_BYTES + 10);
        assert_eq!(s.taken_at, 42);
    }

    #[test]
    fn encode_round_trip_and_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(tmp.path().to_path_buf());
        for t in 0..(KEEP_PER_ROOT as i64 + 3) {
            store.save(&snap(t, vec![entry("", t as u64)])).unwrap();
        }
        let list = store.list("c:\\users\\ME").unwrap();
        assert_eq!(
            list.len(),
            KEEP_PER_ROOT,
            "pruned, and root lookup is case-insensitive"
        );
        assert_eq!(list[0].taken_at, KEEP_PER_ROOT as i64 + 2);
        let loaded = store.load(&list[0].path).unwrap();
        assert_eq!(loaded.total(SizeMode::Allocated), KEEP_PER_ROOT as u64 + 2);
        assert!(store.list("D:\\other").unwrap().is_empty());
        assert!(Snapshot::decode(b"garbage").is_err());
    }
}
