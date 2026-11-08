//! Error types for `hoard-core`.

use std::io;
use std::path::PathBuf;

/// Errors returned by `hoard-core` operations.
///
/// Errors below the scan root never appear here; they are recorded as
/// [`crate::scan::ScanError`]s on the tree and the walk continues.
#[derive(Debug, thiserror::Error)]
pub enum HoardError {
    /// The scan root could not be resolved or stat'ed.
    #[error("cannot read scan root {}: {source}", path.display())]
    Root {
        /// The root as given.
        path: PathBuf,
        /// The underlying IO error.
        #[source]
        source: io::Error,
    },
}

/// Result alias using [`HoardError`].
pub type Result<T> = std::result::Result<T, HoardError>;
