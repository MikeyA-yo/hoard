//! App state: the current scan result, the cleanup tray, and what the app
//! is busy with.
//!
//! Concurrency rules (to keep races out by construction):
//! - One state machine, [`Busy`]: idle, scanning or cleaning. A cleanup
//!   cannot start during a scan and a scan cannot start during a cleanup.
//! - The tray belongs to one scan (`generation`); a new scan replaces it.
//! - Locks are short: the `Mutex` is held only to read or swap fields, the
//!   tree `RwLock` only for a query or a per-item update, never across a
//!   scan, a whole cleanup run or a file deletion. Lock order is always
//!   state mutex first, then tree lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard};

use hoard_core::cleanup::{AddOutcome, CleanupContext, Tray};
use hoard_core::{NodeId, Tree};

/// The result of a finished (or cancelled, partial) scan.
pub struct ScanResult {
    /// Scan id that produced this tree; the tray generation.
    pub generation: u64,
    /// The tree, updated in place by cleanup runs.
    pub tree: RwLock<Tree>,
    pub elapsed_ms: u64,
}

impl ScanResult {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, Tree>, String> {
        self.tree
            .read()
            .map_err(|_| "internal tree lock poisoned".to_string())
    }
}

enum Busy {
    Idle,
    Scanning {
        id: u64,
        cancel: Arc<AtomicBool>,
    },
    Cleaning {
        run_id: String,
        cancel: Arc<AtomicBool>,
    },
}

struct Inner {
    result: Option<Arc<ScanResult>>,
    tray: Tray,
    busy: Busy,
    next_scan_id: u64,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            result: None,
            tray: Tray::new(0),
            busy: Busy::Idle,
            next_scan_id: 0,
        }
    }
}

/// Everything a cleanup run needs, captured atomically when it starts.
pub struct CleanupStart {
    pub result: Arc<ScanResult>,
    pub items: Vec<NodeId>,
    pub cancel: Arc<AtomicBool>,
}

/// Shared state managed by Tauri.
#[derive(Default)]
pub struct AppState {
    inner: Mutex<Inner>,
}

