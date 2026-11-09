//! File classification.
//!
//! M1 provides file categories by extension (used to color the treemap and,
//! in M3, for the File types lens). Dev artifact detection lands in M3.

use std::ffi::OsStr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tree::NodeKind;

/// Broad file category (PRD L2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// A directory (colored separately from files).
    Folder,
    /// Video files.
    Video,
    /// Images and photos, including RAW and design files.
    Image,
    /// Audio files.
    Audio,
    /// Compressed archives and packages.
    Archive,
    /// Disk and VM images.
    DiskImage,
    /// Documents, spreadsheets, slides, plain text.
    Document,
    /// Source code and config.
    Code,
    /// Executables and libraries.
    Executable,
    /// Anything else.
    #[default]
    Other,
}

impl Category {
    /// All categories, in display order.
    pub const ALL: [Category; 10] = [
        Category::Folder,
        Category::Video,
        Category::Image,
        Category::Audio,
        Category::Archive,
        Category::DiskImage,
        Category::Document,
        Category::Code,
        Category::Executable,
        Category::Other,
    ];
}

const VIDEO: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "mts", "m2ts",
    "vob", "ogv",
];
const IMAGE: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif", "raw",
    "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "psd", "xcf", "svg", "ico", "icns", "exr",
    "hdr",
];
const AUDIO: &[&str] = &[
    "mp3", "wav", "flac", "aac", "ogg", "oga", "m4a", "wma", "opus", "aif", "aiff", "alac", "mid",
    "midi", "ape",
];
const ARCHIVE: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "lz4", "lzma",
    "cab", "jar", "war", "apk", "aab", "nupkg", "whl", "crate", "deb", "rpm",
];
const DISK_IMAGE: &[&str] = &[
    "iso",
    "dmg",
    "img",
    "vhd",
    "vhdx",
    "vmdk",
    "qcow2",
    "vdi",
    "wim",
    "esd",
    "sparseimage",
    "sparsebundle",
    "raw_img",
];
const DOCUMENT: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt", "md",
    "epub", "mobi", "csv", "tsv", "pages", "numbers", "key", "tex", "log",
];
const CODE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "c", "h", "cc", "cpp", "hpp", "cs",
    "java", "kt", "swift", "rb", "php", "html", "css", "scss", "json", "toml", "yaml", "yml",
    "xml", "sh", "ps1", "sql", "lua", "dart", "vue", "svelte", "zig", "ex", "exs", "hs", "ml",
    "scala", "r", "ipynb", "map", "lock",
];
const EXECUTABLE: &[&str] = &[
    "exe", "msi", "dll", "so", "dylib", "sys", "appimage", "pkg", "bin", "app", "msix", "appx",
    "node", "pdb", "a", "lib", "o", "obj", "rlib", "class", "pyc", "wasm",
];

/// Category for a file name, by extension (case-insensitive).
pub fn category_for_name(name: &OsStr) -> Category {
    let Some(ext) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return Category::Other;
    };
    let ext = ext.to_ascii_lowercase();
    let e = ext.as_str();
    let table: [(&[&str], Category); 8] = [
        (VIDEO, Category::Video),
        (IMAGE, Category::Image),
        (AUDIO, Category::Audio),
        (ARCHIVE, Category::Archive),
        (DISK_IMAGE, Category::DiskImage),
        (DOCUMENT, Category::Document),
        (CODE, Category::Code),
        (EXECUTABLE, Category::Executable),
    ];
    table
        .iter()
        .find(|(list, _)| list.contains(&e))
        .map_or(Category::Other, |(_, c)| *c)
}

/// Category for a node: directories are [`Category::Folder`], everything else
/// goes by extension.
pub fn category_for(kind: NodeKind, name: &OsStr) -> Category {
    match kind {
        NodeKind::Dir => Category::Folder,
        _ => category_for_name(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str) -> Category {
        category_for_name(OsStr::new(name))
    }

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(c("movie.MKV"), Category::Video);
        assert_eq!(c("photo.jpeg"), Category::Image);
        assert_eq!(c("song.flac"), Category::Audio);
        assert_eq!(c("backup.tar.gz"), Category::Archive);
        assert_eq!(c("ubuntu.iso"), Category::DiskImage);
        assert_eq!(c("report.PDF"), Category::Document);
        assert_eq!(c("main.rs"), Category::Code);
        assert_eq!(c("setup.exe"), Category::Executable);
    }

    #[test]
    fn unknown_and_missing_extensions_are_other() {
        assert_eq!(c("Makefile"), Category::Other);
        assert_eq!(c("data.xyz123"), Category::Other);
        assert_eq!(c(".bashrc"), Category::Other);
    }

    #[test]
    fn directories_are_folders_regardless_of_name() {
        assert_eq!(
            category_for(NodeKind::Dir, OsStr::new("videos.mp4")),
            Category::Folder
        );
        assert_eq!(
            category_for(NodeKind::File, OsStr::new("videos.mp4")),
            Category::Video
        );
    }
}
