//! Windows stat via `CreateFileW` + `GetFileInformationByHandle`.
//!
//! `std::fs::symlink_metadata` does not expose the file index, volume serial
//! number or link count on stable Rust, so we open each entry ourselves with
//! `FILE_FLAG_OPEN_REPARSE_POINT` (never follows links or junctions) and
//! `FILE_READ_ATTRIBUTES` access (does not hydrate cloud placeholders or
//! conflict with other processes' share modes).

use std::cell::Cell;
use std::ffi::{c_void, OsStr};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FileStatInformation, FILE_INFORMATION_CLASS, FILE_STAT_INFORMATION,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_SHARING_VIOLATION, FILETIME, HANDLE, NTSTATUS, STATUS_INVALID_INFO_CLASS,
    STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED, STATUS_NOT_SUPPORTED, UNICODE_STRING,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_COMPRESSED,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE,
    FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

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

/// Signature of `ntdll!NtQueryInformationByName` (Windows 10 1709+).
type QueryByNameFn = unsafe extern "system" fn(
    *const OBJECT_ATTRIBUTES,
    *mut IO_STATUS_BLOCK,
    *mut c_void,
    u32,
    FILE_INFORMATION_CLASS,
) -> NTSTATUS;

/// `NtQueryInformationByName`, looked up at runtime so Hoard still starts on
/// systems without it (it then uses [`stat`] for every entry).
fn query_by_name_fn() -> Option<QueryByNameFn> {
    use std::sync::OnceLock;
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    static FUNC: OnceLock<Option<QueryByNameFn>> = OnceLock::new();
    *FUNC.get_or_init(|| {
        // SAFETY: ntdll.dll is mapped into every Windows process; both names
        // are static NUL-terminated strings.
        let proc = unsafe {
            let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
            GetProcAddress(ntdll, s!("NtQueryInformationByName"))?
        };
        // SAFETY: the exported function has exactly the documented signature
        // declared by `QueryByNameFn`.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryByNameFn>(proc)
        })
    })
}

/// Per-directory stat state: an open handle on the directory, used as the
/// root for `NtQueryInformationByName(FileStatInformation)` on each entry.
///
/// That one call returns everything [`stat`] gathers with an open plus three
/// queries (file id, link count, sizes, attributes, reparse tag), and it
/// describes reparse points themselves, never their targets. Measured about
/// 4x faster than [`stat`] per entry, with identical results.
///
/// One difference: the query reports 0 for a directory's own allocation (its
/// index). [`DirStat::own_allocated`] supplies that value once the scanner
/// opens the directory itself to list it, so no extra open is needed.
///
/// Anything unusual falls back to [`stat`] on the full path: no handle, the
/// call missing or unsupported by the filesystem, an error (so error
/// messages stay the same), or a compressed / sparse file (whose allocated
/// size needs `GetCompressedFileSizeW`).
pub(super) struct DirStat {
    /// Directory handle, volume serial and the query function, or `None` to
    /// always fall back.
    fast: Option<(OwnedHandle, u64, QueryByNameFn)>,
    /// The directory's own allocated size, computed exactly as [`stat`]
    /// does, when the fast path is open.
    own_alloc: Option<u64>,
    /// Cleared after the filesystem rejects the information class, so the
    /// rest of the directory goes straight to the fallback.
    by_name_ok: Cell<bool>,
}

impl DirStat {
    pub(super) fn open(dir: &Path) -> Self {
        match Self::open_fast(dir) {
            Some((fast, own_alloc)) => DirStat {
                fast: Some(fast),
                own_alloc: Some(own_alloc),
                by_name_ok: Cell::new(true),
            },
            None => DirStat {
                fast: None,
                own_alloc: None,
                by_name_ok: Cell::new(false),
            },
        }
    }