impl AppState {
    fn lock(&self) -> Result<MutexGuard<'_, Inner>, String> {
        self.inner
            .lock()
            .map_err(|_| "internal state lock poisoned".to_string())
    }

    /// The current scan result, or an error if nothing has been scanned yet.
    pub fn result(&self) -> Result<Arc<ScanResult>, String> {
        self.lock()?
            .result
            .clone()
            .ok_or_else(|| "no scan loaded".to_string())
    }

    /// Registers a new scan, cancelling any running scan. Refused while a
    /// cleanup is running.
    pub fn begin_scan(&self) -> Result<(u64, Arc<AtomicBool>), String> {
        let mut inner = self.lock()?;
        match &inner.busy {
            Busy::Cleaning { .. } => {
                return Err("a cleanup is running; wait for it to finish".into())
            }
            Busy::Scanning { cancel, .. } => cancel.store(true, Ordering::Relaxed),
            Busy::Idle => {}
        }
        inner.next_scan_id += 1;
        let id = inner.next_scan_id;
        let cancel = Arc::new(AtomicBool::new(false));
        inner.busy = Busy::Scanning {
            id,
            cancel: Arc::clone(&cancel),
        };
        Ok((id, cancel))
    }

    /// Sets the cancel flag if `id` is the running scan. Returns whether it was.
    pub fn cancel_scan(&self, id: u64) -> Result<bool, String> {
        let inner = self.lock()?;
        match &inner.busy {
            Busy::Scanning { id: active, cancel } if *active == id => {
                cancel.store(true, Ordering::Relaxed);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Stores a finished scan if it is still the active one, and starts a
    /// fresh tray for it. A superseded scan is discarded. Returns whether
    /// the result was stored.
    pub fn finish_scan(&self, id: u64, tree: Option<Tree>, elapsed_ms: u64) -> bool {
        let old = {
            let Ok(mut inner) = self.lock() else {
                return false;
            };
            if !matches!(inner.busy, Busy::Scanning { id: active, .. } if active == id) {
                return false;
            }
            inner.busy = Busy::Idle;
            match tree {
                Some(tree) => {
                    inner.tray = Tray::new(id);
                    inner.result.replace(Arc::new(ScanResult {
                        generation: id,
                        tree: RwLock::new(tree),
                        elapsed_ms,
                    }))
                }
                None => None,
            }
        };
        // Drop the previous tree (possibly millions of nodes) outside the lock.
        drop(old);
        true
    }

    /// Runs `f` on the tray with the current tree, unless a cleanup is
    /// running, and returns its output. The tray change and the response
    /// built from it happen under the same lock, so they always agree.
    pub fn with_tray<T>(
        &self,
        f: impl FnOnce(&mut Tray, &Tree, &CleanupContext) -> T,
        ctx_for: impl FnOnce(&Tree) -> CleanupContext,
    ) -> Result<T, String> {
        let mut inner = self.lock()?;
        if matches!(inner.busy, Busy::Cleaning { .. }) {
            return Err("a cleanup is running; the tray is locked until it finishes".into());
        }
        let result = inner.result.clone().ok_or("no scan loaded")?;
        let tree = result.read()?;
        let ctx = ctx_for(&tree);
        Ok(f(&mut inner.tray, &tree, &ctx))
    }

    /// Snapshot of the tray and the tree it belongs to (no mutation).
    pub fn tray(&self) -> Result<(Tray, Option<Arc<ScanResult>>), String> {
        let inner = self.lock()?;
        Ok((inner.tray.clone(), inner.result.clone()))
    }

    /// Starts a cleanup of the current tray. Refused unless idle, or if the
    /// tray is empty or belongs to another scan.
    pub fn begin_cleanup(&self, run_id: &str) -> Result<CleanupStart, String> {
        let mut inner = self.lock()?;
        match inner.busy {
            Busy::Idle => {}
            Busy::Scanning { .. } => return Err("a scan is running; wait for it to finish".into()),
            Busy::Cleaning { .. } => return Err("a cleanup is already running".into()),
        }
        let result = inner.result.clone().ok_or("no scan loaded")?;
        if inner.tray.generation() != result.generation {
            return Err("the tray belongs to an older scan".into());
        }
        let items = inner.tray.items().to_vec();
        if items.is_empty() {
            return Err("the tray is empty".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        inner.busy = Busy::Cleaning {
            run_id: run_id.to_string(),
            cancel: Arc::clone(&cancel),
        };
        Ok(CleanupStart {
            result,
            items,
            cancel,
        })
    }

    /// Requests cancellation of a running cleanup (takes effect between items).
    pub fn cancel_cleanup(&self, run_id: &str) -> Result<bool, String> {
        let inner = self.lock()?;
        match &inner.busy {
            Busy::Cleaning {
                run_id: active,
                cancel,
            } if active == run_id => {
                cancel.store(true, Ordering::Relaxed);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Marks the cleanup finished and drops removed items from the tray.
    pub fn finish_cleanup(&self, run_id: &str) {
        let Ok(mut inner) = self.lock() else { return };
        if matches!(&inner.busy, Busy::Cleaning { run_id: active, .. } if active == run_id) {
            inner.busy = Busy::Idle;
        }
        if let Some(result) = inner.result.clone() {
            if let Ok(tree) = result.tree.read() {
                inner.tray.prune_removed(&tree);
            }
        }
    }
}

/// Short human notes describing a tray add, for the UI.
pub fn describe_add(out: &AddOutcome, tree: &Tree) -> Vec<String> {
    let name = |id: NodeId| {
        tree.get(id)
            .map_or_else(|| "item".to_string(), |n| n.display_name())
    };
    let mut notes = Vec::new();
    if !out.replaced.is_empty() {
        notes.push(format!(
            "{} item(s) already in the tray are inside a folder you added, so the folder replaces them",
            out.replaced.len()
        ));
    }
    for id in &out.covered {
        notes.push(format!(
            "{} is already in the tray (directly or inside a folder)",
            name(*id)
        ));
    }
    for (id, reason) in &out.rejected {
        notes.push(format!("{} cannot be cleaned: {reason}", name(*id)));
    }
    notes
}
