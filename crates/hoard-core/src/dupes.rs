//! Duplicate file finder (PRD L6).
//!
//! Pipeline: group by size, then by a BLAKE3 hash of the first and last
//! 64 KiB, then by a full BLAKE3 hash. Only files that agree at every stage
//! are reported.
//!
//! Work is split so no lock is held while reading files: [`candidates`]
//! takes a quick snapshot from the tree, [`find_duplicates`] does the IO.
//! Read errors skip the file (counted), never abort the job.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::tree::{NodeFlags, NodeId, NodeKind, Tree};

/// Bytes hashed at each end of a file in the partial stage.
pub const EDGE_BYTES: u64 = 64 * 1024;
/// Default smallest file considered (smaller duplicates waste little).
pub const DEFAULT_MIN_SIZE: u64 = 1024 * 1024;

/// A file to consider.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Tree node.
    pub id: NodeId,
    /// Full path at scan time.
    pub path: PathBuf,
    /// Logical size in bytes.
    pub size: u64,
}

/// Files with identical content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DupGroup {
    /// Size of each copy.
    pub size: u64,
    /// Full BLAKE3 hash, hex.
    pub hash: String,
    /// The copies (at least two), in tree order.
    pub nodes: Vec<NodeId>,
}

impl DupGroup {
    /// Bytes that removing all but one copy would free.
    pub fn wasted(&self) -> u64 {
        self.size
            .saturating_mul(self.nodes.len().saturating_sub(1) as u64)
    }
}

/// Progress of a running job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DupProgress {
    /// 1 = partial hashes, 2 = full hashes.
    pub stage: u8,
    /// Files hashed in this stage.
    pub files_done: u64,
    /// Files to hash in this stage.
    pub files_total: u64,
    /// Bytes read in this stage.
    pub bytes_done: u64,
    /// Bytes to read in this stage.
    pub bytes_total: u64,
}

/// Result of a job.
#[derive(Clone, Debug, Default)]
pub struct DupResult {
    /// Groups, most wasted space first.
    pub groups: Vec<DupGroup>,
    /// Files that could not be read.
    pub unreadable: u64,
    /// True if cancelled before finishing (groups are then empty).
    pub cancelled: bool,
}