    /// Opens the directory and returns the fast-path state plus the
    /// directory's own allocated size.
    #[allow(clippy::type_complexity)]
    fn open_fast(dir: &Path) -> Option<((OwnedHandle, u64, QueryByNameFn), u64)> {
        let func = query_by_name_fn()?;
        let wide = to_wide_verbatim(dir);
        // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .ok()?;
        let handle = OwnedHandle(handle);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: valid handle, `info` is a correctly sized out parameter.
        unsafe { GetFileInformationByHandle(handle.0, &mut info) }.ok()?;
        // Only a real directory may be a root for relative names. The
        // scanner never descends into links, but a folder can be swapped for
        // a junction between listing and here.
        let attrs = info.dwFileAttributes;
        if attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0 || attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return None;
        }
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
        .ok()?;
        // Same computation as `stat` uses for a directory.
        let own_alloc = size::allocated_windows(std_info.AllocationSize, attrs, &wide);
        Some((
            (handle, u64::from(info.dwVolumeSerialNumber), func),
            own_alloc,
        ))
    }

    pub(super) fn own_allocated(&self) -> Option<u64> {
        self.own_alloc
    }

    pub(super) fn is_fast(&self) -> bool {
        self.fast.is_some() && self.by_name_ok.get()
    }

    pub(super) fn stat(&self, name: &OsStr, path: &Path) -> io::Result<EntryMeta> {
        match self.stat_fast(name) {
            Some(meta) => Ok(meta),
            None => stat(path),
        }
    }

    fn stat_fast(&self, name: &OsStr) -> Option<EntryMeta> {
        let (dir, dev, func) = self.fast.as_ref()?;
        if !self.by_name_ok.get() {
            return None;
        }
        let wide: Vec<u16> = name.encode_wide().collect();
        // Never let a separator turn the relative name into a path.
        if wide.is_empty()
            || wide
                .iter()
                .any(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
        {
            return None;
        }
        let bytes = u16::try_from(wide.len() * 2).ok()?;
        let object_name = UNICODE_STRING {
            Length: bytes,
            MaximumLength: bytes,
            Buffer: PWSTR(wide.as_ptr().cast_mut()),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: dir.0,
            ObjectName: &object_name,
            ..Default::default()
        };
        let mut iosb = IO_STATUS_BLOCK::default();
        let mut info = FILE_STAT_INFORMATION::default();
        // SAFETY: all pointers refer to live locals of the right types (the
        // name buffer is only read), the size matches the output buffer, and
        // `func` is NtQueryInformationByName (see `query_by_name_fn`).
        let status = unsafe {
            func(
                &attributes,
                &mut iosb,
                (&mut info as *mut FILE_STAT_INFORMATION).cast::<c_void>(),
                size_of::<FILE_STAT_INFORMATION>() as u32,
                FileStatInformation,
            )
        };
        if status.is_err() {
            if matches!(
                status,
                STATUS_INVALID_INFO_CLASS
                    | STATUS_NOT_IMPLEMENTED
                    | STATUS_NOT_SUPPORTED
                    | STATUS_INVALID_PARAMETER
            ) {
                self.by_name_ok.set(false);
            }
            return None;
        }
        let attrs = info.FileAttributes;
        if attrs & (FILE_ATTRIBUTE_COMPRESSED.0 | FILE_ATTRIBUTE_SPARSE_FILE.0) != 0 {
            return None;
        }
        let is_link = attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            && info.ReparseTag & REPARSE_TAG_NAME_SURROGATE != 0;
        let kind = if is_link {
            NodeKind::Symlink
        } else if attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
            NodeKind::Dir
        } else {
            NodeKind::File
        };
        let allocated = u64::try_from(info.AllocationSize).unwrap_or(0);
        let (size_apparent, size_allocated) = match kind {
            NodeKind::Symlink => (0, 0),
            NodeKind::Dir => (0, allocated),
            NodeKind::File | NodeKind::Other => {
                (u64::try_from(info.EndOfFile).unwrap_or(0), allocated)
            }
        };
        Some(EntryMeta {
            kind,
            size_apparent,
            size_allocated,
            mtime: ticks_to_unix(u64::try_from(info.LastWriteTime).unwrap_or(0)),
            dev: *dev,
            // The 64-bit NTFS file id, the same value `stat` builds from
            // nFileIndexHigh/Low (the bits are reinterpreted, not converted).
            ino: info.FileId as u64,
            nlink: u64::from(info.NumberOfLinks),
        })
    }
}

