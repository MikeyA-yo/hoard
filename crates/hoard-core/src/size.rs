//! Allocated (on-disk) versus apparent (logical) size.
//!
//! Hoard records both for every node and defaults to allocated size (PRD S4).
//! - Unix: allocated = `st_blocks * 512`.
//! - Windows: allocated = `FILE_STANDARD_INFO.AllocationSize` (cluster
//!   rounded, 0 for dehydrated cloud placeholders), except for compressed or
//!   sparse files where `GetCompressedFileSizeW` gives the real on-disk size.

use serde::{Deserialize, Serialize};

/// Which size to show and sort by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeMode {
    /// Bytes actually allocated on disk. The default.
    #[default]
    Allocated,
    /// Logical file length, as reported by `ls -l`.
    Apparent,
}

/// Allocated size from a Unix `st_blocks` value (always 512-byte units).
#[cfg(unix)]
pub(crate) fn allocated_from_blocks(blocks: u64) -> u64 {
    blocks.saturating_mul(512)
}

/// Allocated size on Windows.
///
/// `allocation_size` is `FILE_STANDARD_INFO.AllocationSize`. For compressed or
/// sparse files that value overstates usage, so ask `GetCompressedFileSizeW`
/// instead. `wide_path` is the NUL-terminated verbatim path.
#[cfg(windows)]
pub(crate) fn allocated_windows(allocation_size: i64, attrs: u32, wide_path: &[u16]) -> u64 {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, NO_ERROR};
    use windows::Win32::Storage::FileSystem::{
        GetCompressedFileSizeW, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_SPARSE_FILE,
        INVALID_FILE_SIZE,
    };

    let fallback = u64::try_from(allocation_size).unwrap_or(0);
    if attrs & (FILE_ATTRIBUTE_COMPRESSED.0 | FILE_ATTRIBUTE_SPARSE_FILE.0) == 0 {
        return fallback;
    }
    let mut high: u32 = 0;
    // SAFETY: `wide_path` is NUL terminated and outlives the call; `high` is a valid out pointer.
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(wide_path.as_ptr()), Some(&mut high)) };
    // SAFETY: GetLastError has no preconditions.
    if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != NO_ERROR {
        return fallback;
    }
    (u64::from(high) << 32) | u64::from(low)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn blocks_are_512_bytes() {
        assert_eq!(super::allocated_from_blocks(8), 4096);
        assert_eq!(super::allocated_from_blocks(u64::MAX), u64::MAX);
    }
}
