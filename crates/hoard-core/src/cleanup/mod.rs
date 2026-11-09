//! Cleanup: tray, validation, Trash / permanent removal, action log.
//!
//! **This is the most dangerous code in Hoard.** Every item passes, in order:
//!
//! 1. not covered by another item in the same run;
//! 2. static checks against the scan ([`static_checks`]): the node exists
//!    and was not already removed, is not the scan root, is not protected
//!    ([`protected::check`]), its contents were fully scanned, and its
//!    identity (device + inode) is known;
//! 3. disk re-validation ([`validate`]): `symlink_metadata` again, device,
//!    inode and kind must match the scan, and the canonicalized *parent*
//!    must still be inside the canonicalized scan root;
//! 4. removal: Trash (default) or permanent, never following links;
//! 5. tree update and one action-log line.
//!
//! A failure at any step skips that item and records why. Nothing is ever
//! retried with weaker checks or a weaker method. No lock is held while
//! files are being removed.

mod log;
pub mod protected;
mod remove;
mod tray;

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

pub use log::{ActionLog, LogEntry};
pub use protected::ProtectedList;
pub use remove::{Remover, SystemRemover};
pub use tray::{AddOutcome, Tray};

use crate::platform;
use crate::tree::{NodeFlags, NodeId, NodeKind, Tree};
use remove::RemoveError;

/// How items leave the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupMode {
    /// Move to Trash / Recycle Bin. The default.
    Trash,
    /// Delete permanently. Explicitly enabled per run.
    Permanent,
}

/// Permanent deletes above this many bytes need the typed total (PRD C3).
pub const CONFIRM_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// Permanent deletes above this many items need the typed total (PRD C3).
pub const CONFIRM_ITEMS: u64 = 1_000;

/// If a permanent delete of `total_bytes` across `items` files needs a typed
/// confirmation, returns the exact text the user must type (e.g. `42.1 GB`).
pub fn requires_typed_confirmation(
    mode: CleanupMode,
    total_bytes: u64,
    items: u64,
) -> Option<String> {
    (mode == CleanupMode::Permanent && (total_bytes > CONFIRM_BYTES || items > CONFIRM_ITEMS))
        .then(|| crate::format::bytes(total_bytes))
}

/// Where and against what a cleanup runs.
#[derive(Clone, Debug)]
pub struct CleanupContext {
    /// The scanned folder. Items must stay inside it.
    pub scan_root: PathBuf,
    /// The JSONL action log.
    pub log_path: PathBuf,
    /// Protected locations.
    pub protected: ProtectedList,
}

impl CleanupContext {
    /// System protections only (no per-user folders). Used by tests.
    pub fn new(scan_root: PathBuf, log_path: PathBuf) -> Self {
        CleanupContext {
            scan_root,
            log_path,
            protected: ProtectedList::system(),
        }
    }

    /// System protections plus the current user's home and standard folders
    /// (including redirected ones such as OneDrive Documents).
    pub fn for_current_user(scan_root: PathBuf, log_path: PathBuf) -> Self {
        let mut ctx = Self::new(scan_root, log_path);
        if let Some(home) = dirs::home_dir() {
            let user_dirs: Vec<PathBuf> = [
                dirs::desktop_dir(),
                dirs::document_dir(),
                dirs::download_dir(),
                dirs::picture_dir(),
                dirs::audio_dir(),
                dirs::video_dir(),
                dirs::public_dir(),
                dirs::config_dir(),
                dirs::data_local_dir(),
            ]
            .into_iter()
            .flatten()
            .collect();
            ctx.protected.add_user(&home, &user_dirs);
        }
        ctx
    }

    /// Adds `home` and its standard folders to the protected list.
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.protected.add_user(&home, &[]);
        self
    }

    /// Adds folders that are protected themselves (contents allowed).
    pub fn with_user_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        for d in dirs {
            self.protected
                .exact
                .push((d, "a top-level user folder".into()));
        }
        self
    }

    /// The default action log location: `<data dir>/Hoard/actions.jsonl`.
    pub fn default_log_path() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("Hoard").join("actions.jsonl"))
    }
}