/// A raw volume handle. Volume handles do no caching, so buffers must be
/// sector-aligned in memory as well as in offset and length; unaligned
/// buffers go through an aligned bounce buffer.
struct WinVolume {
    file: std::fs::File,
}

impl WinVolume {
    fn read_exact_at(&self, offset: u64, dst: &mut [u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < dst.len() {
            let n = self
                .file
                .seek_read(&mut dst[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

impl super::VolumeRead for WinVolume {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if (buf.as_ptr() as usize).is_multiple_of(super::VOLUME_ALIGN) {
            return self.read_exact_at(offset, buf);
        }
        let mut bounce = vec![0u8; buf.len() + super::VOLUME_ALIGN];
        let start = bounce.as_ptr().align_offset(super::VOLUME_ALIGN);
        let dst = &mut bounce[start..start + buf.len()];
        self.read_exact_at(offset, dst)?;
        buf.copy_from_slice(dst);
        Ok(())
    }
}

/// See [`super::open_raw_volume`].
pub(super) fn open_raw_volume(path: &Path) -> Option<Box<dyn super::VolumeRead + Send>> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Storage::FileSystem::{
        GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };

    let mut plain: Vec<u16> = path.as_os_str().encode_wide().collect();
    plain.push(0);
    let mut mount = [0u16; 1024];
    // SAFETY: NUL-terminated input; the output slice length is passed along.
    unsafe { GetVolumePathNameW(PCWSTR(plain.as_ptr()), &mut mount) }.ok()?;
    let mut guid = [0u16; 128];
    // SAFETY: `mount` is NUL-terminated by the previous call; output sized.
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid) }.ok()?;
    let mut fs_name = [0u16; 64];
    // SAFETY: `guid` is a NUL-terminated volume root path.
    unsafe {
        GetVolumeInformationW(
            PCWSTR(guid.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_name),
        )
    }
    .ok()?;
    let fs_len = fs_name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(fs_name.len());
    if String::from_utf16_lossy(&fs_name[..fs_len]) != "NTFS" {
        return None;
    }
    // The device itself is the volume name without its trailing backslash.
    let mut device: Vec<u16> = guid.iter().copied().take_while(|&c| c != 0).collect();
    if device.last() == Some(&u16::from(b'\\')) {
        device.pop();
    }
    device.push(0);
    // SAFETY: NUL-terminated device path that outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(device.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
    }
    .ok()?;
    // SAFETY: a valid handle we own; the File takes over closing it.
    let file = unsafe { std::fs::File::from_raw_handle(handle.0) };
    Some(Box::new(WinVolume { file }))
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

/// Total size and item count of the Recycle Bins on all drives.
pub(super) fn recycle_bin_size() -> Option<(u64, u64)> {
    use windows::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
    let mut info = SHQUERYRBINFO {
        cbSize: size_of::<SHQUERYRBINFO>() as u32,
        i64Size: 0,
        i64NumItems: 0,
    };
    // SAFETY: a null root path means "all drives"; `info` is a valid out
    // parameter with cbSize set as the API requires.
    unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) }.ok()?;
    Some((
        u64::try_from(info.i64Size).unwrap_or(0),
        u64::try_from(info.i64NumItems).unwrap_or(0),
    ))
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
    ticks_to_unix((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime))
}

/// Converts 100 ns ticks since 1601 to unix seconds.
fn ticks_to_unix(ticks: u64) -> i64 {
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
    fn recycle_bin_size_is_readable() {
        // Read-only query; the numbers depend on the machine.
        assert!(recycle_bin_size().is_some());
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
