//! Apps lens (PRD L5): installed apps and the space attributed to them.
//!
//! - Install folder from the OS's app registry: confidence **exact**.
//! - Data folders under the user/app data roots matched by app or
//!   publisher name: confidence **likely** (never bulk-added; PRD risk table).
//!
//! Windows reads the Uninstall registry keys; other platforms return no
//! apps yet (M5).

#[cfg(windows)]
mod windows;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cleanup::protected::resolve;
use crate::rules::MatchContext;
use crate::tree::{NodeFlags, NodeId, Tree};

/// An installed app as registered with the OS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp {
    /// Display name.
    pub name: String,
    /// Publisher, if registered.
    pub publisher: Option<String>,
    /// Version, if registered.
    pub version: Option<String>,
    /// Install folder, if registered.
    pub install_location: Option<PathBuf>,
    /// The OS's own size estimate in bytes, if registered.
    pub estimated_size: Option<u64>,
}

/// Installed apps from the OS registry (empty where not supported yet).
pub fn installed_apps() -> Vec<InstalledApp> {
    #[cfg(windows)]
    {
        windows::installed_apps()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// How sure the attribution is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// The app's registered install folder.
    Exact,
    /// A data folder whose name matches the app or its publisher.
    Likely,
}

/// A folder attributed to an app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFolder {
    /// The folder.
    pub node: NodeId,
    /// How sure.
    pub confidence: Confidence,
    /// On-disk bytes.
    pub size_allocated: u64,
}

/// An app and everything attributed to it in the scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFootprint {
    /// The app.
    pub app: InstalledApp,
    /// Install folder, if it is inside the scan.
    pub install: Option<AppFolder>,
    /// Data folders, largest first.
    pub data: Vec<AppFolder>,
    /// Install + data, on disk.
    pub total: u64,
}

/// Lower-case letters and digits only.
fn key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// App name without versions, architecture and edition noise.
fn app_key(name: &str) -> String {
    let is_version = |w: &str| {
        let w = w.strip_prefix(['v', 'V']).unwrap_or(w);
        !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == '.')
    };
    let noise = [
        "x64", "x86", "64-bit", "32-bit", "amd64", "arm64", "version", "-",
    ];
    let cleaned: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|w| !is_version(w) && !noise.contains(&w.to_ascii_lowercase().as_str()))
        .collect();
    key(&cleaned.join(" "))
}

/// Publisher without legal suffixes.
fn publisher_key(p: &str) -> String {
    let words: Vec<&str> = p
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
        .filter(|w| {
            ![
                "inc",
                "llc",
                "ltd",
                "corp",
                "corporation",
                "co",
                "gmbh",
                "limited",
                "sa",
                "ag",
                "bv",
                "the",
            ]
            .contains(&w.to_ascii_lowercase().as_str())
        })
        .collect();
    key(&words.join(" "))
}

/// Folder names too generic to ever attribute.
const STOPLIST: &[&str] = &[
    "microsoft",
    "packages",
    "temp",
    "programs",
    "cache",
    "caches",
    "crashdumps",
    "d3dscache",
    "local",
    "roaming",
    "locallow",
    "comms",
    "connecteddevicesplatform",
    "history",
    "publishers",
    "virtualstore",
    "low",
    "common",
    "commonfiles",
    "windows",
    "nvidia",
    "intel",
    "amd",
    "app",
    "apps",
    "data",
    "logs",
    "config",
    "settings",
];

/// Does a folder name match an app key? Exact, or a long enough prefix /
/// suffix of the app key ("Code" for "Visual Studio Code").
fn name_matches(folder: &str, app: &str) -> bool {
    let f = key(folder);
    if f.len() < 3 || app.is_empty() || STOPLIST.contains(&f.as_str()) {
        return false;
    }
    f == app || (f.len() >= 4 && (app.ends_with(&f) || app.starts_with(&f)))
}

/// Finds the node for `path` inside the tree.
fn locate(tree: &Tree, root: &Path, path: &Path) -> Option<NodeId> {
    let target = resolve(path);
    let rel_components: Vec<String> = {
        let t: Vec<String> = target
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        let r: Vec<String> = root
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        if t.len() < r.len() || t[..r.len()] != r[..] {
            return None;
        }
        t[r.len()..].to_vec()
    };
    let mut cur = tree.root();
    for part in rel_components {
        cur = *tree.children(cur).iter().find(|&&c| {
            tree.get(c)
                .is_some_and(|n| n.name.to_string_lossy().to_lowercase() == part)
        })?;
    }
    Some(cur)
}

fn live_dir(tree: &Tree, id: NodeId) -> bool {
    tree.get(id)
        .is_some_and(|n| n.is_dir() && !n.flags.contains(NodeFlags::REMOVED))
}

