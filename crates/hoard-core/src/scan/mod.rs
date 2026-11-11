//! Parallel directory walk producing a [`Tree`].
//!
//! The walk uses `jwalk` (rayon). Every directory listing is post-processed on
//! a worker thread (`process_read_dir`), where each entry is stat'ed (via
//! [`platform::DirStat`], one per directory) without following symlinks and descent is pruned for links, other filesystems,
//! skipped paths and cancellation. The calling thread consumes entries in
//! depth-first order and appends them to the arena.
//!
//! Filesystem errors never abort the walk: each becomes a [`ScanError`] and
//! the affected node is flagged [`NodeFlags::ERROR`].

mod progress;

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::FileType;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use jwalk::{Parallelism, WalkDirGeneric};
use serde::{Deserialize, Serialize};

pub use progress::{
    ScanEvent, TopLevelEntry, BATCH_INTERVAL, BATCH_MAX_ENTRIES, PROGRESS_INTERVAL,
};

use crate::error::{HoardError, Result};
use crate::platform::{self, EntryMeta};
use crate::size::SizeMode;
use crate::tree::{NodeFlags, NodeId, NodeKind, Tree, TreeBuilder};
use progress::Throttle;

/// Options for [`scan`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanOptions {
    /// Descend into directories on other filesystems / volumes. Default false.
    pub cross_filesystems: bool,
    /// Report apparent instead of allocated size by default. Both are always
    /// recorded; this only sets [`Tree::size_mode`] and progress byte counts.
    pub apparent: bool,
    /// Directories to list but not descend into. Relative paths are resolved
    /// against the current directory.
    pub skip: Vec<PathBuf>,
    /// Worker threads; 0 picks a default based on the CPU count.
    pub threads: usize,
}

/// Which operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanErrorKind {
    /// Listing a directory's contents failed (typically permission denied).
    ReadDir,
    /// Reading an entry's metadata failed.
    Stat,
    /// The walker could not produce an entry.
    Entry,
}

/// A filesystem error recorded during a scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanError {
    /// The path that failed.
    pub path: PathBuf,
    /// What was being attempted.
    pub kind: ScanErrorKind,
    /// The OS error message.
    pub message: String,
}

/// Per-entry state filled in by worker threads.
#[derive(Debug, Default)]
struct EntryState {
    meta: Option<std::result::Result<EntryMeta, String>>,
    flags: NodeFlags,
    /// Drop this entry from the tree (pseudo filesystem).
    excluded: bool,
    /// The own allocated size of the directory being listed, carried by its
    /// first child (see [`platform::DirStat::own_allocated`]).
    parent_alloc: Option<u64>,
}

type Client = ((), EntryState);

/// Read-only context shared with the worker threads.
struct WalkContext {
    cancel: Arc<AtomicBool>,
    cross_filesystems: bool,
    allowed_devs: Vec<u64>,
    /// Dropped from the tree entirely (pseudo filesystems).
    excluded: HashSet<PathBuf>,
    /// Kept as a node but not descended into (user skips).
    skipped: HashSet<PathBuf>,
}

impl WalkContext {
    fn process(
        &self,
        dir: &Path,
        children: &mut Vec<jwalk::Result<jwalk::DirEntry<Client>>>,
        is_root: bool,
    ) {
        if self.cancel.load(Ordering::Relaxed) {
            children.clear();
            return;
        }
        if children.is_empty() {
            return;
        }
        let dir_stat = platform::DirStat::open(dir);
        let mut drop_any = false;
        for child in children.iter_mut() {
            let Ok(entry) = child else { continue };
            let path = entry.path();
            if !self.excluded.is_empty() && self.excluded.contains(&path) {
                entry.read_children = None;
                entry.client_state.excluded = true;
                drop_any = true;
                continue;
            }
            match dir_stat.stat(&entry.file_name, &path) {
                Ok(meta) => {
                    if meta.kind != NodeKind::Dir {
                        // Never descend into anything but a real directory.
                        entry.read_children = None;
                    } else if !is_root
                        && !self.cross_filesystems
                        && !self.allowed_devs.contains(&meta.dev)
                    {
                        entry.read_children = None;
                        entry.client_state.flags.insert(NodeFlags::OTHER_FS);
                    } else if !self.skipped.is_empty() && self.skipped.contains(&path) {
                        entry.read_children = None;
                        entry.client_state.flags.insert(NodeFlags::SKIPPED);
                    }
                    entry.client_state.meta = Some(Ok(meta));
                }
                Err(e) => {
                    entry.read_children = None;
                    entry.client_state.meta = Some(Err(e.to_string()));
                }
            }
        }
        if drop_any {
            children.retain(|c| !matches!(c, Ok(e) if e.client_state.excluded));
        }
        if let Some(size) = dir_stat.own_allocated() {
            if let Some(Ok(first)) = children.iter_mut().find(|c| c.is_ok()) {
                first.client_state.parent_alloc = Some(size);
            }
        }
    }
}