/// Why an item was not removed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SkipReason {
    /// The id is not part of this scan.
    NotInTree,
    /// Already removed by an earlier run.
    AlreadyRemoved,
    /// The scanned folder itself is never removed.
    ScanRoot,
    /// On the protected list.
    Protected {
        /// Human-readable reason.
        why: String,
    },
    /// The folder's contents were not fully scanned (unreadable, other
    /// filesystem or skipped), so what would be deleted is unknown.
    ContentsUnknown,
    /// The scan could not record the item's identity, so it cannot be
    /// re-validated.
    Unverifiable,
    /// The item changed on disk since the scan.
    Changed {
        /// What changed.
        why: String,
    },
    /// The item is no longer inside the scanned folder.
    OutsideRoot,
    /// A folder containing it is also in this run.
    CoveredByOther,
    /// The run was cancelled before reaching it.
    Cancelled,
    /// The action log stopped accepting writes; the run halted.
    LogUnavailable,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::NotInTree => write!(f, "not part of the current scan"),
            SkipReason::AlreadyRemoved => write!(f, "already removed"),
            SkipReason::ScanRoot => write!(f, "the scanned folder itself cannot be removed"),
            SkipReason::Protected { why } => write!(f, "protected: {why}"),
            SkipReason::ContentsUnknown => write!(f, "its contents could not be fully scanned"),
            SkipReason::Unverifiable => {
                write!(f, "its identity could not be recorded during the scan")
            }
            SkipReason::Changed { why } => write!(f, "changed since the scan: {why}"),
            SkipReason::OutsideRoot => write!(f, "no longer inside the scanned folder"),
            SkipReason::CoveredByOther => write!(f, "inside another item in this cleanup"),
            SkipReason::Cancelled => write!(f, "cleanup was cancelled"),
            SkipReason::LogUnavailable => {
                write!(f, "the action log could not be written, so cleanup stopped")
            }
        }
    }
}

/// What happened to one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOutcome {
    /// Gone from its location (in the Trash, or deleted).
    Removed,
    /// Not attempted.
    Skipped(SkipReason),
    /// Attempted, nothing removed.
    Failed(String),
    /// Permanent folder removal removed some contents but not all.
    Partial(String),
}

impl ItemOutcome {
    /// `removed`, `skipped`, `failed` or `partial`.
    pub fn status(&self) -> &'static str {
        match self {
            ItemOutcome::Removed => "removed",
            ItemOutcome::Skipped(_) => "skipped",
            ItemOutcome::Failed(_) => "failed",
            ItemOutcome::Partial(_) => "partial",
        }
    }

    /// Human-readable reason, if not removed.
    pub fn reason(&self) -> Option<String> {
        match self {
            ItemOutcome::Removed => None,
            ItemOutcome::Skipped(r) => Some(r.to_string()),
            ItemOutcome::Failed(m) | ItemOutcome::Partial(m) => Some(m.clone()),
        }
    }
}

/// Per-item result.
#[derive(Clone, Debug)]
pub struct ItemResult {
    /// The tray item.
    pub id: NodeId,
    /// Its path (empty if the id was unknown).
    pub path: PathBuf,
    /// Its kind at scan time.
    pub kind: NodeKind,
    /// Estimated on-disk size at scan time.
    pub size_allocated: u64,
    /// Estimated logical size at scan time.
    pub size_apparent: u64,
    /// What happened.
    pub outcome: ItemOutcome,
}

/// The post-cleanup report (PRD C5).
#[derive(Clone, Debug)]
pub struct CleanupReport {
    /// Caller-supplied run id.
    pub run_id: String,
    /// Trash or permanent.
    pub mode: CleanupMode,
    /// One entry per requested item, in request order.
    pub results: Vec<ItemResult>,
    /// Sum of on-disk sizes of removed items (an estimate).
    pub estimated_freed: u64,
    /// Sum of logical sizes of removed items.
    pub estimated_freed_apparent: u64,
    /// Free bytes on the volume before the run, if measurable.
    pub free_before: Option<u64>,
    /// Free bytes on the volume after the run, if measurable.
    pub free_after: Option<u64>,
    /// True if the run was cancelled part way.
    pub cancelled: bool,
}

