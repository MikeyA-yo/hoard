//! Suggestion rules engine (PRD 6.5).
//!
//! Rules are data (`rules/default.toml`); the engine walks a scanned tree
//! once and reports, per rule, the folders and files it matched. Adding a
//! rule never needs engine changes unless it needs a new matcher, in which
//! case the matcher set is extended here and tested.
//!
//! Semantics:
//! - All matchers present in a rule must hold.
//! - Rules are tried in file order; the first match wins.
//! - Nothing inside a matched folder is matched again (no double counting).
//! - A rule that names paths (`path_is`, `parent_is`, `path_under`) that do
//!   not exist inside the scanned folder matches nothing.
//! - Suggesting is read-only. Removal still goes through the full cleanup
//!   safety pipeline.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::cleanup::protected::resolve;
use crate::tree::{Node, NodeFlags, NodeId, NodeKind, Tree};

/// The built-in rules shipped with Hoard.
pub const DEFAULT_RULES: &str = include_str!("../../../../rules/default.toml");

/// How safe it is to remove what a rule matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Regenerated automatically or on demand; nothing personal.
    Safe,
    /// Usually fine, but look first.
    Review,
    /// Can lose data or break things; read the explanation.
    Caution,
}

/// What kind of space a rule finds (used to group suggestions and lenses).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// Build outputs and dependency folders inside projects.
    Dev,
    /// Package manager caches.
    Packages,
    /// App and browser caches, logs.
    Caches,
    /// The Downloads folder.
    Downloads,
    /// Temporary files.
    Temp,
    /// System-level items (crash dumps, virtual disks).
    System,
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// A rule's match conditions. Empty lists and `None` mean "not used".
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Matcher {
    /// Node is a folder with one of these names.
    #[serde(default, deserialize_with = "one_or_many")]
    pub dir_name: Vec<String>,
    /// Node is a file with one of these names.
    #[serde(default, deserialize_with = "one_or_many")]
    pub file_name: Vec<String>,
    /// Node is a file with one of these extensions (no dot).
    #[serde(default, deserialize_with = "one_or_many")]
    pub ext_in: Vec<String>,
    /// The node's parent contains an entry with one of these names.
    #[serde(default, deserialize_with = "one_or_many")]
    pub sibling: Vec<String>,
    /// The node is a folder directly containing one of these names.
    #[serde(default, deserialize_with = "one_or_many")]
    pub contains_file: Vec<String>,
    /// The node is exactly one of these paths.
    #[serde(default, deserialize_with = "one_or_many")]
    pub path_is: Vec<String>,
    /// The node sits directly inside one of these paths.
    #[serde(default, deserialize_with = "one_or_many")]
    pub parent_is: Vec<String>,
    /// The node is somewhere below one of these paths.
    #[serde(default, deserialize_with = "one_or_many")]
    pub path_under: Vec<String>,
    /// Nothing in the node (or its subtree) changed for this many days.
    #[serde(default)]
    pub older_than_days: Option<u32>,
    /// The node is at least this big (on disk).
    #[serde(default)]
    pub min_bytes: Option<u64>,
}

impl Matcher {
    fn is_empty(&self) -> bool {
        self.dir_name.is_empty()
            && self.file_name.is_empty()
            && self.ext_in.is_empty()
            && self.sibling.is_empty()
            && self.contains_file.is_empty()
            && self.path_is.is_empty()
            && self.parent_is.is_empty()
            && self.path_under.is_empty()
            && self.older_than_days.is_none()
            && self.min_bytes.is_none()
    }
}

fn yes() -> bool {
    true
}

/// One suggestion rule.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Stable identifier.
    pub id: String,
    /// Short display name.
    pub name: String,
    /// Conditions.
    #[serde(rename = "match")]
    pub matcher: Matcher,
    /// Safety level.
    pub safety: Safety,
    /// Kind of space.
    pub group: Group,
    /// One-line explanation of what it is.
    pub explain: String,
    /// How it comes back, or what to run instead.
    #[serde(default)]
    pub regenerate: Option<String>,
    /// Restrict to these OSes (`windows`, `macos`, `linux`).
    #[serde(default)]
    pub os: Option<Vec<String>>,
    /// False for information-only rules that must never reach the tray.
    #[serde(default = "yes")]
    pub addable: bool,
}

