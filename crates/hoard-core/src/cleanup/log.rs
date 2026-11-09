//! Append-only JSONL action log (PRD C6).
//!
//! One line per tray item processed, written after the item is handled.
//! A run refuses to start if the log cannot be opened, so no deletion can
//! happen without a record.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::CleanupMode;
use crate::tree::NodeKind;

/// One log line.
#[derive(Debug, Serialize)]
pub struct LogEntry<'a> {
    /// Milliseconds since the Unix epoch.
    pub ts_ms: u64,
    /// The cleanup run this item belongs to.
    pub run_id: &'a str,
    /// Trash or permanent.
    pub mode: CleanupMode,
    /// Full path of the item.
    pub path: String,
    /// What the item was at scan time.
    pub kind: NodeKind,
    /// Estimated on-disk size at scan time.
    pub size_allocated: u64,
    /// Estimated logical size at scan time.
    pub size_apparent: u64,
    /// `removed`, `skipped`, `failed` or `partial`.
    pub result: &'static str,
    /// Why it was skipped or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// An open action log.
pub struct ActionLog {
    file: File,
}

impl ActionLog {
    /// Opens (creating if needed) the log for appending.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(ActionLog { file })
    }

    /// Appends one entry and flushes it to disk.
    pub fn append(&mut self, entry: &LogEntry<'_>) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()
    }
}

/// Current time in milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