impl CleanupReport {
    /// Measured change in free space (after minus before). Trash moves do
    /// not free space until the Trash is emptied, so this can be ~0.
    pub fn measured_freed(&self) -> Option<i64> {
        let before = i64::try_from(self.free_before?).ok()?;
        let after = i64::try_from(self.free_after?).ok()?;
        Some(after - before)
    }

    /// Number of results with the given status.
    pub fn count(&self, status: &str) -> usize {
        self.results
            .iter()
            .filter(|r| r.outcome.status() == status)
            .count()
    }
}

/// Errors that stop a run before any item is touched.
#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    /// The action log could not be opened.
    #[error("cannot open the action log: {0}")]
    Log(#[source] io::Error),
    /// A large permanent delete was not confirmed with the exact total.
    #[error("type {expected} to confirm permanent deletion")]
    ConfirmationRequired {
        /// The text the user must type.
        expected: String,
    },
    /// The tree lock was poisoned by a panic elsewhere.
    #[error("internal tree lock poisoned")]
    Lock,
}

/// Progress callback payload.
#[derive(Clone, Debug)]
pub struct CleanupProgress {
    /// Items finished so far.
    pub done: usize,
    /// Items in the run.
    pub total: usize,
    /// The item being processed.
    pub current: PathBuf,
}

/// An item that passed every check and may be removed now.
#[derive(Clone, Debug)]
pub struct ValidatedItem {
    /// Tree id.
    pub id: NodeId,
    /// Full path.
    pub path: PathBuf,
    /// Kind (verified on disk).
    pub kind: NodeKind,
    /// Device id (verified on disk).
    pub dev: u64,
    /// Inode / file index (verified on disk).
    pub ino: u64,
}

/// Checks that need only the tree (plus the protected-list path
/// resolution). Used on tray add and again at execution.
pub(crate) fn static_checks(
    tree: &Tree,
    id: NodeId,
    ctx: &CleanupContext,
) -> Result<PathBuf, SkipReason> {
    let node = tree.get(id).ok_or(SkipReason::NotInTree)?;
    if node.flags.contains(NodeFlags::REMOVED) {
        return Err(SkipReason::AlreadyRemoved);
    }
    if id == tree.root() {
        return Err(SkipReason::ScanRoot);
    }
    let path = tree.path(id).ok_or(SkipReason::NotInTree)?;
    if let Some(why) = protected::check(&path, ctx) {
        return Err(SkipReason::Protected { why });
    }
    if let Some(why) = protected::check_tree(tree, id) {
        return Err(SkipReason::Protected { why });
    }
    if node.is_dir()
        && tree.subtree_has_flag(
            id,
            NodeFlags::ERROR | NodeFlags::OTHER_FS | NodeFlags::SKIPPED,
        )
    {
        return Err(SkipReason::ContentsUnknown);
    }
    if node.flags.contains(NodeFlags::ERROR) || (node.dev == 0 && node.ino == 0) {
        return Err(SkipReason::Unverifiable);
    }
    Ok(path)
}

/// Full validation: [`static_checks`] plus re-reading the item from disk.
pub fn validate(
    tree: &Tree,
    id: NodeId,
    ctx: &CleanupContext,
) -> Result<ValidatedItem, SkipReason> {
    let path = static_checks(tree, id, ctx)?;
    let node = tree.get(id).ok_or(SkipReason::NotInTree)?;

    let now = platform::stat(&path).map_err(|e| SkipReason::Changed {
        why: if e.kind() == io::ErrorKind::NotFound {
            "it no longer exists".into()
        } else {
            format!("it can no longer be read ({e})")
        },
    })?;
    if now.dev != node.dev || now.ino != node.ino {
        return Err(SkipReason::Changed {
            why: "a different item now exists at this path".into(),
        });
    }
    if now.kind != node.kind {
        return Err(SkipReason::Changed {
            why: "its type changed".into(),
        });
    }

    // The parent (not the item, which may be a link) must still resolve to
    // a location inside the scan root.
    let parent = path.parent().ok_or(SkipReason::OutsideRoot)?;
    let parent = std::fs::canonicalize(parent).map_err(|e| SkipReason::Changed {
        why: format!("its folder can no longer be resolved ({e})"),
    })?;
    let root = std::fs::canonicalize(&ctx.scan_root).map_err(|e| SkipReason::Changed {
        why: format!("the scanned folder can no longer be resolved ({e})"),
    })?;
    if !parent.starts_with(&root) {
        return Err(SkipReason::OutsideRoot);
    }

    Ok(ValidatedItem {
        id,
        path,
        kind: node.kind,
        dev: node.dev,
        ino: node.ino,
    })
}