/// Errors loading rules.
#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    /// The TOML did not parse or had unknown fields.
    #[error("invalid rules file: {0}")]
    Parse(#[from] toml::de::Error),
    /// A rule is malformed.
    #[error("rule {id}: {problem}")]
    Invalid {
        /// Rule id.
        id: String,
        /// What is wrong.
        problem: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    rule: Vec<Rule>,
}

/// A validated, ordered set of rules.
#[derive(Clone, Debug)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

/// At most this many rules may use `path_under` (they share a bit mask).
const MAX_PATH_UNDER_RULES: usize = 64;

impl RuleSet {
    /// The built-in rules.
    pub fn builtin() -> Result<Self, RulesError> {
        Self::from_toml(DEFAULT_RULES)
    }

    /// Parses and validates rules from TOML.
    pub fn from_toml(text: &str) -> Result<Self, RulesError> {
        let file: RuleFile = toml::from_str(text)?;
        let mut ids = HashSet::new();
        let mut under = 0;
        let mut rules = file.rule;
        for r in &mut rules {
            let invalid = |problem: &str| RulesError::Invalid {
                id: r.id.clone(),
                problem: problem.to_string(),
            };
            if !ids.insert(r.id.clone()) {
                return Err(invalid("duplicate id"));
            }
            if r.matcher.is_empty() {
                return Err(invalid("has no match conditions"));
            }
            if !r.matcher.dir_name.is_empty()
                && (!r.matcher.file_name.is_empty() || !r.matcher.ext_in.is_empty())
            {
                return Err(invalid(
                    "cannot require both a folder name and a file name/extension",
                ));
            }
            if !r.matcher.path_under.is_empty() {
                under += 1;
            }
            for ext in &mut r.matcher.ext_in {
                *ext = ext.trim_start_matches('.').to_lowercase();
            }
        }
        if under > MAX_PATH_UNDER_RULES {
            return Err(RulesError::Invalid {
                id: "*".into(),
                problem: format!("more than {MAX_PATH_UNDER_RULES} rules use path_under"),
            });
        }
        Ok(RuleSet { rules })
    }

    /// Rules in priority order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Looks up a rule by id.
    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }
}

/// Where the user's well-known folders are, for path placeholders.
#[derive(Clone, Debug, Default)]
pub struct MatchContext {
    /// `~` and `{home}`.
    pub home: Option<PathBuf>,
    /// `{downloads}`.
    pub downloads: Option<PathBuf>,
    /// `{local_app_data}` (Windows `%LOCALAPPDATA%`).
    pub local_app_data: Option<PathBuf>,
    /// `{app_data}` (Windows `%APPDATA%`).
    pub app_data: Option<PathBuf>,
    /// `{temp}`.
    pub temp: Option<PathBuf>,
    /// `{cache}` (XDG cache, `~/Library/Caches`, `%LOCALAPPDATA%`).
    pub cache: Option<PathBuf>,
    /// Ignore `os` restrictions (tests).
    pub all_os: bool,
    /// "Now" in unix seconds, for `older_than_days`.
    pub now: i64,
}

impl MatchContext {
    /// The current user's folders.
    pub fn current() -> Self {
        MatchContext {
            home: dirs::home_dir(),
            downloads: dirs::download_dir(),
            local_app_data: dirs::data_local_dir(),
            app_data: dirs::config_dir(),
            temp: Some(std::env::temp_dir()),
            cache: dirs::cache_dir(),
            all_os: false,
            now: now_secs(),
        }
    }