/// Snapshot of regular files at least `min_size` bytes, skipping removed
/// nodes and hard-link duplicates (they share data with a counted link).
pub fn candidates(tree: &Tree, min_size: u64) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut stack = vec![tree.root()];
    while let Some(id) = stack.pop() {
        let Some(n) = tree.get(id) else { continue };
        if n.flags
            .intersects(NodeFlags::REMOVED | NodeFlags::HARDLINK_DUP)
        {
            continue;
        }
        match n.kind {
            NodeKind::Dir => stack.extend_from_slice(&n.children),
            NodeKind::File if n.size_apparent >= min_size.max(1) => {
                if let Some(path) = tree.path(id) {
                    out.push(Candidate {
                        id,
                        path,
                        size: n.size_apparent,
                    });
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|c| c.id);
    out
}

fn partial_hash(c: &Candidate) -> io::Result<[u8; 32]> {
    let mut f = File::open(&c.path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; EDGE_BYTES as usize];
    let head = read_up_to(&mut f, &mut buf)?;
    h.update(&buf[..head]);
    if c.size > 2 * EDGE_BYTES {
        f.seek(SeekFrom::End(-(EDGE_BYTES as i64)))?;
        let tail = read_up_to(&mut f, &mut buf)?;
        h.update(&buf[..tail]);
    }
    Ok(*h.finalize().as_bytes())
}

fn read_up_to(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn full_hash(c: &Candidate, cancel: &AtomicBool, read: &AtomicU64) -> io::Result<Option<[u8; 32]>> {
    let mut f = File::open(&c.path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        read.fetch_add(k as u64, Ordering::Relaxed);
    }
    Ok(Some(*h.finalize().as_bytes()))
}

/// Keeps only buckets with two or more members.
fn multi<K: std::hash::Hash + Eq>(buckets: HashMap<K, Vec<Candidate>>) -> Vec<Vec<Candidate>> {
    buckets.into_values().filter(|v| v.len() > 1).collect()
}

/// Runs the pipeline. `progress` is called from the calling thread between
/// batches; `cancel` is checked per file and per 1 MiB read.
pub fn find_duplicates(
    cands: Vec<Candidate>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(DupProgress),
) -> DupResult {
    let unreadable = AtomicU64::new(0);

    // Stage 0: same size.
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    for c in cands {
        by_size.entry(c.size).or_default().push(c);
    }
    let same_size: Vec<Candidate> = multi(by_size).into_iter().flatten().collect();

    // Stage 1: partial hash.
    let stage1 = run_stage(1, same_size, cancel, progress, |c, _| {
        partial_hash(c)
            .map(Some)
            .map_err(|_| unreadable.fetch_add(1, Ordering::Relaxed))
            .ok()
            .flatten()
    });
    let Some(stage1) = stage1 else {
        return DupResult {
            cancelled: true,
            ..DupResult::default()
        };
    };
    let mut by_partial: HashMap<(u64, [u8; 32]), Vec<Candidate>> = HashMap::new();
    for (c, h) in stage1 {
        by_partial.entry((c.size, h)).or_default().push(c);
    }
    let same_partial: Vec<Candidate> = multi(by_partial).into_iter().flatten().collect();

    // Stage 2: full hash (small files are already fully covered by stage 1,
    // but hashing them again keeps one rule and is cheap).
    let stage2 = run_stage(
        2,
        same_partial,
        cancel,
        progress,
        |c, read| match full_hash(c, cancel, read) {
            Ok(h) => h,
            Err(_) => {
                unreadable.fetch_add(1, Ordering::Relaxed);
                None
            }
        },
    );
    let Some(stage2) = stage2 else {
        return DupResult {
            cancelled: true,
            ..DupResult::default()
        };
    };
    let mut by_full: HashMap<(u64, [u8; 32]), Vec<Candidate>> = HashMap::new();
    for (c, h) in stage2 {
        by_full.entry((c.size, h)).or_default().push(c);
    }

    let mut groups: Vec<DupGroup> = by_full
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|((size, hash), v)| {
            let mut nodes: Vec<NodeId> = v.into_iter().map(|c| c.id).collect();
            nodes.sort();
            DupGroup {
                size,
                hash: hash.iter().map(|b| format!("{b:02x}")).collect(),
                nodes,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then_with(|| a.nodes.cmp(&b.nodes))
    });
    DupResult {
        groups,
        unreadable: unreadable.load(Ordering::Relaxed),
        cancelled: false,
    }
}

/// Hashes `items` in parallel batches, reporting progress between batches.
/// Returns `None` if cancelled.
fn run_stage(
    stage: u8,
    items: Vec<Candidate>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(DupProgress),
    hash: impl Fn(&Candidate, &AtomicU64) -> Option<[u8; 32]> + Sync,
) -> Option<Vec<(Candidate, [u8; 32])>> {
    let files_total = items.len() as u64;
    let bytes_total: u64 = items
        .iter()
        .map(|c| {
            if stage == 1 {
                c.size.min(2 * EDGE_BYTES)
            } else {
                c.size
            }
        })
        .sum();
    let read = AtomicU64::new(0);
    let mut out = Vec::with_capacity(items.len());
    let mut done = 0u64;
    progress(DupProgress {
        stage,
        files_done: 0,
        files_total,
        bytes_done: 0,
        bytes_total,
    });
    // Batches keep progress responsive without per-file callbacks across threads.
    for batch in items.chunks(64) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let hashed: Vec<(Candidate, Option<[u8; 32]>)> = batch
            .par_iter()
            .map(|c| (c.clone(), hash(c, &read)))
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        done += batch.len() as u64;
        if stage == 1 {
            read.fetch_add(
                batch.iter().map(|c| c.size.min(2 * EDGE_BYTES)).sum(),
                Ordering::Relaxed,
            );
        }
        out.extend(hashed.into_iter().filter_map(|(c, h)| h.map(|h| (c, h))));
        progress(DupProgress {
            stage,
            files_done: done,
            files_total,
            bytes_done: read.load(Ordering::Relaxed).min(bytes_total),
            bytes_total,
        });
    }
    Some(out)
}