fn execute(item: &ValidatedItem, mode: CleanupMode, remover: &dyn Remover) -> ItemOutcome {
    match mode {
        CleanupMode::Trash => {
            if let Err(e) = remover.trash(&item.path) {
                return ItemOutcome::Failed(format!("could not move to the Trash: {e}"));
            }
            // Trust the filesystem, not the API's return value.
            match platform::stat(&item.path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => ItemOutcome::Removed,
                Ok(_) => ItemOutcome::Failed("still present after moving to the Trash".into()),
                Err(e) => ItemOutcome::Failed(format!("could not confirm removal: {e}")),
            }
        }
        CleanupMode::Permanent => match remove::remove_permanently(&item.path, item.kind, item.dev)
        {
            Ok(()) => ItemOutcome::Removed,
            Err(RemoveError::Failed(m)) => ItemOutcome::Failed(m),
            Err(RemoveError::Partial(m)) => ItemOutcome::Partial(m),
        },
    }
}

/// Coverage and totals for a set of items, computed exactly as a run will.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPlan {
    /// Per item: true if a folder containing it is also in the set (and
    /// passes the static checks, so it will actually be attempted).
    pub covered: Vec<bool>,
    /// On-disk bytes of the items that will be attempted.
    pub total_bytes: u64,
    /// Files in the items that will be attempted (folders count their files,
    /// an empty folder counts 1).
    pub total_items: u64,
}

/// Plans a run over `ids`. Only items that pass the static checks can cover
/// others: a protected folder in the set must not hide the items inside it.
pub fn plan(tree: &Tree, ids: &[NodeId], ctx: &CleanupContext) -> RunPlan {
    let eligible: Vec<bool> = ids
        .iter()
        .map(|&id| static_checks(tree, id, ctx).is_ok())
        .collect();
    let covered: Vec<bool> = ids
        .iter()
        .map(|&id| {
            ids.iter()
                .zip(&eligible)
                .any(|(&other, &ok)| ok && other != id && tree.is_ancestor_or_self(other, id))
        })
        .collect();
    let mut total_bytes = 0u64;
    let mut total_items = 0u64;
    for (&id, &c) in ids.iter().zip(&covered) {
        if let (false, Some(n)) = (c, tree.get(id)) {
            total_bytes = total_bytes.saturating_add(n.size_allocated);
            total_items = total_items.saturating_add(u64::from(n.file_count.max(1)));
        }
    }
    RunPlan {
        covered,
        total_bytes,
        total_items,
    }
}

/// The text a user must type to run a permanent delete of `ids`, if any.
/// Uses the same [`plan`] as [`run_cleanup`], so UI and engine agree.
pub fn required_confirmation(
    tree: &Tree,
    ids: &[NodeId],
    mode: CleanupMode,
    ctx: &CleanupContext,
) -> Option<String> {
    let p = plan(tree, ids, ctx);
    requires_typed_confirmation(mode, p.total_bytes, p.total_items)
}

