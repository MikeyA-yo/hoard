//! Linux specifics.

/// Kernel pseudo filesystems. Never scanned.
pub(super) const PSEUDO_FS: &[&str] = &["/proc", "/sys", "/dev", "/run", "/var/run"];
