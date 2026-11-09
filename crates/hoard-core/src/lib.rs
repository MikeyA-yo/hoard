//! `hoard-core`: disk usage scanning, accounting and (later) cleanup.
//!
//! No UI and no Tauri dependency. The CLI and desktop app are thin layers
//! over this crate.
//!
//! ```no_run
//! use std::sync::{atomic::AtomicBool, Arc};
//! use hoard_core::{scan, ScanOptions, SizeMode};
//!
//! let (tx, _rx) = crossbeam_channel::unbounded();
//! let tree = scan(std::path::Path::new("."), ScanOptions::default(), tx, Arc::new(AtomicBool::new(false)))?;
//! println!("{}", hoard_core::format::bytes(tree.total(SizeMode::Allocated)));
//! # Ok::<(), hoard_core::HoardError>(())
//! ```

#![warn(missing_docs)]

pub mod classify;
pub mod cleanup;
pub mod disks;
pub mod error;
pub mod format;
pub mod lenses;
pub mod platform;
pub mod rules;
pub mod scan;
pub mod size;
pub mod tree;
pub mod view;

pub use error::{HoardError, Result};
pub use scan::{scan, ScanError, ScanErrorKind, ScanEvent, ScanOptions, TopLevelEntry};
pub use size::SizeMode;
pub use tree::{LargestKind, Node, NodeFlags, NodeId, NodeKind, SortKey, Tree};
