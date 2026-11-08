//! Progress events and throttling.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::tree::{NodeId, NodeKind};

/// Minimum interval between [`ScanEvent::Progress`] events.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Minimum interval between [`ScanEvent::Batch`] events.
pub const BATCH_INTERVAL: Duration = Duration::from_millis(500);
/// Maximum entries in one batch (largest first), so a huge directory never
/// floods the channel or the IPC bridge.
pub const BATCH_MAX_ENTRIES: usize = 500;

/// Events sent while a scan runs. Sending never blocks the scan: if the
/// channel is full or disconnected the event is dropped.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScanEvent {
    /// Running counters, at most every [`PROGRESS_INTERVAL`], plus one final
    /// event when the walk ends.
    Progress {
        /// Non-directory entries seen so far.
        files: u64,
        /// Directories seen so far.
        dirs: u64,
        /// Bytes counted so far, in the scan's size mode.
        bytes: u64,
        /// The entry most recently processed.
        current_path: PathBuf,
        /// Errors recorded so far.
        errors: u64,
        /// Time since the scan started.
        elapsed_ms: u64,
    },
    /// Partial results: running totals of the root's direct children, so a UI
    /// can draw the top level of the treemap while the scan continues.
    Batch {
        /// Largest first, at most [`BATCH_MAX_ENTRIES`].
        top_level: Vec<TopLevelEntry>,
    },
}

/// Running totals for one direct child of the scan root.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TopLevelEntry {
    /// Node id in the tree being built (stable once the scan finishes).
    pub id: NodeId,
    /// File name (lossy UTF-8).
    pub name: String,
    /// Entry kind.
    pub kind: NodeKind,
    /// On-disk bytes counted so far.
    pub size_allocated: u64,
    /// Logical bytes counted so far.
    pub size_apparent: u64,
    /// Files counted so far.
    pub file_count: u64,
}

/// Fires at most once per interval.
pub(crate) struct Throttle {
    every: Duration,
    last: Instant,
}

impl Throttle {
    pub(crate) fn new(every: Duration) -> Self {
        Throttle {
            every,
            last: Instant::now(),
        }
    }

    /// True (and resets) if at least one interval passed since the last fire.
    pub(crate) fn ready(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last) >= self.every {
            self.last = now;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_fires_once_per_interval() {
        let mut t = Throttle::new(Duration::from_millis(100));
        let start = t.last;
        assert!(!t.ready(start + Duration::from_millis(50)));
        assert!(t.ready(start + Duration::from_millis(100)));
        assert!(!t.ready(start + Duration::from_millis(150)));
        assert!(t.ready(start + Duration::from_millis(210)));
    }
}
