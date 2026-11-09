//! The protected list (PRD 6.7, rule 2).
//!
//! Two kinds of entries:
//! - **subtree**: the path and everything under it (`C:\Windows`, `/usr`).
//! - **exact**: only the path itself; its contents may be cleaned
//!   (`C:\Program Files`, the home folder, `Documents`).
//!
//! Plus, from the [`CleanupContext`]: filesystem roots, the home folder, top
//! level user folders, and the scan root and all of its ancestors.
//!
//! Comparison uses a normalized key built from the canonicalized parent plus
//! the file name, so 8.3 short names (`PROGRA~1`), `\\?\` prefixes, case
//! differences and symlinked parent folders cannot be used to slip past it.

use std::path::{Component, Path, PathBuf, Prefix};

use super::CleanupContext;

/// Why a path is protected, as shown to the user.
pub type Reason = String;

/// Checks `path` against the protected list. Returns the reason if protected.
pub fn check(path: &Path, ctx: &CleanupContext) -> Option<Reason> {
    let key = key_of(&resolve(path));
    if key.len() <= 2 {
        // [prefix?, root] only: a filesystem or drive root.
        return Some("this is the root of a drive".into());
    }
    for (entry, label) in &ctx.protected.subtree {
        if key.starts_with(&key_of(&resolve(entry))) {
            return Some(format!("{label} is a protected system location"));
        }
    }
    for (entry, label) in &ctx.protected.exact {
        if key == key_of(&resolve(entry)) {
            return Some(format!(
                "{label} itself cannot be removed (its contents can)"
            ));
        }
    }
    if let Some(name) = key.last() {
        if key.len() == 3 && SYSTEM_ROOT_FILES.contains(&name.as_str()) {
            return Some("this is a Windows system file".into());
        }
    }
    let root_key = key_of(&resolve(&ctx.scan_root));
    if root_key.starts_with(&key) {
        return Some(if root_key == key {
            "this is the scanned folder itself".into()
        } else {
            "it contains the scanned folder".into()
        });
    }
    None
}

/// Files directly in a drive root that are never removable (Windows).
const SYSTEM_ROOT_FILES: &[&str] = &[
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "dumpstack.log.tmp",
    "bootmgr",
];

