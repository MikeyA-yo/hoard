//! macOS specifics.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Pseudo filesystems and automount points. Never scanned.
pub(super) const PSEUDO_FS: &[&str] = &["/dev", "/.vol", "/net"];

const DATA_VOLUME: &str = "/System/Volumes/Data";

/// When scanning `/`, the data volume is reached through firmlinks
/// (`/Users`, `/Applications`, ...) and has its own device id, so it must be
/// treated as the same filesystem as the root.
pub(super) fn same_filesystem_devs(root: &Path) -> Vec<u64> {
    if root != Path::new("/") {
        return Vec::new();
    }
    std::fs::symlink_metadata(DATA_VOLUME)
        .map(|md| vec![md.dev()])
        .unwrap_or_default()
}

/// When scanning `/`, skip the raw data volume mount: its contents are already
/// counted through the firmlinks, and walking it too would double count.
pub(super) fn extra_skips(root: &Path) -> Vec<PathBuf> {
    if root == Path::new("/") {
        vec![PathBuf::from(DATA_VOLUME)]
    } else {
        Vec::new()
    }
}
