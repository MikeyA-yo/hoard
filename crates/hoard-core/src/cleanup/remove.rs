//! The two ways an item leaves the disk: Trash or permanent removal.
//!
//! Permanent removal of a directory walks it manually: every entry is
//! stat'ed without following links, links are unlinked (never followed),
//! and the walk refuses to descend into another filesystem (a mount point).

use std::fs;
use std::io;
use std::path::Path;

use crate::platform;
use crate::tree::NodeKind;

/// Moves items to the OS Trash / Recycle Bin. Abstracted so the pipeline
/// can be tested without touching the real Trash.
pub trait Remover: Sync {
    /// Moves `path` (never its link target) to the Trash.
    fn trash(&self, path: &Path) -> Result<(), String>;
}

/// The real Trash / Recycle Bin via the `trash` crate.
///
/// The crate canonicalizes only the parent folder, so a link is trashed as
/// a link. On Windows it uses `IFileOperation` with undo enabled; an item
/// that cannot be recycled is refused and reported, not destroyed.
pub struct SystemRemover;

impl Remover for SystemRemover {
    fn trash(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// Result of a permanent removal that did not fully succeed.
#[derive(Debug)]
pub(crate) enum RemoveError {
    /// Nothing was removed.
    Failed(String),
    /// Some contents were removed, but the item still exists.
    Partial(String),
}

/// Removes `path` permanently. `kind` and `dev` are the values verified
/// against the scan just before this call.
pub(crate) fn remove_permanently(path: &Path, kind: NodeKind, dev: u64) -> Result<(), RemoveError> {
    match kind {
        NodeKind::Dir => {
            let mut errors = Vec::new();
            let mut removed_any = false;
            remove_tree(path, dev, &mut errors, &mut removed_any);
            if errors.is_empty() {
                Ok(())
            } else {
                let mut msg = errors[..errors.len().min(3)].join("; ");
                if errors.len() > 3 {
                    msg.push_str(&format!(" (and {} more)", errors.len() - 3));
                }
                if removed_any {
                    Err(RemoveError::Partial(msg))
                } else {
                    Err(RemoveError::Failed(msg))
                }
            }
        }
        NodeKind::Symlink => remove_link(path).map_err(|e| RemoveError::Failed(e.to_string())),
        NodeKind::File | NodeKind::Other => {
            fs::remove_file(path).map_err(|e| RemoveError::Failed(e.to_string()))
        }
    }
}

/// Post-order removal. Errors are collected and the walk continues so as
/// much as possible is removed, but nothing outside `dir` is ever touched.
fn remove_tree(dir: &Path, dev: u64, errors: &mut Vec<String>, removed_any: &mut bool) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(e) => e.path(),
            Err(e) => {
                errors.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        let meta = match platform::stat(&path) {
            Ok(m) => m,
            Err(e) => {
                errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let result = match meta.kind {
            NodeKind::Dir if meta.dev != dev => {
                errors.push(format!(
                    "{}: is a different drive or mount point; left in place",
                    path.display()
                ));
                continue;
            }
            NodeKind::Dir => {
                remove_tree(&path, dev, errors, removed_any);
                continue;
            }
            NodeKind::Symlink => remove_link(&path),
            NodeKind::File | NodeKind::Other => fs::remove_file(&path),
        };
        match result {
            Ok(()) => *removed_any = true,
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    match fs::remove_dir(dir) {
        Ok(()) => *removed_any = true,
        // Leftover contents already produced an error above.
        Err(e) if errors.is_empty() => errors.push(format!("{}: {e}", dir.display())),
        Err(_) => {}
    }
}

/// Removes a symlink or junction itself, never its target.
fn remove_link(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        // Directory links and junctions are removed with RemoveDirectory,
        // which deletes the reparse point, not the target's contents.
        if fs::symlink_metadata(path)?.file_type().is_symlink_dir() {
            return fs::remove_dir(path);
        }
        fs::remove_file(path)
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(path)
    }
}
