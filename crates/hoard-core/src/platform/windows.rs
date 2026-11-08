//! Windows stat via `CreateFileW` + `GetFileInformationByHandle`.
//!
//! `std::fs::symlink_metadata` does not expose the file index, volume serial
//! number or link count on stable Rust, so we open each entry ourselves with
//! `FILE_FLAG_OPEN_REPARSE_POINT` (never follows links or junctions) and
//! `FILE_READ_ATTRIBUTES` access (does not hydrate cloud placeholders or
//! conflict with other processes' share modes).

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_SHARING_VIOLATION, FILETIME, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
};

use super::EntryMeta;
use crate::size;
use crate::tree::NodeKind;

/// Reparse tags with this bit set are "name surrogates": symlinks, junctions
/// and mount points. They are treated as links and never followed.
const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;

/// Closes the handle on drop.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful CreateFileW and is closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(super) fn stat(path: &Path) -> io::Result<EntryMeta> {
    let wide = to_wide_verbatim(path);
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
    let opened = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = match opened {
        Ok(h) => OwnedHandle(h),
        Err(e) => {
            let err = win_err(e);
            if err.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) {
                return stat_fallback(path);
            }
            return Err(err);
        }
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: valid handle, `info` is a correctly sized out parameter.
    unsafe { GetFileInformationByHandle(handle.0, &mut info) }.map_err(win_err)?;

    let mut std_info = FILE_STANDARD_INFO::default();
    // SAFETY: valid handle, buffer and size match FILE_STANDARD_INFO.
    unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileStandardInfo,
            (&mut std_info as *mut FILE_STANDARD_INFO).cast::<c_void>(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }
    .map_err(win_err)?;

    let attrs = info.dwFileAttributes;
    let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    let is_link = attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 && is_name_surrogate(&handle)?;

    let kind = if is_link {
        NodeKind::Symlink
    } else if is_dir {
        NodeKind::Dir
    } else {
        NodeKind::File
    };

    let (size_apparent, size_allocated) = match kind {
        NodeKind::Symlink => (0, 0),
        NodeKind::Dir => (
            0,
            size::allocated_windows(std_info.AllocationSize, attrs, &wide),
        ),
        NodeKind::File | NodeKind::Other => (
            u64::try_from(std_info.EndOfFile).unwrap_or(0),
            size::allocated_windows(std_info.AllocationSize, attrs, &wide),
        ),
    };

    Ok(EntryMeta {
        kind,
        size_apparent,
        size_allocated,
        mtime: filetime_to_unix(info.ftLastWriteTime),
        dev: u64::from(info.dwVolumeSerialNumber),
        ino: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        nlink: u64::from(info.nNumberOfLinks),
    })
}

/// Converts a `windows` crate error to an `io::Error` carrying the plain
/// Win32 error code (e.g. 5 for access denied), so messages read
/// "os error 5" and `raw_os_error` comparisons work. The crate's own `From`
/// impl keeps the HRESULT form (0x8007xxxx).
fn win_err(e: windows::core::Error) -> io::Error {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
    } else {
        io::Error::from(e)
    }
}

fn is_name_surrogate(handle: &OwnedHandle) -> io::Result<bool> {
    let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: valid handle, buffer and size match FILE_ATTRIBUTE_TAG_INFO.
    unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileAttributeTagInfo,
            (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast::<c_void>(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    }
    .map_err(win_err)?;
    Ok(tag.ReparseTag & REPARSE_TAG_NAME_SURROGATE != 0)
}

/// For files that refuse even an attributes-only open (e.g. `pagefile.sys`),
/// fall back to the directory listing data that std reads. No file index is
/// available, so hardlink detection cannot apply; the allocated size is the
/// apparent size rounded up to 4 KiB.
fn stat_fallback(path: &Path) -> io::Result<EntryMeta> {
    let md = std::fs::symlink_metadata(path)?;
    let ft = md.file_type();
    if ft.is_dir() {
        // Directories need a real identity (dev + ino) for cleanup validation.
        return Err(io::Error::from_raw_os_error(
            ERROR_SHARING_VIOLATION.0 as i32,
        ));
    }
    let kind = if ft.is_symlink() {
        NodeKind::Symlink
    } else {
        NodeKind::File
    };
    let len = if kind == NodeKind::Symlink {
        0
    } else {
        md.len()
    };
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    Ok(EntryMeta {
        kind,
        size_apparent: len,
        size_allocated: len.div_ceil(4096).saturating_mul(4096),
        mtime,
        dev: 0,
        ino: 0,
        nlink: 1,
    })
}

fn filetime_to_unix(ft: FILETIME) -> i64 {
    let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    // 10,000,000 ticks of 100 ns per second; u64 / 1e7 always fits in i64.
    (ticks / 10_000_000) as i64 - FILETIME_UNIX_OFFSET_SECS
}

/// Converts a path to a NUL-terminated verbatim (`\\?\`) UTF-16 path so that
/// paths longer than `MAX_PATH` work. Expects an absolute, normalized path
/// (the scanner builds every path from `std::path::absolute(root)`).
pub(crate) fn to_wide_verbatim(path: &Path) -> Vec<u16> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let starts_with = |prefix: &str| {
        let p: Vec<u16> = prefix.encode_utf16().collect();
        wide.starts_with(&p)
    };
    let mut out: Vec<u16> = Vec::with_capacity(wide.len() + 9);
    if starts_with(r"\\?\") || starts_with(r"\\.\") {
        out.extend_from_slice(&wide);
    } else if starts_with(r"\\") {
        out.extend(r"\\?\UNC\".encode_utf16());
        out.extend_from_slice(&wide[2..]);
    } else if path.is_absolute() {
        out.extend(r"\\?\".encode_utf16());
        out.extend_from_slice(&wide);
    } else {
        out.extend_from_slice(&wide);
    }
    // Verbatim paths are passed to the filesystem as is, so separators must be
    // backslashes. '/' is never valid inside a Windows file name.
    for c in &mut out {
        if *c == u16::from(b'/') {
            *c = u16::from(b'\\');
        }
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[u16]) -> String {
        String::from_utf16_lossy(&v[..v.len() - 1])
    }

    #[test]
    fn verbatim_prefixes() {
        assert_eq!(s(&to_wide_verbatim(Path::new(r"C:\a\b"))), r"\\?\C:\a\b");
        assert_eq!(
            s(&to_wide_verbatim(Path::new(r"\\srv\share\x"))),
            r"\\?\UNC\srv\share\x"
        );
        assert_eq!(s(&to_wide_verbatim(Path::new(r"\\?\C:\a"))), r"\\?\C:\a");
        assert_eq!(s(&to_wide_verbatim(Path::new("C:/a/b"))), r"\\?\C:\a\b");
    }

    #[test]
    fn win32_errors_keep_plain_codes() {
        use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};
        let e = win_err(windows::core::Error::from(ERROR_ACCESS_DENIED.to_hresult()));
        assert_eq!(e.raw_os_error(), Some(5));
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        let e = win_err(windows::core::Error::from(
            ERROR_SHARING_VIOLATION.to_hresult(),
        ));
        assert_eq!(e.raw_os_error(), Some(ERROR_SHARING_VIOLATION.0 as i32));
    }

    #[test]
    fn filetime_epoch() {
        // 1970-01-01 in FILETIME ticks.
        let ticks: u64 = 116_444_736_000_000_000;
        let ft = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        assert_eq!(filetime_to_unix(ft), 0);
    }
}