/// Scans `root` and returns the tree.
///
/// - Symlinks and junctions are never followed, including when `root` itself
///   is one (the tree is then a single [`NodeKind::Symlink`] node).
/// - Hard links are counted once.
/// - Errors on anything below the root are recorded, not returned.
/// - Setting `cancel` stops the walk between directory reads; the partial
///   tree is returned with [`Tree::is_complete`] false.
///
/// Returns an error only if the root itself cannot be resolved or stat'ed.
pub fn scan(
    root: &Path,
    opts: ScanOptions,
    progress: Sender<ScanEvent>,
    cancel: Arc<AtomicBool>,
) -> Result<Tree> {
    let started = Instant::now();
    let root_path = std::path::absolute(root).map_err(|source| HoardError::Root {
        path: root.to_path_buf(),
        source,
    })?;
    let root_meta = platform::stat(&root_path).map_err(|source| HoardError::Root {
        path: root_path.clone(),
        source,
    })?;
    let size_mode = if opts.apparent {
        SizeMode::Apparent
    } else {
        SizeMode::Allocated
    };
    let mut builder = TreeBuilder::new(root_path.clone(), &root_meta, size_mode);

    if root_meta.kind != NodeKind::Dir {
        // Do not hand a symlinked root to jwalk: it always follows the root.
        return Ok(builder.finish(Vec::new(), true));
    }

    let mut allowed_devs = vec![root_meta.dev];
    allowed_devs.extend(platform::same_filesystem_devs(&root_path));
    let mut excluded: HashSet<PathBuf> = platform::pseudo_fs_paths()
        .iter()
        .map(PathBuf::from)
        .collect();
    excluded.extend(platform::extra_skips(&root_path));
    // Never exclude the root itself (e.g. the user explicitly scans /proc).
    excluded.remove(&root_path);
    let skipped: HashSet<PathBuf> = opts
        .skip
        .iter()
        .filter_map(|p| std::path::absolute(p).ok())
        .collect();

    let ctx = WalkContext {
        cancel: Arc::clone(&cancel),
        cross_filesystems: opts.cross_filesystems,
        allowed_devs,
        excluded,
        skipped,
    };
    let threads = if opts.threads > 0 {
        opts.threads
    } else {
        default_threads()
    };

    let walker = WalkDirGeneric::<Client>::new(&root_path)
        .skip_hidden(false)
        .follow_links(false)
        .sort(false)
        .parallelism(Parallelism::RayonNewPool(threads))
        .process_read_dir(move |depth, path, _state, children| {
            ctx.process(path, children, depth.is_none());
        });

    let mut state = WalkState::new(size_mode, progress, started);
    let mut errors: Vec<ScanError> = Vec::new();
    // stack[d] is the directory node at depth d on the current path.
    let mut stack: Vec<NodeId> = Vec::new();
    let mut complete = true;

    for item in walker {
        if cancel.load(Ordering::Relaxed) {
            complete = false;
            break;
        }
        match item {
            Ok(mut entry) => {
                let depth = entry.depth;
                let read_error = entry
                    .read_children
                    .as_ref()
                    .and_then(|rc| rc.error())
                    .map(|e| e.to_string());
                let id = if depth == 0 {
                    builder.root()
                } else {
                    stack.truncate(depth);
                    let Some(&parent) = stack.last() else {
                        // Depth-first order guarantees a parent; skip defensively.
                        continue;
                    };
                    if let Some(size) = entry.client_state.parent_alloc.take() {
                        let before = builder.set_own_allocated(parent, size);
                        state.adjust_dir_alloc(depth - 1, before, size);
                    }
                    let (meta, flags) = match entry.client_state.meta.take() {
                        Some(Ok(m)) => (m, entry.client_state.flags),
                        Some(Err(message)) => {
                            errors.push(ScanError {
                                path: entry.path(),
                                kind: ScanErrorKind::Stat,
                                message,
                            });
                            let fallback = EntryMeta {
                                kind: kind_from_file_type(entry.file_type),
                                ..EntryMeta::default()
                            };
                            (fallback, NodeFlags::ERROR)
                        }
                        // Not processed (cancelled mid-listing).
                        None => continue,
                    };
                    let Some(id) = builder.push(parent, entry.file_name.clone(), &meta, flags)
                    else {
                        // Arena full (> u32::MAX nodes): stop with a partial tree.
                        complete = false;
                        break;
                    };
                    id
                };
                if let Some(message) = read_error {
                    errors.push(ScanError {
                        path: entry.path(),
                        kind: ScanErrorKind::ReadDir,
                        message,
                    });
                    builder.flag(id, NodeFlags::ERROR);
                }
                let is_dir = depth == 0 || entry.file_type.is_dir();
                if is_dir {
                    stack.truncate(depth);
                    stack.push(id);
                }
                state.record(&builder, id, depth, is_dir, &entry.file_name);
                if state.tick_due() {
                    state.emit_progress(&entry.path(), errors.len());
                    state.maybe_emit_batch(false);
                }
            }
            Err(err) => {
                let path = err
                    .path()
                    .map_or_else(|| root_path.clone(), Path::to_path_buf);
                let depth = err.depth();
                if depth > 0 {
                    if let Some(&parent) = stack.get(depth - 1) {
                        builder.flag(parent, NodeFlags::ERROR);
                    }
                }
                errors.push(ScanError {
                    path,
                    kind: ScanErrorKind::Entry,
                    message: err.to_string(),
                });
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        complete = false;
    }
    state.emit_progress(&root_path, errors.len());
    state.maybe_emit_batch(true);
    tracing::debug!(
        files = state.files,
        dirs = state.dirs,
        errors = errors.len(),
        complete,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "scan finished"
    );
    Ok(builder.finish(errors, complete))
}

fn kind_from_file_type(ft: FileType) -> NodeKind {
    if ft.is_symlink() {
        NodeKind::Symlink
    } else if ft.is_dir() {
        NodeKind::Dir
    } else if ft.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    }
}

fn default_threads() -> usize {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    // Stat calls block on IO, so oversubscribe a little.
    (cpus * 2).clamp(2, 64)
}

/// Counters and event emission for the consuming thread.
struct WalkState {
    size_mode: SizeMode,
    sender: Sender<ScanEvent>,
    started: Instant,
    files: u64,
    dirs: u64,
    bytes: u64,
    since_check: u32,
    progress_throttle: Throttle,
    batch_throttle: Throttle,
    /// Running totals for the root's direct children.
    top: Vec<TopLevelEntry>,
}

impl WalkState {
    fn new(size_mode: SizeMode, sender: Sender<ScanEvent>, started: Instant) -> Self {
        WalkState {
            size_mode,
            sender,
            started,
            files: 0,
            dirs: 0,
            bytes: 0,
            since_check: 0,
            progress_throttle: Throttle::new(PROGRESS_INTERVAL),
            batch_throttle: Throttle::new(BATCH_INTERVAL),
            top: Vec::new(),
        }
    }

    fn record(
        &mut self,
        builder: &TreeBuilder,
        id: NodeId,
        depth: usize,
        is_dir: bool,
        name: &OsString,
    ) {
        let (alloc, apparent) = builder.own_sizes(id);
        if is_dir {
            self.dirs += 1;
        } else {
            self.files += 1;
        }
        self.bytes += match self.size_mode {
            SizeMode::Allocated => alloc,
            SizeMode::Apparent => apparent,
        };
        if depth == 1 {
            self.top.push(TopLevelEntry {
                id,
                name: name.to_string_lossy().into_owned(),
                kind: if is_dir {
                    NodeKind::Dir
                } else {
                    NodeKind::File
                },
                size_allocated: 0,
                size_apparent: 0,
                file_count: 0,
            });
        }
        // Depth-first order: every entry below depth 1 belongs to the most
        // recent depth-1 entry.
        if depth >= 1 {
            if let Some(t) = self.top.last_mut() {
                t.size_allocated += alloc;
                t.size_apparent += apparent;
                t.file_count += u64::from(!is_dir);
            }
        }
    }

    /// Accounts for a directory's own allocated size changing from `before`
    /// to `after` after it was recorded (`depth` is the directory's depth).
    fn adjust_dir_alloc(&mut self, depth: usize, before: u64, after: u64) {
        if self.size_mode == SizeMode::Allocated {
            self.bytes = (self.bytes + after).saturating_sub(before);
        }
        if depth >= 1 {
            if let Some(t) = self.top.last_mut() {
                t.size_allocated = (t.size_allocated + after).saturating_sub(before);
            }
        }
    }

    /// Cheap check, consulting the clock only every 256 entries.
    fn tick_due(&mut self) -> bool {
        self.since_check += 1;
        if self.since_check < 256 {
            return false;
        }
        self.since_check = 0;
        self.progress_throttle.ready(Instant::now())
    }

    fn emit_progress(&mut self, current: &Path, errors: usize) {
        let _ = self.sender.try_send(ScanEvent::Progress {
            files: self.files,
            dirs: self.dirs,
            bytes: self.bytes,
            current_path: current.to_path_buf(),
            errors: errors as u64,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
    }

    fn maybe_emit_batch(&mut self, force: bool) {
        if !force && !self.batch_throttle.ready(Instant::now()) {
            return;
        }
        let mode = self.size_mode;
        let mut top = self.top.clone();
        top.sort_by_key(|t| {
            std::cmp::Reverse(match mode {
                SizeMode::Allocated => t.size_allocated,
                SizeMode::Apparent => t.size_apparent,
            })
        });
        top.truncate(BATCH_MAX_ENTRIES);
        let _ = self.sender.try_send(ScanEvent::Batch { top_level: top });
    }
}
