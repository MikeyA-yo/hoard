//! Serializable DTOs for IPC. Core types are converted here; the frontend
//! never sees `Tree` or `Node` directly.

use hoard_core::apps::{AppFolder, AppFootprint, Confidence};
use hoard_core::classify::{category_for, Category};
use hoard_core::cleanup::{CleanupMode, CleanupReport, Tray};
use hoard_core::disks::DiskInfo;
use hoard_core::lenses::{CategoryTotal, Project};
use hoard_core::rules::{Group as RuleGroupKind, RuleMatches, Safety};
use hoard_core::snapshot::{ChangeKind, DiffEntry};
use hoard_core::view::{TreemapNode, MAX_VIEW_ITEMS};
use hoard_core::{NodeId, NodeKind, ScanError, ScanErrorKind, SizeMode, Tree};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ tray

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayItemDto {
    pub node: NodeDto,
    pub path: String,
}

/// The whole tray. `version` increases on every change and `generation`
/// identifies the scan; the UI applies a response only if it is newer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayDto {
    pub generation: u64,
    pub version: u64,
    pub count: usize,
    pub items: Vec<TrayItemDto>,
    pub total_allocated: u64,
    pub total_apparent: u64,
    pub file_count: u64,
    /// Text the user must type to run a permanent delete of this tray, if any.
    pub permanent_confirmation: Option<String>,
    /// Notes about the last change (covered, replaced, rejected items).
    pub notes: Vec<String>,
}

impl TrayDto {
    pub fn new(
        tray: &Tray,
        tree: &Tree,
        notes: Vec<String>,
        permanent_confirmation: Option<String>,
    ) -> Self {
        let items: Vec<TrayItemDto> = tray
            .items()
            .iter()
            .take(MAX_VIEW_ITEMS)
            .filter_map(|&id| {
                Some(TrayItemDto {
                    node: NodeDto::new(tree, id)?,
                    path: tree.path(id)?.to_string_lossy().into_owned(),
                })
            })
            .collect();
        TrayDto {
            generation: tray.generation(),
            version: tray.version(),
            count: tray.items().len(),
            items,
            total_allocated: tray.total(tree, SizeMode::Allocated),
            total_apparent: tray.total(tree, SizeMode::Apparent),
            file_count: tray.file_count(tree),
            permanent_confirmation,
            notes,
        }
    }

    pub fn empty(tray: &Tray) -> Self {
        TrayDto {
            generation: tray.generation(),
            version: tray.version(),
            count: 0,
            items: Vec::new(),
            total_allocated: 0,
            total_apparent: 0,
            file_count: 0,
            permanent_confirmation: None,
            notes: Vec::new(),
        }
    }
}

// ------------------------------------------------------------------ suggestions

