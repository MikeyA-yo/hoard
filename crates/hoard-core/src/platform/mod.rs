//! Platform specific filesystem access.
//!
//! Everything outside this module calls the common signatures defined here.
//! Per-OS code lives in `unix.rs`, `windows.rs`, `linux.rs` and `macos.rs`.

use std::io;
use std::path::{Path, PathBuf};

use crate::tree::NodeKind;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

/// Metadata for one filesystem entry, read without following symlinks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryMeta {
    /// What kind of entry this is. A symlink (or Windows junction) is always
    /// reported as [`NodeKind::Symlink`], never as its target.
    pub kind: NodeKind,
    /// Logical size in bytes. Always 0 for directories.
    pub size_apparent: u64,
    /// On-disk size in bytes (see [`crate::size`]).
    pub size_allocated: u64,
    /// Last modification time, unix seconds.
    pub mtime: i64,
    /// Device id (Unix `st_dev`, Windows volume serial number).
    pub dev: u64,
    /// Inode (Unix `st_ino`, Windows file index).
    pub ino: u64,
    /// Number of hard links to this entry.
    pub nlink: u64,
}

/// Stats `path` without following symlinks.
///
/// This is the `symlink_metadata` contract: if `path` is a symlink (or a
/// Windows junction / other name-surrogate reparse point), the link itself is
/// described, never its target.
pub fn stat(path: &Path) -> io::Result<EntryMeta> {
    imp::stat(path)
}

/// Size of the Recycle Bin / Trash: `(bytes, items)`, or `None` where it
/// cannot be measured cheaply. Read-only; Hoard never empties it.
pub fn trash_size() -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        windows::recycle_bin_size()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Absolute paths of pseudo / virtual filesystems that are never scanned
/// (`/proc`, `/sys`, `/dev`, ...). They are dropped from the tree entirely.
pub fn pseudo_fs_paths() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        linux::PSEUDO_FS
    }
    #[cfg(target_os = "macos")]
    {
        macos::PSEUDO_FS
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        &["/proc", "/dev"]
    }
    #[cfg(windows)]
    {
        &[]
    }
}

/// Extra device ids that count as "the same filesystem" as the scan root.
///
/// On macOS the read-only system volume and the data volume are presented as
/// one tree via firmlinks, so scanning `/` must be allowed into the data
/// volume. Elsewhere this is empty.
pub fn same_filesystem_devs(root: &Path) -> Vec<u64> {
    #[cfg(target_os = "macos")]
    {
        macos::same_filesystem_devs(root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root;
        Vec::new()
    }
}

/// Paths to skip for a given scan root, beyond [`pseudo_fs_paths`]. Used on
/// macOS to avoid counting the data volume twice when scanning `/`.
pub fn extra_skips(root: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos::extra_skips(root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root;
        Vec::new()
    }
}
