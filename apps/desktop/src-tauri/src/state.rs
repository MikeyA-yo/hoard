//! App state: the current tree and the running scan.
//!
//! Locks are held only long enough to clone an `Arc` or swap a field, never
//! across a scan, a query or dropping a large tree.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use hoard_core::Tree;

/// The finished (or cancelled, partial) result of the latest scan.
pub struct ScanResult {
    pub tree: Arc<Tree>,
    pub elapsed_ms: u64,
}

struct ActiveScan {
    id: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    result: Option<Arc<ScanResult>>,
    active: Option<ActiveScan>,
    next_id: u64,
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

    /// Registers a new scan, cancelling any running one. Returns its id and
    /// cancel flag.
    pub fn begin_scan(&self) -> Result<(u64, Arc<AtomicBool>), String> {
        let mut inner = self.lock()?;
        if let Some(prev) = inner.active.take() {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        inner.next_id += 1;
        let id = inner.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        inner.active = Some(ActiveScan {
            id,
            cancel: Arc::clone(&cancel),
        });
        Ok((id, cancel))
    }

    /// Sets the cancel flag if `id` is the running scan. Returns whether it was.
    pub fn cancel_scan(&self, id: u64) -> Result<bool, String> {
        let inner = self.lock()?;
        match &inner.active {
            Some(a) if a.id == id => {
                a.cancel.store(true, Ordering::Relaxed);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Stores a finished scan if it is still the active one. A scan that was
    /// superseded by a newer `start_scan` is discarded. Returns whether the
    /// result was stored.
    pub fn finish_scan(&self, id: u64, result: Option<ScanResult>) -> bool {
        let old = {
            let Ok(mut inner) = self.lock() else {
                return false;
            };
            if inner.active.as_ref().map(|a| a.id) != Some(id) {
                return false;
            }
            inner.active = None;
            match result {
                Some(r) => inner.result.replace(Arc::new(r)),
                None => None,
            }
        };
        // Drop the previous tree (possibly millions of nodes) outside the lock.
        drop(old);
        true
    }
}
