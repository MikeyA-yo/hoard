//! Test fixture builder: creates file trees inside a `TempDir`.
//!
//! Nothing here touches paths outside temp dirs.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use hoard_core::{scan, NodeId, ScanEvent, ScanOptions, Tree};
use tempfile::TempDir;

/// A temporary directory tree.
pub struct Fixture {
    dir: TempDir,
}

/// How a link was created. Windows needs a privilege for real symlinks, so
/// directory links fall back to junctions there (also never followed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Symlink,
    Junction,
}

impl Fixture {
    pub fn new() -> Self {
        Fixture {
            dir: tempfile::Builder::new()
                .prefix("hoard-test-")
                .tempdir()
                .expect("create temp dir"),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root()
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    /// Creates a file of exactly `size` bytes (non-zero content so it is not sparse).
    pub fn file(&self, rel: &str, size: usize) -> PathBuf {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        let mut f = fs::File::create(&p).expect("create file");
        let chunk = vec![0xA5u8; 64 * 1024];
        let mut left = size;
        while left > 0 {
            let n = left.min(chunk.len());
            f.write_all(&chunk[..n]).expect("write");
            left -= n;
        }
        f.sync_all().expect("sync");
        p
    }

    pub fn dir(&self, rel: &str) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(&p).expect("create dir");
        p
    }

    pub fn hardlink(&self, existing: &str, new: &str) -> PathBuf {
        let p = self.path(new);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::hard_link(self.path(existing), &p).expect("hard link");
        p
    }

    /// Creates a link at `rel` pointing to the directory `target`.
    pub fn dir_link(&self, target: &Path, rel: &str) -> LinkKind {
        let link = self.path(rel);
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, &link).expect("symlink");
            LinkKind::Symlink
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, &link).is_ok() {
                return LinkKind::Symlink;
            }
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .status()
                .expect("run mklink");
            assert!(status.success(), "mklink /J failed");
            LinkKind::Junction
        }
    }

    /// Creates a file symlink at `rel` pointing to `target`. Returns false if
    /// the platform does not allow it (Windows without Developer Mode).
    pub fn file_symlink(&self, target: &Path, rel: &str) -> bool {
        let link = self.path(rel);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, &link).expect("symlink");
            true
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, &link).is_ok()
        }
    }

    /// Makes a directory unreadable. Returns a guard that restores access on
    /// drop (so `TempDir` can clean up), or `None` if the current user can
    /// still read it anyway (e.g. running as root).
    pub fn unreadable_dir(&self, rel: &str) -> Option<AccessGuard> {
        let p = self.path(rel);
        let guard = AccessGuard::deny(&p);
        if fs::read_dir(&p).is_ok() {
            eprintln!(
                "note: could not make {} unreadable (privileged user?)",
                p.display()
            );
            return None;
        }
        Some(guard)
    }
}

/// Restores directory access on drop.
pub struct AccessGuard {
    path: PathBuf,
}

impl AccessGuard {
    #[cfg(unix)]
    fn deny(path: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        AccessGuard {
            path: path.to_path_buf(),
        }
    }

    #[cfg(windows)]
    fn deny(path: &Path) -> Self {
        let user = std::env::var("USERNAME").expect("USERNAME");
        let status = std::process::Command::new("icacls")
            .arg(path)
            .args(["/deny", &format!("{user}:(RD)")])
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run icacls");
        assert!(status.success(), "icacls /deny failed");
        AccessGuard {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for AccessGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        }
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_default();
            let _ = std::process::Command::new("icacls")
                .arg(&self.path)
                .args(["/remove:d", &user])
                .stdout(std::process::Stdio::null())
                .status();
        }
    }
}

/// Scans with default options and no cancellation.
pub fn scan_default(root: &Path) -> Tree {
    scan_with(root, ScanOptions::default()).0
}

/// Scans and returns the tree plus every event that was sent.
pub fn scan_with(root: &Path, opts: ScanOptions) -> (Tree, Vec<ScanEvent>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let tree = scan(root, opts, tx, Arc::new(AtomicBool::new(false))).expect("scan");
    let events = rx.try_iter().collect();
    (tree, events)
}

/// Finds a node by its path relative to the scan root.
pub fn find(tree: &Tree, rel: &str) -> Option<NodeId> {
    let mut cur = tree.root();
    for part in Path::new(rel).components() {
        let name = part.as_os_str();
        cur = *tree
            .children(cur)
            .iter()
            .find(|id| tree.get(**id).is_some_and(|n| &*n.name == name))?;
    }
    Some(cur)
}