/// Attributes install and data folders in `tree` to `apps`. Each folder is
/// attributed to at most one app (first match in largest-app order).
pub fn attribute(tree: &Tree, apps: &[InstalledApp], ctx: &MatchContext) -> Vec<AppFootprint> {
    let root = resolve(tree.root_path());
    let data_roots: Vec<NodeId> = [
        ctx.app_data.clone(),
        ctx.local_app_data.clone(),
        program_data(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|p| locate(tree, &root, &p))
    .collect();
    let size = |id: NodeId| tree.get(id).map_or(0, |n| n.size_allocated);
    let mut claimed: HashSet<NodeId> = HashSet::new();

    // Publishers with exactly one app may claim their whole publisher folder.
    let mut publisher_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for a in apps {
        if let Some(p) = &a.publisher {
            *publisher_counts.entry(publisher_key(p)).or_default() += 1;
        }
    }

    let mut out: Vec<AppFootprint> = apps
        .iter()
        .map(|app| {
            let install = app
                .install_location
                .as_ref()
                .and_then(|p| locate(tree, &root, p))
                .filter(|&id| live_dir(tree, id) && id != tree.root());
            if let Some(id) = install {
                claimed.insert(id);
            }
            AppFootprint {
                app: app.clone(),
                install: install.map(|id| AppFolder {
                    node: id,
                    confidence: Confidence::Exact,
                    size_allocated: size(id),
                }),
                data: Vec::new(),
                total: 0,
            }
        })
        .collect();

    for fp in &mut out {
        let akey = app_key(&fp.app.name);
        let pkey = fp
            .app
            .publisher
            .as_deref()
            .map(publisher_key)
            .unwrap_or_default();
        let sole_publisher = !pkey.is_empty() && publisher_counts.get(&pkey) == Some(&1);
        let mut found: Vec<NodeId> = Vec::new();
        for &dr in &data_roots {
            for &child in tree.children(dr) {
                if !live_dir(tree, child) || claimed.contains(&child) {
                    continue;
                }
                let Some(n) = tree.get(child) else { continue };
                let name = n.name.to_string_lossy();
                // A publisher folder is checked first: it may hold several
                // apps, so it is only claimed whole by a sole-app publisher.
                if !pkey.is_empty() && key(&name) == pkey {
                    // Publisher folder: look one level down for the app.
                    let inner: Vec<NodeId> = tree
                        .children(child)
                        .iter()
                        .copied()
                        .filter(|&g| live_dir(tree, g) && !claimed.contains(&g))
                        .filter(|&g| {
                            tree.get(g)
                                .is_some_and(|gn| name_matches(&gn.name.to_string_lossy(), &akey))
                        })
                        .collect();
                    if !inner.is_empty() {
                        found.extend(inner);
                    } else if sole_publisher && !STOPLIST.contains(&pkey.as_str()) {
                        found.push(child);
                    }
                } else if name_matches(&name, &akey) {
                    found.push(child);
                }
            }
        }
        for id in found {
            if claimed.insert(id) {
                fp.data.push(AppFolder {
                    node: id,
                    confidence: Confidence::Likely,
                    size_allocated: size(id),
                });
            }
        }
        fp.data.sort_by_key(|d| std::cmp::Reverse(d.size_allocated));
        fp.total = fp.install.as_ref().map_or(0, |i| i.size_allocated)
            + fp.data.iter().map(|d| d.size_allocated).sum::<u64>();
    }
    out.retain(|fp| fp.total > 0);
    out.sort_by_key(|fp| std::cmp::Reverse(fp.total));
    out
}

fn program_data() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_strip_noise() {
        assert_eq!(
            app_key("Microsoft Visual Studio Code (User)"),
            "microsoftvisualstudiocodeuser"
        );
        assert_eq!(app_key("7-Zip 23.01 (x64)"), "7zip");
        assert_eq!(app_key("Python 3.13.0 (64-bit)"), "python");
        assert_eq!(publisher_key("Google LLC"), "google");
        assert_eq!(publisher_key("Microsoft Corporation"), "microsoft");
    }

    #[test]
    fn folder_matching() {
        assert!(name_matches("Discord", "discord"));
        assert!(name_matches("Code", "microsoftvisualstudiocode"));
        assert!(name_matches("Spotify", "spotify"));
        assert!(!name_matches("Microsoft", "microsoftteams"), "stoplisted");
        assert!(!name_matches("Temp", "tempcleaner"), "stoplisted");
        assert!(!name_matches("go", "google"), "too short");
        assert!(!name_matches("Slack", "discord"));
    }
}