/// Items listed per rule; the rest are counted, not sent.
pub const MAX_SUGGESTION_ITEMS: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionItemDto {
    pub node: NodeDto,
    pub path: String,
    pub newest_mtime: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionGroupDto {
    pub rule_id: String,
    pub name: String,
    pub safety: Safety,
    pub group: RuleGroupKind,
    pub explain: String,
    pub regenerate: Option<String>,
    pub addable: bool,
    pub total_allocated: u64,
    pub total_apparent: u64,
    pub count: usize,
    pub items: Vec<SuggestionItemDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionsDto {
    pub groups: Vec<SuggestionGroupDto>,
    /// Bulk-addable total: Safe and addable groups only.
    pub safe_total: u64,
    pub safe_count: usize,
    pub review_total: u64,
    pub caution_total: u64,
}

impl SuggestionsDto {
    pub fn new(tree: &Tree, groups: &[RuleMatches<'_>]) -> Self {
        let sum = |pred: &dyn Fn(&RuleMatches<'_>) -> bool| -> u64 {
            groups
                .iter()
                .filter(|g| pred(g))
                .map(|g| g.total_allocated)
                .sum()
        };
        SuggestionsDto {
            safe_total: sum(&|g| g.rule.safety == Safety::Safe && g.rule.addable),
            safe_count: groups
                .iter()
                .filter(|g| g.rule.safety == Safety::Safe && g.rule.addable)
                .map(|g| g.items.len())
                .sum(),
            review_total: sum(&|g| g.rule.safety == Safety::Review),
            caution_total: sum(&|g| g.rule.safety == Safety::Caution),
            groups: groups
                .iter()
                .map(|g| SuggestionGroupDto {
                    rule_id: g.rule.id.clone(),
                    name: g.rule.name.clone(),
                    safety: g.rule.safety,
                    group: g.rule.group,
                    explain: g.rule.explain.clone(),
                    regenerate: g.rule.regenerate.clone(),
                    addable: g.rule.addable,
                    total_allocated: g.total_allocated,
                    total_apparent: g.total_apparent,
                    count: g.items.len(),
                    items: g
                        .items
                        .iter()
                        .take(MAX_SUGGESTION_ITEMS)
                        .filter_map(|s| {
                            Some(SuggestionItemDto {
                                node: NodeDto::new(tree, s.node)?,
                                path: tree.path(s.node)?.to_string_lossy().into_owned(),
                                newest_mtime: s.newest_mtime,
                            })
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

// ------------------------------------------------------------------ lenses

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotalDto {
    pub category: Category,
    pub size_allocated: u64,
    pub size_apparent: u64,
    pub files: u64,
}

impl From<&CategoryTotal> for CategoryTotalDto {
    fn from(c: &CategoryTotal) -> Self {
        CategoryTotalDto {
            category: c.category,
            size_allocated: c.size_allocated,
            size_apparent: c.size_apparent,
            files: c.files,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDto {
    pub node: NodeDto,
    pub path: String,
    pub rule_id: String,
    pub rule_name: String,
    pub safety: Safety,
    pub addable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub root: NodeDto,
    pub path: String,
    pub last_activity: i64,
    pub total_allocated: u64,
    pub total_apparent: u64,
    pub artifacts: Vec<ArtifactDto>,
}

/// Projects listed per request; the rest are counted.
pub const MAX_PROJECTS: usize = 1_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsDto {
    pub projects: Vec<ProjectDto>,
    pub count: usize,
    pub total_allocated: u64,
}

impl ProjectsDto {
    pub fn new(tree: &Tree, projects: &[Project]) -> Self {
        ProjectsDto {
            count: projects.len(),
            total_allocated: projects.iter().map(|p| p.total_allocated).sum(),
            projects: projects
                .iter()
                .take(MAX_PROJECTS)
                .filter_map(|p| {
                    Some(ProjectDto {
                        root: NodeDto::new(tree, p.root)?,
                        path: tree.path(p.root)?.to_string_lossy().into_owned(),
                        last_activity: p.last_activity,
                        total_allocated: p.total_allocated,
                        total_apparent: p.total_apparent,
                        artifacts: p
                            .artifacts
                            .iter()
                            .filter_map(|a| {
                                Some(ArtifactDto {
                                    node: NodeDto::new(tree, a.node)?,
                                    path: tree.path(a.node)?.to_string_lossy().into_owned(),
                                    rule_id: a.rule_id.clone(),
                                    rule_name: a.rule_name.clone(),
                                    safety: a.safety,
                                    addable: a.addable,
                                })
                            })
                            .collect(),
                    })
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashSizeDto {
    pub bytes: u64,
    pub items: u64,
}

// ------------------------------------------------------------------ cleanup

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CleanupProgressDto {
    pub run_id: String,
    pub done: usize,
    pub total: usize,
    pub current_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupItemDto {
    pub path: String,
    pub kind: NodeKind,
    pub size_allocated: u64,
    pub size_apparent: u64,
    /// removed | skipped | failed | partial
    pub status: &'static str,
    pub reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReportDto {
    pub run_id: String,
    pub mode: CleanupMode,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub partial: usize,
    pub estimated_freed: u64,
    pub estimated_freed_apparent: u64,
    /// Free-space change of the volume (after minus before), if measurable.
    pub measured_freed: Option<i64>,
    pub cancelled: bool,
    /// Items that were not removed come first, then removed ones.
    pub items: Vec<CleanupItemDto>,
    pub total_items: usize,
}

impl From<&CleanupReport> for CleanupReportDto {
    fn from(r: &CleanupReport) -> Self {
        let mut items: Vec<CleanupItemDto> = r
            .results
            .iter()
            .map(|i| CleanupItemDto {
                path: i.path.to_string_lossy().into_owned(),
                kind: i.kind,
                size_allocated: i.size_allocated,
                size_apparent: i.size_apparent,
                status: i.outcome.status(),
                reason: i.outcome.reason(),
            })
            .collect();
        items.sort_by_key(|i| i.status == "removed");
        let total_items = items.len();
        items.truncate(MAX_VIEW_ITEMS);
        CleanupReportDto {
            run_id: r.run_id.clone(),
            mode: r.mode,
            removed: r.count("removed"),
            skipped: r.count("skipped"),
            failed: r.count("failed"),
            partial: r.count("partial"),
            estimated_freed: r.estimated_freed,
            estimated_freed_apparent: r.estimated_freed_apparent,
            measured_freed: r.measured_freed(),
            cancelled: r.cancelled,
            items,
            total_items,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskDto {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub removable: bool,
    pub read_only: bool,
}

impl From<&DiskInfo> for DiskDto {
    fn from(d: &DiskInfo) -> Self {
        DiskDto {
            name: d.name.clone(),
            mount_point: d.mount_point.to_string_lossy().into_owned(),
            file_system: d.file_system.clone(),
            total: d.total,
            available: d.available,
            used: d.used(),
            removable: d.removable,
            read_only: d.read_only,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanOptsDto {
    pub cross_filesystems: bool,
    pub apparent: bool,
    pub skip: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDto {
    pub scan_id: u64,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub current_path: String,
    pub errors: u64,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchEntryDto {
    pub id: u32,
    pub name: String,
    pub kind: NodeKind,
    pub category: Category,
    pub size_allocated: u64,
    pub size_apparent: u64,
    pub file_count: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchDto {
    pub scan_id: u64,
    pub top_level: Vec<BatchEntryDto>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SummaryDto {
    pub root_id: u32,
    pub root_path: String,
    pub size_allocated: u64,
    pub size_apparent: u64,
    pub files: u32,
    pub dirs: usize,
    pub error_count: usize,
    pub complete: bool,
    pub size_mode: SizeMode,
    pub elapsed_ms: u64,
}

impl SummaryDto {
    pub fn new(tree: &Tree, elapsed_ms: u64) -> Self {
        let root = tree.get(tree.root());
        SummaryDto {
            root_id: tree.root().0,
            root_path: tree.root_path().to_string_lossy().into_owned(),
            size_allocated: root.map_or(0, |n| n.size_allocated),
            size_apparent: root.map_or(0, |n| n.size_apparent),
            files: root.map_or(0, |n| n.file_count),
            dirs: tree.dir_count(),
            error_count: tree.errors().len(),
            complete: tree.is_complete(),
            size_mode: tree.size_mode(),
            elapsed_ms,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DoneDto {
    pub scan_id: u64,
    /// Present when the scan produced a tree (complete or cancelled).
    pub summary: Option<SummaryDto>,
    /// Present when the scan could not start (e.g. root unreadable).
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDto {
    pub id: u32,
    pub name: String,
    pub kind: NodeKind,
    pub category: Category,
    pub size_allocated: u64,
    pub size_apparent: u64,
    pub file_count: u32,
    pub child_count: usize,
    pub mtime: i64,
    pub flags: Vec<&'static str>,
    pub parent: Option<u32>,
}

impl NodeDto {
    pub fn new(tree: &Tree, id: NodeId) -> Option<Self> {
        let n = tree.get(id)?;
        Some(NodeDto {
            id: id.0,
            name: n.display_name(),
            kind: n.kind,
            category: category_for(n.kind, &n.name),
            size_allocated: n.size_allocated,
            size_apparent: n.size_apparent,
            file_count: n.file_count,
            child_count: n.children.len(),
            mtime: n.mtime,
            flags: n.flags.names(),
            parent: n.parent.map(|p| p.0),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrumbDto {
    pub id: u32,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetailDto {
    pub node: NodeDto,
    pub path: String,
    pub ancestors: Vec<CrumbDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildrenPageDto {
    pub total: usize,
    pub offset: usize,
    pub items: Vec<NodeDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LargestItemDto {
    pub node: NodeDto,
    pub path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreemapDto {
    pub id: Option<u32>,
    pub name: String,
    pub kind: Option<NodeKind>,
    pub category: Category,
    pub size: u64,
    pub file_count: u32,
    pub mtime: i64,
    pub merged_count: u32,
    pub has_more: bool,
    pub children: Vec<TreemapDto>,
}

impl From<TreemapNode> for TreemapDto {
    fn from(t: TreemapNode) -> Self {
        TreemapDto {
            id: t.id.map(|i| i.0),
            name: t.name,
            kind: t.kind,
            category: t.category,
            size: t.size,
            file_count: t.file_count,
            mtime: t.mtime,
            merged_count: t.merged_count,
            has_more: t.has_more,
            children: t.children.into_iter().map(TreemapDto::from).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub path: String,
    pub kind: ScanErrorKind,
    pub message: String,
}

impl From<&ScanError> for ErrorDto {
    fn from(e: &ScanError) -> Self {
        ErrorDto {
            path: e.path.to_string_lossy().into_owned(),
            kind: e.kind,
            message: e.message.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorsPageDto {
    pub total: usize,
    pub items: Vec<ErrorDto>,
}

// ------------------------------------------------------------------ duplicates

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DupesProgressDto {
    pub run_id: String,
    pub stage: u8,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesSummaryDto {
    pub run_id: String,
    pub groups: usize,
    pub wasted: u64,
    pub unreadable: u64,
    pub cancelled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupFileDto {
    pub node: NodeDto,
    pub path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroupDto {
    pub size: u64,
    pub wasted: u64,
    pub files: Vec<DupFileDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupesPageDto {
    /// False until a search has finished for the loaded scan.
    pub ready: bool,
    pub total_groups: usize,
    pub wasted: u64,
    pub unreadable: u64,
    pub groups: Vec<DupGroupDto>,
}

// ------------------------------------------------------------------ history

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    pub taken_at: i64,
    pub total_allocated: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntryDto {
    pub path: String,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub change: i64,
    pub kind: ChangeKind,
}

impl From<&DiffEntry> for DiffEntryDto {
    fn from(d: &DiffEntry) -> Self {
        DiffEntryDto {
            path: d.path.clone(),
            before: d.before,
            after: d.after,
            change: d.change(),
            kind: d.kind(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthDto {
    /// When the earlier snapshot was taken.
    pub since: i64,
    pub change: i64,
    pub top: Vec<DiffEntryDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDto {
    /// Newest first.
    pub snapshots: Vec<SnapshotDto>,
    /// Latest vs the one before, if there are two.
    pub growth: Option<GrowthDto>,
}

// ------------------------------------------------------------------ apps

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppFolderDto {
    pub node: NodeDto,
    pub path: String,
    pub confidence: Confidence,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDto {
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub estimated_size: Option<u64>,
    pub total: u64,
    pub install: Option<AppFolderDto>,
    pub data: Vec<AppFolderDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppsDto {
    pub apps: Vec<AppDto>,
    /// Apps registered with the OS (including those with nothing in this scan).
    pub registered: usize,
}

impl AppsDto {
    pub fn new(tree: &Tree, footprints: &[AppFootprint], registered: usize) -> Self {
        let folder = |f: &AppFolder| {
            Some(AppFolderDto {
                node: NodeDto::new(tree, f.node)?,
                path: tree.path(f.node)?.to_string_lossy().into_owned(),
                confidence: f.confidence,
            })
        };
        AppsDto {
            registered,
            apps: footprints
                .iter()
                .take(MAX_VIEW_ITEMS)
                .map(|fp| AppDto {
                    name: fp.app.name.clone(),
                    publisher: fp.app.publisher.clone(),
                    version: fp.app.version.clone(),
                    estimated_size: fp.app.estimated_size,
                    total: fp.total,
                    install: fp.install.as_ref().and_then(folder),
                    data: fp.data.iter().filter_map(folder).collect(),
                })
                .collect(),
        }
    }
}