    /// Expands `~` and `{placeholder}` prefixes. `None` if the placeholder is
    /// unknown on this machine.
    pub fn expand(&self, pattern: &str) -> Option<PathBuf> {
        let (base, rest) = if let Some(rest) = pattern.strip_prefix("~/") {
            (self.home.clone()?, rest)
        } else if pattern == "~" {
            (self.home.clone()?, "")
        } else if let Some(stripped) = pattern.strip_prefix('{') {
            let (name, rest) = stripped.split_once('}')?;
            let base = match name {
                "home" => self.home.clone(),
                "downloads" => self.downloads.clone(),
                "local_app_data" => self.local_app_data.clone(),
                "app_data" => self.app_data.clone(),
                "temp" => self.temp.clone(),
                "cache" => self.cache.clone(),
                _ => None,
            }?;
            (base, rest.trim_start_matches('/'))
        } else {
            (PathBuf::new(), pattern)
        };
        let mut out = base;
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            out.push(part);
        }
        Some(out)
    }

    fn applies(&self, rule: &Rule) -> bool {
        if self.all_os {
            return true;
        }
        rule.os
            .as_ref()
            .is_none_or(|list| list.iter().any(|o| o == current_os()))
    }
}

fn current_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One matched item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// The matched node.
    pub node: NodeId,
    /// On-disk size.
    pub size_allocated: u64,
    /// Logical size.
    pub size_apparent: u64,
    /// Newest modification time in the item (unix seconds).
    pub newest_mtime: i64,
}

/// All matches of one rule.
#[derive(Clone, Debug)]
pub struct RuleMatches<'a> {
    /// The rule.
    pub rule: &'a Rule,
    /// Matched items, largest first.
    pub items: Vec<Suggestion>,
    /// Sum of on-disk sizes.
    pub total_allocated: u64,
    /// Sum of logical sizes.
    pub total_apparent: u64,
}

/// A rule with its path matchers resolved to nodes of one tree.
struct Compiled<'a> {
    rule: &'a Rule,
    /// `None` = matcher unused; `Some(empty)` = paths given but none in tree.
    path_is: Option<HashSet<NodeId>>,
    parent_is: Option<HashSet<NodeId>>,
    /// Bit in the "under" mask, if the rule uses `path_under`.
    under_bit: Option<u32>,
    under_targets: Vec<NodeId>,
    dir_name: Vec<String>,
    file_name: Vec<String>,
    sibling: Vec<String>,
    contains_file: Vec<String>,
}

