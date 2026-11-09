//! Lenses: alternative groupings of the same scan (PRD 6.3).
//!
//! - File types (L2): totals per category, and the largest files of one.
//! - Projects (L3): dev artifacts grouped by project, with the project's
//!   last activity (newest source file, ignoring artifacts and `.git`).
//!
//! The caches lens (L4) is the suggestions list filtered by rule group.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::classify::{category_for_name, Category};
use crate::rules::{Group, RuleMatches, Safety};
use crate::size::SizeMode;
use crate::tree::{NodeFlags, NodeId, NodeKind, Tree};

/// Totals for one file category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategoryTotal {
    /// The category.
    pub category: Category,
    /// On-disk bytes.
    pub size_allocated: u64,
    /// Logical bytes.
    pub size_apparent: u64,
    /// Number of files.
    pub files: u64,
}

/// Visits every live (not removed) non-directory node.
fn for_each_file(tree: &Tree, mut f: impl FnMut(NodeId, &crate::tree::Node)) {
    let mut stack = vec![tree.root()];
    while let Some(id) = stack.pop() {
        let Some(n) = tree.get(id) else { continue };
        if n.flags.contains(NodeFlags::REMOVED) {
            continue;
        }
        if n.is_dir() {
            stack.extend_from_slice(&n.children);
        } else {
            f(id, n);
        }
    }
}

/// Totals per file category, largest (on disk) first. Hard-link duplicates
/// count zero, as everywhere else.
pub fn file_types(tree: &Tree) -> Vec<CategoryTotal> {
    let mut totals: HashMap<Category, CategoryTotal> = HashMap::new();
    for_each_file(tree, |_, n| {
        let category = category_for_name(&n.name);
        let t = totals.entry(category).or_insert(CategoryTotal {
            category,
            size_allocated: 0,
            size_apparent: 0,
            files: 0,
        });
        t.size_allocated = t.size_allocated.saturating_add(n.size_allocated);
        t.size_apparent = t.size_apparent.saturating_add(n.size_apparent);
        t.files += 1;
    });
    let mut out: Vec<CategoryTotal> = totals.into_values().collect();
    out.sort_by_key(|t| (Reverse(t.size_allocated), t.files));
    out
}

/// The `limit` largest files of one category, largest first.
pub fn largest_in_category(
    tree: &Tree,
    category: Category,
    limit: usize,
    mode: SizeMode,
) -> Vec<NodeId> {
    if limit == 0 {
        return Vec::new();
    }
    let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::with_capacity(limit + 1);
    for_each_file(tree, |id, n| {
        if category_for_name(&n.name) == category {
            heap.push(Reverse((n.size(mode), id)));
            if heap.len() > limit {
                heap.pop();
            }
        }
    });
    let mut out: Vec<(u64, NodeId)> = heap.into_iter().map(|Reverse(x)| x).collect();
    out.sort_by(|a, b| b.cmp(a));
    out.into_iter().map(|(_, id)| id).collect()
}

/// Names that mark a project root.
pub const PROJECT_MARKERS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    ".git",
];

/// A dev artifact inside a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// The artifact folder.
    pub node: NodeId,
    /// Rule that found it.
    pub rule_id: String,
    /// The rule's display name.
    pub rule_name: String,
    /// The rule's safety level.
    pub safety: Safety,
    /// Whether it may go in the tray.
    pub addable: bool,
    /// On-disk bytes.
    pub size_allocated: u64,
    /// Logical bytes.
    pub size_apparent: u64,
}

/// A project and its artifacts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    /// The project folder (nearest ancestor with a project marker).
    pub root: NodeId,
    /// Newest source file modification (unix seconds), 0 if none.
    pub last_activity: i64,
    /// Artifacts, largest first.
    pub artifacts: Vec<Artifact>,
    /// Sum of artifact on-disk sizes.
    pub total_allocated: u64,
    /// Sum of artifact logical sizes.
    pub total_apparent: u64,
}

