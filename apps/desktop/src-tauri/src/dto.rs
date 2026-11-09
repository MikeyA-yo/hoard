//! Serializable DTOs for IPC. Core types are converted here; the frontend
//! never sees `Tree` or `Node` directly.

use hoard_core::classify::{category_for, Category};
use hoard_core::disks::DiskInfo;
use hoard_core::view::TreemapNode;
use hoard_core::{NodeId, NodeKind, ScanError, ScanErrorKind, SizeMode, Tree};
use serde::{Deserialize, Serialize};

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