/// Resolves `path` to its real location without following the item itself:
/// - an existing non-link: canonicalize the whole path (expands 8.3 short
///   names and symlinked parent folders; nothing at the end to follow);
/// - a link (symlink or junction) or a missing path: canonicalize the
///   nearest existing ancestor and re-append the remaining components, so
///   the link itself is never followed and short names are still expanded.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink());
    if let Ok(false) = is_link {
        if let Ok(p) = std::fs::canonicalize(path) {
            return p;
        }
    }
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = path;
    loop {
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                if let Ok(base) = std::fs::canonicalize(parent) {
                    return tail.iter().rev().fold(base, |acc, part| acc.join(part));
                }
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Normalized comparison key: one string per component, lower-cased on
/// case-insensitive platforms, with verbatim and plain prefixes unified.
fn key_of(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Prefix(p) => Some(match p.kind() {
                Prefix::VerbatimDisk(d) | Prefix::Disk(d) => {
                    format!("{}:", (d as char).to_ascii_lowercase())
                }
                Prefix::VerbatimUNC(server, share) | Prefix::UNC(server, share) => fold(&format!(
                    "\\\\{}\\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                )),
                Prefix::Verbatim(s) | Prefix::DeviceNS(s) => fold(&s.to_string_lossy()),
            }),
            Component::RootDir => Some("/".to_string()),
            Component::CurDir => None,
            Component::ParentDir => Some("..".to_string()),
            Component::Normal(s) => Some(fold(&s.to_string_lossy())),
        })
        .collect()
}

#[cfg(any(windows, target_os = "macos"))]
fn fold(s: &str) -> String {
    s.to_lowercase()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn fold(s: &str) -> String {
    s.to_string()
}

/// System-wide protected entries for the current OS.
#[derive(Clone, Debug, Default)]
pub struct ProtectedList {
    /// The path and everything below it.
    pub subtree: Vec<(PathBuf, String)>,
    /// Only the path itself.
    pub exact: Vec<(PathBuf, String)>,
}

impl ProtectedList {
    /// Built-in entries for this OS.
    pub fn system() -> Self {
        let mut list = ProtectedList::default();
        platform_entries(&mut list);
        list
    }

    /// Adds per-user entries: the home folder, its standard folders, and
    /// sensitive dot-folders.
    pub fn add_user(&mut self, home: &Path, user_dirs: &[PathBuf]) {
        self.exact
            .push((home.to_path_buf(), "your home folder".into()));
        for d in user_dirs {
            self.exact
                .push((d.clone(), "a top-level user folder".into()));
        }
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            "Movies",
            "OneDrive",
            "Public",
        ] {
            self.exact
                .push((home.join(name), "a top-level user folder".into()));
        }
        for name in [".ssh", ".gnupg"] {
            self.subtree
                .push((home.join(name), "your keys folder".into()));
        }
        #[cfg(windows)]
        {
            for rel in [
                "AppData",
                "AppData\\Local",
                "AppData\\Roaming",
                "AppData\\LocalLow",
            ] {
                self.exact
                    .push((home.join(rel), "an AppData folder".into()));
            }
        }
        #[cfg(target_os = "macos")]
        {
            for rel in [
                "Library",
                "Applications",
                "Library/Application Support",
                "Library/Containers",
            ] {
                self.exact.push((home.join(rel), "a Library folder".into()));
            }
            self.subtree
                .push((home.join("Library/Keychains"), "your keychains".into()));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            for rel in [".config", ".local", ".local/share", ".cache"] {
                self.exact
                    .push((home.join(rel), "a configuration folder".into()));
            }
        }
    }
}

#[cfg(windows)]
fn platform_entries(list: &mut ProtectedList) {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let drive = env("SystemDrive").unwrap_or_else(|| PathBuf::from("C:"));
    let root = PathBuf::from(format!("{}\\", drive.display()));
    let windir = env("SystemRoot")
        .or_else(|| env("windir"))
        .unwrap_or_else(|| root.join("Windows"));
    list.subtree.push((windir, "Windows".into()));
    for (rel, label) in [
        ("System Volume Information", "System Volume Information"),
        ("$Recycle.Bin", "the Recycle Bin"),
        ("Recovery", "Recovery"),
        ("Boot", "Boot"),
        ("Config.Msi", "Config.Msi"),
        ("Program Files\\WindowsApps", "WindowsApps"),
        ("ProgramData\\Microsoft", "ProgramData\\Microsoft"),
    ] {
        list.subtree.push((root.join(rel), label.into()));
    }
    let mut exact = vec![
        (root.join("Program Files"), "Program Files"),
        (root.join("Program Files (x86)"), "Program Files (x86)"),
        (root.join("ProgramData"), "ProgramData"),
        (root.join("Users"), "the Users folder"),
        (root.join("Users\\Public"), "the Public user folder"),
        (root.join("Users\\Default"), "the Default user folder"),
    ];
    for (k, label) in [
        ("ProgramFiles", "Program Files"),
        ("ProgramFiles(x86)", "Program Files (x86)"),
        ("ProgramW6432", "Program Files"),
        ("ProgramData", "ProgramData"),
    ] {
        if let Some(p) = env(k) {
            exact.push((p, label));
        }
    }
    list.exact
        .extend(exact.into_iter().map(|(p, l)| (p, l.to_string())));
}

#[cfg(target_os = "macos")]
fn platform_entries(list: &mut ProtectedList) {
    for p in [
        "/System", "/usr", "/bin", "/sbin", "/etc", "/var", "/private", "/cores", "/dev",
    ] {
        list.subtree.push((PathBuf::from(p), p.to_string()));
    }
    for p in [
        "/Applications",
        "/Library",
        "/Users",
        "/Volumes",
        "/opt",
        "/Network",
    ] {
        list.exact.push((PathBuf::from(p), p.to_string()));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_entries(list: &mut ProtectedList) {
    for p in [
        "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/proc", "/run",
        "/sbin", "/sys", "/usr", "/var/lib",
    ] {
        list.subtree.push((PathBuf::from(p), p.to_string()));
    }
    for p in [
        "/home", "/root", "/opt", "/var", "/mnt", "/media", "/srv", "/snap", "/tmp",
    ] {
        list.exact.push((PathBuf::from(p), p.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(scan_root: &Path) -> CleanupContext {
        CleanupContext::new(
            scan_root.to_path_buf(),
            std::env::temp_dir().join("unused.jsonl"),
        )
    }

    /// Every built-in entry rejects itself; subtree entries also reject a
    /// child; exact entries allow a child.
    #[test]
    fn every_system_entry_is_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with(tmp.path());
        let list = ProtectedList::system();
        assert!(!list.subtree.is_empty() && !list.exact.is_empty());
        for (p, _) in &list.subtree {
            assert!(
                check(p, &ctx).is_some(),
                "subtree entry not protected: {}",
                p.display()
            );
            let child = p.join("some-child-item");
            assert!(
                check(&child, &ctx).is_some(),
                "child of subtree entry allowed: {}",
                child.display()
            );
        }
        for (p, _) in &list.exact {
            assert!(
                check(p, &ctx).is_some(),
                "exact entry not protected: {}",
                p.display()
            );
            let child = p.join("some-child-item");
            if !list
                .subtree
                .iter()
                .any(|(s, _)| key_of(&resolve(&child)).starts_with(&key_of(s)))
            {
                assert!(
                    check(&child, &ctx).is_none(),
                    "child of exact entry blocked: {}",
                    child.display()
                );
            }
        }
    }

    #[test]
    fn every_user_entry_is_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let docs = home.join("Docs-redirected");
        let ctx = ctx_with(tmp.path())
            .with_home(home.clone())
            .with_user_dirs(vec![docs.clone()]);
        let mut list = ProtectedList::default();
        list.add_user(&home, &[docs]);
        for (p, _) in list.exact.iter().chain(list.subtree.iter()) {
            assert!(
                check(p, &ctx).is_some(),
                "user entry not protected: {}",
                p.display()
            );
        }
        for (p, _) in &list.subtree {
            assert!(check(&p.join("id_rsa"), &ctx).is_some());
        }
        assert!(check(&home.join("Documents").join("cv.pdf"), &ctx).is_none());
        assert!(check(&home.join("random.txt"), &ctx).is_none());
    }

    #[test]
    fn roots_and_scan_root_ancestors_are_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let scan_root = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&scan_root).unwrap();
        let ctx = ctx_with(&scan_root);
        assert!(check(&scan_root, &ctx).is_some());
        assert!(check(&tmp.path().join("a"), &ctx).is_some());
        assert!(check(tmp.path(), &ctx).is_some());
        assert!(check(&scan_root.join("child"), &ctx).is_none());
        // A sibling that merely shares a name prefix is not an ancestor.
        assert!(check(&tmp.path().join("a").join("b-sibling"), &ctx).is_none());
        #[cfg(windows)]
        {
            assert!(check(Path::new("C:\\"), &ctx).is_some());
            assert!(check(Path::new("D:\\"), &ctx).is_some());
            assert!(check(Path::new("C:\\pagefile.sys"), &ctx).is_some());
        }
        #[cfg(unix)]
        assert!(check(Path::new("/"), &ctx).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_prefix_and_short_names_do_not_bypass() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_with(tmp.path());
        let windir = std::env::var("SystemRoot").unwrap();
        assert!(check(Path::new(&windir.to_uppercase()), &ctx).is_some());
        assert!(check(&Path::new(&windir.to_lowercase()).join("System32"), &ctx).is_some());
        assert!(check(
            &Path::new(&format!("\\\\?\\{windir}")).join("System32"),
            &ctx
        )
        .is_some());
        // 8.3 short name of Program Files resolves to the real folder.
        assert!(check(Path::new("C:\\PROGRA~1"), &ctx).is_some());
        assert!(check(Path::new("C:\\Program Files\\WindowsApps\\x"), &ctx).is_some());
        assert!(check(Path::new("C:\\Program Files\\SomeApp"), &ctx).is_none());
    }
}