fn has_marker(tree: &Tree, dir: NodeId) -> bool {
    tree.children(dir)
        .iter()
        .filter_map(|&c| tree.get(c))
        .any(|n| {
            !n.flags.contains(NodeFlags::REMOVED)
                && PROJECT_MARKERS
                    .iter()
                    .any(|m| n.name.to_string_lossy().eq_ignore_ascii_case(m))
        })
}

/// Nearest ancestor of `id` (excluding `id`) that is a project root.
fn project_root(tree: &Tree, id: NodeId) -> Option<NodeId> {
    let mut cur = tree.get(id)?.parent;
    while let Some(p) = cur {
        if has_marker(tree, p) {
            return Some(p);
        }
        cur = tree.get(p)?.parent;
    }
    None
}

/// Groups dev artifacts (rules in [`Group::Dev`]) by project, largest total
/// first. Artifacts with no project root above them are left out (they
/// still appear in suggestions).
pub fn projects(tree: &Tree, matches: &[RuleMatches<'_>]) -> Vec<Project> {
    let mut by_root: HashMap<NodeId, Vec<Artifact>> = HashMap::new();
    let mut skip: HashSet<NodeId> = HashSet::new();
    for m in matches.iter().filter(|m| m.rule.group == Group::Dev) {
        for s in &m.items {
            skip.insert(s.node);
            let Some(root) = project_root(tree, s.node) else {
                continue;
            };
            by_root.entry(root).or_default().push(Artifact {
                node: s.node,
                rule_id: m.rule.id.clone(),
                rule_name: m.rule.name.clone(),
                safety: m.rule.safety,
                addable: m.rule.addable,
                size_allocated: s.size_allocated,
                size_apparent: s.size_apparent,
            });
        }
    }

    let mut out: Vec<Project> = by_root
        .into_iter()
        .map(|(root, mut artifacts)| {
            artifacts.sort_by_key(|a| Reverse(a.size_allocated));
            Project {
                root,
                last_activity: last_activity(tree, root, &skip),
                total_allocated: artifacts.iter().map(|a| a.size_allocated).sum(),
                total_apparent: artifacts.iter().map(|a| a.size_apparent).sum(),
                artifacts,
            }
        })
        .collect();
    out.sort_by_key(|p| (Reverse(p.total_allocated), p.root));
    out
}

/// Newest file mtime under `root`, not descending into `skip` folders or
/// `.git`. Folder mtimes are ignored (creating `node_modules` touches them).
fn last_activity(tree: &Tree, root: NodeId, skip: &HashSet<NodeId>) -> i64 {
    let mut newest = 0;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(n) = tree.get(id) else { continue };
        if n.flags.contains(NodeFlags::REMOVED) || skip.contains(&id) {
            continue;
        }
        match n.kind {
            NodeKind::Dir => {
                if id != root && n.name.to_string_lossy().eq_ignore_ascii_case(".git") {
                    continue;
                }
                stack.extend_from_slice(&n.children);
            }
            _ => newest = newest.max(n.mtime),
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::platform::EntryMeta;
    use crate::rules::{suggest, MatchContext, RuleSet};
    use crate::tree::TreeBuilder;

    fn file(size: u64, mtime: i64) -> EntryMeta {
        EntryMeta {
            kind: NodeKind::File,
            size_allocated: size,
            size_apparent: size,
            mtime,
            nlink: 1,
            ..EntryMeta::default()
        }
    }

    fn dir(mtime: i64) -> EntryMeta {
        EntryMeta {
            kind: NodeKind::Dir,
            mtime,
            nlink: 1,
            ..EntryMeta::default()
        }
    }

    /// root/
    ///   media/ movie.mkv (5000), song.mp3 (300), photo.jpg (200)
    ///   web/   package.json (t=100), src/app.ts (t=500), node_modules/x.js (t=900), .git/HEAD (t=990)
    ///   tool/  Cargo.toml (t=50), target/bin (t=999)
    ///   loose/ __pycache__/m.pyc  (no project marker)
    fn sample() -> Tree {
        let mut b = TreeBuilder::new(PathBuf::from("/r"), &dir(0), SizeMode::Allocated);
        let r = b.root();
        let n = crate::tree::NodeFlags::NONE;
        let media = b.push(r, "media".into(), &dir(0), n).unwrap();
        b.push(media, "movie.mkv".into(), &file(5000, 1), n)
            .unwrap();
        b.push(media, "song.mp3".into(), &file(300, 1), n).unwrap();
        b.push(media, "photo.jpg".into(), &file(200, 1), n).unwrap();
        let web = b.push(r, "web".into(), &dir(995), n).unwrap();
        b.push(web, "package.json".into(), &file(10, 100), n)
            .unwrap();
        let src = b.push(web, "src".into(), &dir(0), n).unwrap();
        b.push(src, "app.ts".into(), &file(20, 500), n).unwrap();
        let nm = b.push(web, "node_modules".into(), &dir(900), n).unwrap();
        b.push(nm, "x.js".into(), &file(4000, 900), n).unwrap();
        let git = b.push(web, ".git".into(), &dir(990), n).unwrap();
        b.push(git, "HEAD".into(), &file(5, 990), n).unwrap();
        let tool = b.push(r, "tool".into(), &dir(0), n).unwrap();
        b.push(tool, "Cargo.toml".into(), &file(10, 50), n).unwrap();
        let target = b.push(tool, "target".into(), &dir(999), n).unwrap();
        b.push(target, "bin".into(), &file(9000, 999), n).unwrap();
        let loose = b.push(r, "loose".into(), &dir(0), n).unwrap();
        let pc = b.push(loose, "__pycache__".into(), &dir(0), n).unwrap();
        b.push(pc, "m.pyc".into(), &file(100, 1), n).unwrap();
        b.finish(Vec::new(), true)
    }

    fn ctx() -> MatchContext {
        MatchContext {
            all_os: true,
            now: 10_000,
            ..MatchContext::default()
        }
    }

    #[test]
    fn file_type_totals() {
        let t = sample();
        let totals = file_types(&t);
        let get = |c: Category| totals.iter().find(|x| x.category == c).cloned();
        assert_eq!(get(Category::Video).unwrap().size_apparent, 5000);
        assert_eq!(get(Category::Audio).unwrap().files, 1);
        assert_eq!(get(Category::Image).unwrap().size_apparent, 200);
        assert!(
            get(Category::Folder).is_none(),
            "folders are not a file type"
        );
        // Largest first: other (target/bin 9000 + .git/HEAD + m.pyc) before video.
        assert_eq!(totals[0].category, Category::Other);
        let all: u64 = totals.iter().map(|x| x.size_apparent).sum();
        assert_eq!(all, t.total(SizeMode::Apparent));
    }

    #[test]
    fn largest_files_of_one_category() {
        let t = sample();
        let ids = largest_in_category(&t, Category::Code, 10, SizeMode::Apparent);
        let names: Vec<String> = ids
            .iter()
            .map(|i| t.get(*i).unwrap().display_name())
            .collect();
        // Config files (.toml, .json) count as code too.
        assert_eq!(names, vec!["x.js", "app.ts", "Cargo.toml", "package.json"]);
        assert!(largest_in_category(&t, Category::Code, 0, SizeMode::Apparent).is_empty());
    }

    #[test]
    fn projects_group_artifacts_and_ignore_artifacts_for_activity() {
        let mut t = sample();
        let rules = RuleSet::builtin().unwrap();
        let matches = suggest(&t, &rules, &ctx());
        let ps = projects(&t, &matches);
        let names: Vec<String> = ps
            .iter()
            .map(|p| t.get(p.root).unwrap().display_name())
            .collect();
        // tool (target 9000) before web (node_modules 4000); loose has no marker.
        assert_eq!(names, vec!["tool", "web"]);
        let web = &ps[1];
        assert_eq!(web.artifacts[0].rule_id, "node_modules");
        assert_eq!(web.total_apparent, 4000);
        // node_modules (900), .git (990) and folder mtimes (995) are ignored.
        assert_eq!(web.last_activity, 500);
        assert_eq!(ps[0].last_activity, 50);

        // Removed artifacts disappear from the lens.
        let target = ps[0].artifacts[0].node;
        t.remove_subtree(target);
        let ps = projects(&t, &suggest(&t, &rules, &ctx()));
        assert_eq!(ps.len(), 1);
    }
}