/// Runs `rules` over `tree`. Returns only rules with matches, biggest total
/// first.
pub fn suggest<'a>(tree: &Tree, rules: &'a RuleSet, ctx: &MatchContext) -> Vec<RuleMatches<'a>> {
    let root_key = resolve(tree.root_path());
    let locate_all = |patterns: &[String]| -> Option<HashSet<NodeId>> {
        if patterns.is_empty() {
            return None;
        }
        Some(
            patterns
                .iter()
                .filter_map(|p| ctx.expand(p))
                .filter_map(|p| locate(tree, &root_key, &p))
                .collect(),
        )
    };

    let mut next_bit = 0u32;
    let compiled: Vec<Compiled<'a>> = rules
        .rules()
        .iter()
        .filter(|r| ctx.applies(r))
        .filter_map(|r| {
            let m = &r.matcher;
            let path_is = locate_all(&m.path_is);
            let parent_is = locate_all(&m.parent_is);
            let under = locate_all(&m.path_under);
            // A rule whose named paths are not in this scan cannot match.
            if [&path_is, &parent_is, &under]
                .iter()
                .any(|s| s.as_ref().is_some_and(HashSet::is_empty))
            {
                return None;
            }
            let under_bit = under.as_ref().map(|_| {
                let b = next_bit;
                next_bit += 1;
                b
            });
            Some(Compiled {
                rule: r,
                path_is,
                parent_is,
                under_bit,
                under_targets: under.map(|s| s.into_iter().collect()).unwrap_or_default(),
                dir_name: m.dir_name.iter().map(|s| fold(s)).collect(),
                file_name: m.file_name.iter().map(|s| fold(s)).collect(),
                sibling: m.sibling.iter().map(|s| fold(s)).collect(),
                contains_file: m.contains_file.iter().map(|s| fold(s)).collect(),
            })
        })
        .collect();

    let mut found: Vec<Vec<Suggestion>> = vec![Vec::new(); compiled.len()];
    // (node, mask of path_under rules whose target is an ancestor)
    let mut stack: Vec<(NodeId, u64)> = vec![(tree.root(), 0)];
    while let Some((id, mask)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        if node.flags.contains(NodeFlags::REMOVED) {
            continue;
        }
        if id != tree.root() {
            if let Some(ci) = compiled
                .iter()
                .position(|c| matches(c, tree, id, node, mask, ctx.now))
            {
                if node.size_allocated > 0 || node.size_apparent > 0 {
                    found[ci].push(Suggestion {
                        node: id,
                        size_allocated: node.size_allocated,
                        size_apparent: node.size_apparent,
                        newest_mtime: newest_mtime(tree, id),
                    });
                }
                continue; // never match inside a matched item
            }
        }
        if node.is_dir() {
            let mut child_mask = mask;
            for c in &compiled {
                if let Some(bit) = c.under_bit {
                    if c.under_targets.contains(&id) {
                        child_mask |= 1 << bit;
                    }
                }
            }
            stack.extend(node.children.iter().map(|&k| (k, child_mask)));
        }
    }

    let mut out: Vec<RuleMatches<'a>> = compiled
        .iter()
        .zip(found)
        .filter(|(_, items)| !items.is_empty())
        .map(|(c, mut items)| {
            items.sort_by_key(|a| std::cmp::Reverse(a.size_allocated));
            RuleMatches {
                rule: c.rule,
                total_allocated: items.iter().map(|s| s.size_allocated).sum(),
                total_apparent: items.iter().map(|s| s.size_apparent).sum(),
                items,
            }
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.total_allocated));
    out
}

fn matches(c: &Compiled<'_>, tree: &Tree, id: NodeId, node: &Node, mask: u64, now: i64) -> bool {
    let m = &c.rule.matcher;
    let name = fold(&node.name.to_string_lossy());
    if !c.dir_name.is_empty() && !(node.kind == NodeKind::Dir && c.dir_name.contains(&name)) {
        return false;
    }
    if !c.file_name.is_empty() && !(node.kind == NodeKind::File && c.file_name.contains(&name)) {
        return false;
    }
    if !m.ext_in.is_empty() {
        let ext = Path::new(&*node.name)
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_lowercase);
        if node.kind != NodeKind::File || !ext.is_some_and(|e| m.ext_in.contains(&e)) {
            return false;
        }
    }
    if let Some(set) = &c.path_is {
        if !set.contains(&id) {
            return false;
        }
    }
    if let Some(set) = &c.parent_is {
        if !node.parent.is_some_and(|p| set.contains(&p)) {
            return false;
        }
    }
    if let Some(bit) = c.under_bit {
        if mask & (1 << bit) == 0 {
            return false;
        }
    }
    if !c.sibling.is_empty() {
        let Some(parent) = node.parent else {
            return false;
        };
        if !has_child_named(tree, parent, &c.sibling) {
            return false;
        }
    }
    if !c.contains_file.is_empty()
        && !(node.kind == NodeKind::Dir && has_child_named(tree, id, &c.contains_file))
    {
        return false;
    }
    if let Some(min) = m.min_bytes {
        if node.size_allocated < min {
            return false;
        }
    }
    if let Some(days) = m.older_than_days {
        let cutoff = now.saturating_sub(i64::from(days) * 86_400);
        if newest_mtime(tree, id) >= cutoff {
            return false;
        }
    }
    true
}

fn has_child_named(tree: &Tree, dir: NodeId, names: &[String]) -> bool {
    tree.children(dir)
        .iter()
        .filter_map(|&c| tree.get(c))
        .any(|n| {
            !n.flags.contains(NodeFlags::REMOVED)
                && names.contains(&fold(&n.name.to_string_lossy()))
        })
}

