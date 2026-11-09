//! Mounted disks with capacity and free space (PRD S1).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sysinfo::Disks;

/// One mounted volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Volume label or device name (may be empty).
    pub name: String,
    /// Where the volume is mounted (`C:\`, `/`, `/Volumes/X`).
    pub mount_point: PathBuf,
    /// Filesystem type (`NTFS`, `apfs`, `ext4`, ...).
    pub file_system: String,
    /// Capacity in bytes.
    pub total: u64,
    /// Free bytes available to the current user.
    pub available: u64,
    /// True for USB sticks and other removable media.
    pub removable: bool,
    /// True if mounted read-only.
    pub read_only: bool,
}

impl DiskInfo {
    /// Used bytes (capacity minus available).
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// Lists mounted disks, largest first. Zero-sized pseudo mounts are dropped.
pub fn list_disks() -> Vec<DiskInfo> {
    let disks = Disks::new_with_refreshed_list();
    let mut out: Vec<DiskInfo> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| DiskInfo {
            name: d.name().to_string_lossy().into_owned(),
            mount_point: d.mount_point().to_path_buf(),
            file_system: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
            read_only: d.is_read_only(),
        })
        .collect();
    out.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    out.dedup_by(|a, b| a.mount_point == b.mount_point);
    out
}

/// Free bytes on the volume containing `path`: the disk with the longest
/// mount point that is a prefix of `path`. Used to measure freed space.
pub fn available_space_for(path: &Path) -> Option<u64> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_at_least_one_disk_with_sane_numbers() {
        let disks = list_disks();
        assert!(!disks.is_empty(), "no disks found");
        for d in &disks {
            assert!(d.available <= d.total, "{d:?}");
            assert_eq!(d.used(), d.total - d.available);
        }
    }

    #[test]
    fn free_space_for_temp_dir() {
        let tmp = std::env::temp_dir();
        let tmp = std::path::absolute(tmp).unwrap();
        assert!(available_space_for(&tmp).is_some());
    }
}
