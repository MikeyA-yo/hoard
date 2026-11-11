//! Unix (Linux, macOS, BSD) stat.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::EntryMeta;
use crate::size;
use crate::tree::NodeKind;

/// `lstat` the path. Never follows symlinks.
pub(super) fn stat(path: &Path) -> io::Result<EntryMeta> {
    let md = fs::symlink_metadata(path)?;
    let ft = md.file_type();
    let kind = if ft.is_symlink() {
        NodeKind::Symlink
    } else if ft.is_dir() {
        NodeKind::Dir
    } else if ft.is_file() {
        NodeKind::File
    } else {
        NodeKind::Other
    };
    Ok(EntryMeta {
        kind,
        size_apparent: if kind == NodeKind::Dir { 0 } else { md.len() },
        size_allocated: size::allocated_from_blocks(md.blocks()),
        mtime: md.mtime(),
        dev: md.dev(),
        ino: md.ino(),
        nlink: md.nlink(),
    })
}

/// Per-directory stat state. On Unix every entry is `lstat`ed by full path.
pub(super) struct DirStat;

impl DirStat {
    pub(super) fn open(_dir: &Path) -> Self {
        DirStat
    }

    pub(super) fn stat(&self, _name: &std::ffi::OsStr, path: &Path) -> io::Result<EntryMeta> {
        stat(path)
    }

    pub(super) fn own_allocated(&self) -> Option<u64> {
        None
    }

    pub(super) fn is_fast(&self) -> bool {
        false
    }
}