/// Newest modification time in a node's subtree (the node itself for files).
pub fn newest_mtime(tree: &Tree, id: NodeId) -> i64 {
    let mut newest = i64::MIN;
    let mut stack = vec![id];
    while let Some(cur) = stack.pop() {
        if let Some(n) = tree.get(cur) {
            newest = newest.max(n.mtime);
            stack.extend_from_slice(&n.children);
        }
    }
    newest
}

/// Finds the node for `target` inside the tree, if the scan covered it.
fn locate(tree: &Tree, root: &Path, target: &Path) -> Option<NodeId> {
    let target = resolve(target);
    let rel = strip_prefix_folded(&target, root)?;
    let mut cur = tree.root();
    for part in rel.components() {
        let want = fold(&part.as_os_str().to_string_lossy());
        cur = *tree.children(cur).iter().find(|&&c| {
            tree.get(c)
                .is_some_and(|n| fold(&n.name.to_string_lossy()) == want)
        })?;
    }
    Some(cur)
}

/// `path.strip_prefix(prefix)`, comparing components case-insensitively on
/// case-insensitive platforms.
fn strip_prefix_folded(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let mut p = path.components();
    for want in prefix.components() {
        let got = p.next()?;
        if fold(&got.as_os_str().to_string_lossy()) != fold(&want.as_os_str().to_string_lossy()) {
            return None;
        }
    }
    Some(p.as_path().to_path_buf())
}

#[cfg(any(windows, target_os = "macos"))]
fn fold(s: &str) -> String {
    s.to_lowercase()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn fold(s: &str) -> String {
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_parse_and_validate() {
        let set = RuleSet::builtin().expect("default.toml is valid");
        assert!(set.rules().len() >= 20);
        assert!(set.get("node_modules").is_some());
        // Disk images are information only.
        assert!(!set.get("docker_disk").unwrap().addable);
    }

    #[test]
    fn rejects_unknown_fields_and_empty_rules() {
        let typo = r#"
            [[rule]]
            id = "x"
            name = "X"
            match = { dir_nam = "x" }
            safety = "safe"
            group = "dev"
            explain = "x"
        "#;
        assert!(matches!(
            RuleSet::from_toml(typo),
            Err(RulesError::Parse(_))
        ));
        let empty = r#"
            [[rule]]
            id = "x"
            name = "X"
            match = {}
            safety = "safe"
            group = "dev"
            explain = "x"
        "#;
        assert!(matches!(
            RuleSet::from_toml(empty),
            Err(RulesError::Invalid { .. })
        ));
        let dup = r#"
            [[rule]]
            id = "x"
            name = "X"
            match = { dir_name = "a" }
            safety = "safe"
            group = "dev"
            explain = "x"
            [[rule]]
            id = "x"
            name = "Y"
            match = { dir_name = "b" }
            safety = "safe"
            group = "dev"
            explain = "y"
        "#;
        assert!(matches!(
            RuleSet::from_toml(dup),
            Err(RulesError::Invalid { .. })
        ));
    }

    #[test]
    fn expands_placeholders() {
        let ctx = MatchContext {
            home: Some(PathBuf::from("/h")),
            local_app_data: Some(PathBuf::from("/h/AppData/Local")),
            ..MatchContext::default()
        };
        assert_eq!(
            ctx.expand("~/.npm/_cacache").unwrap(),
            Path::new("/h").join(".npm").join("_cacache")
        );
        assert_eq!(
            ctx.expand("{local_app_data}/pip/Cache").unwrap(),
            Path::new("/h/AppData/Local").join("pip").join("Cache")
        );
        assert_eq!(ctx.expand("~").unwrap(), PathBuf::from("/h"));
        assert!(
            ctx.expand("{downloads}/x").is_none(),
            "unknown on this machine"
        );
        assert!(ctx.expand("{nope}/x").is_none());
    }
}