/// Runs the cleanup pipeline over `ids` (see the module docs).
///
/// - Returns `Err` only if the run cannot start (log unavailable, missing
///   confirmation); then nothing has been touched.
/// - Otherwise returns a report with one result per id, in order.
/// - `confirmation` must equal [`requires_typed_confirmation`]'s text when
///   that returns `Some`.
/// - `cancel` is checked between items.
#[allow(clippy::too_many_arguments)]
pub fn run_cleanup(
    tree: &RwLock<Tree>,
    ids: &[NodeId],
    mode: CleanupMode,
    confirmation: Option<&str>,
    ctx: &CleanupContext,
    remover: &dyn Remover,
    run_id: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(CleanupProgress),
) -> Result<CleanupReport, CleanupError> {
    // Plan under a short read lock: coverage, sizes, confirmation.
    let RunPlan {
        covered,
        total_bytes,
        total_items,
    } = {
        let t = tree.read().map_err(|_| CleanupError::Lock)?;
        plan(&t, ids, ctx)
    };
    if let Some(expected) = requires_typed_confirmation(mode, total_bytes, total_items) {
        if confirmation.map(str::trim) != Some(expected.as_str()) {
            return Err(CleanupError::ConfirmationRequired { expected });
        }
    }

    let mut log = ActionLog::open(&ctx.log_path).map_err(CleanupError::Log)?;
    let free_before = crate::disks::available_space_for(&ctx.scan_root);

    let mut results = Vec::with_capacity(ids.len());
    let mut cancelled = false;
    let mut log_failed = false;

    for (i, (&id, &is_covered)) in ids.iter().zip(&covered).enumerate() {
        // Snapshot what the scan knew about the item (short read lock).
        let (path, kind, size_allocated, size_apparent) = {
            let t = tree.read().map_err(|_| CleanupError::Lock)?;
            match t.get(id) {
                Some(n) => (
                    t.path(id).unwrap_or_default(),
                    n.kind,
                    n.size_allocated,
                    n.size_apparent,
                ),
                None => (PathBuf::new(), NodeKind::Other, 0, 0),
            }
        };
        progress(CleanupProgress {
            done: i,
            total: ids.len(),
            current: path.clone(),
        });

        let outcome = if log_failed {
            ItemOutcome::Skipped(SkipReason::LogUnavailable)
        } else if cancelled || cancel.load(Ordering::Relaxed) {
            cancelled = true;
            ItemOutcome::Skipped(SkipReason::Cancelled)
        } else if is_covered {
            ItemOutcome::Skipped(SkipReason::CoveredByOther)
        } else {
            let validated = {
                let t = tree.read().map_err(|_| CleanupError::Lock)?;
                validate(&t, id, ctx)
            };
            match validated {
                Err(reason) => ItemOutcome::Skipped(reason),
                // No lock held while touching the disk.
                Ok(item) => execute(&item, mode, remover),
            }
        };

        // Reflect the result in the tree (short write lock).
        match &outcome {
            ItemOutcome::Removed => {
                let mut t = tree.write().map_err(|_| CleanupError::Lock)?;
                t.remove_subtree(id);
            }
            ItemOutcome::Partial(_) => {
                let mut t = tree.write().map_err(|_| CleanupError::Lock)?;
                t.set_flag(id, NodeFlags::STALE);
            }
            _ => {}
        }

        if !log_failed {
            let entry = LogEntry {
                ts_ms: log::now_ms(),
                run_id,
                mode,
                path: path.to_string_lossy().into_owned(),
                kind,
                size_allocated,
                size_apparent,
                result: outcome.status(),
                reason: outcome.reason(),
            };
            if let Err(e) = log.append(&entry) {
                tracing::error!("action log write failed, stopping cleanup: {e}");
                log_failed = true;
            }
        }

        results.push(ItemResult {
            id,
            path,
            kind,
            size_allocated,
            size_apparent,
            outcome,
        });
    }

    let removed = results.iter().filter(|r| r.outcome == ItemOutcome::Removed);
    let (estimated_freed, estimated_freed_apparent) = removed.fold((0u64, 0u64), |(a, b), r| {
        (
            a.saturating_add(r.size_allocated),
            b.saturating_add(r.size_apparent),
        )
    });

    Ok(CleanupReport {
        run_id: run_id.to_string(),
        mode,
        results,
        estimated_freed,
        estimated_freed_apparent,
        free_before,
        free_after: crate::disks::available_space_for(&ctx.scan_root),
        cancelled,
    })
}
