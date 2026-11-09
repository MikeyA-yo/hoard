//! Tauri commands. Each is a thin adapter: look up state, call `hoard-core`,
//! convert to DTOs. All are `async` so they run off the main (UI) thread.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use hoard_core::classify::category_for;
use hoard_core::view::{self, TreemapOptions, MAX_VIEW_ITEMS};
use hoard_core::{LargestKind, NodeId, ScanEvent, ScanOptions, SizeMode, SortKey};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::dto::{
    BatchDto, BatchEntryDto, ChildrenPageDto, CrumbDto, DiskDto, DoneDto, ErrorDto, ErrorsPageDto,
    LargestItemDto, NodeDetailDto, NodeDto, ProgressDto, ScanOptsDto, SummaryDto, TreemapDto,
};
use crate::state::{AppState, ScanResult};

/// Upper bound for `get_largest`.
const MAX_LARGEST: usize = 1_000;

type CmdResult<T> = Result<T, String>;

#[tauri::command]
pub async fn list_disks() -> CmdResult<Vec<DiskDto>> {
    Ok(hoard_core::disks::list_disks()
        .iter()
        .map(DiskDto::from)
        .collect())
}

#[tauri::command]
pub async fn home_dir(app: AppHandle) -> CmdResult<String> {
    app.path()
        .home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// Starts a scan on a background thread and returns its id. Any running scan
/// is cancelled. Emits `scan://progress`, `scan://batch` and `scan://done`.
#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    opts: Option<ScanOptsDto>,
) -> CmdResult<u64> {
    let opts = opts.unwrap_or_default();
    let (scan_id, cancel) = state.begin_scan()?;
    let scan_opts = ScanOptions {
        cross_filesystems: opts.cross_filesystems,
        apparent: opts.apparent,
        skip: opts.skip.into_iter().map(PathBuf::from).collect(),
        threads: 0,
    };
    let root = PathBuf::from(path);
    std::thread::Builder::new()
        .name(format!("hoard-scan-{scan_id}"))
        .spawn(move || run_scan(app, scan_id, root, scan_opts, cancel))
        .map_err(|e| format!("could not start scan thread: {e}"))?;
    Ok(scan_id)
}

fn run_scan(
    app: AppHandle,
    scan_id: u64,
    root: PathBuf,
    opts: ScanOptions,
    cancel: Arc<AtomicBool>,
) {
    let (tx, rx) = crossbeam_channel::bounded::<ScanEvent>(256);
    let forward_app = app.clone();
    let forwarder = std::thread::spawn(move || {
        for event in rx {
            forward_event(&forward_app, scan_id, event);
        }
    });

    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        hoard_core::scan(&root, opts, tx, cancel)
    }));
    // The sender was moved into `scan` and is dropped by now, so the
    // forwarder drains remaining events and exits.
    let _ = forwarder.join();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let state = app.state::<AppState>();
    let done = match outcome {
        Ok(Ok(tree)) => {
            let summary = SummaryDto::new(&tree, elapsed_ms);
            let stored = state.finish_scan(
                scan_id,
                Some(ScanResult {
                    tree: Arc::new(tree),
                    elapsed_ms,
                }),
            );
            if !stored {
                // Superseded by a newer scan; its own done event will follow.
                return;
            }
            DoneDto {
                scan_id,
                summary: Some(summary),
                error: None,
            }
        }
        Ok(Err(e)) => {
            state.finish_scan(scan_id, None);
            DoneDto {
                scan_id,
                summary: None,
                error: Some(e.to_string()),
            }
        }
        Err(_) => {
            tracing::error!(scan_id, "scan thread panicked");
            state.finish_scan(scan_id, None);
            DoneDto {
                scan_id,
                summary: None,
                error: Some("internal error while scanning".to_string()),
            }
        }
    };
    if let Err(e) = app.emit("scan://done", done) {
        tracing::warn!("emit scan://done failed: {e}");
    }
}

fn forward_event(app: &AppHandle, scan_id: u64, event: ScanEvent) {
    let result = match event {
        ScanEvent::Progress {
            files,
            dirs,
            bytes,
            current_path,
            errors,
            elapsed_ms,
        } => app.emit(
            "scan://progress",
            ProgressDto {
                scan_id,
                files,
                dirs,
                bytes,
                current_path: current_path.to_string_lossy().into_owned(),
                errors,
                elapsed_ms,
            },
        ),
        ScanEvent::Batch { top_level } => app.emit(
            "scan://batch",
            BatchDto {
                scan_id,
                top_level: top_level
                    .into_iter()
                    .map(|t| BatchEntryDto {
                        id: t.id.0,
                        category: category_for(t.kind, OsStr::new(&t.name)),
                        name: t.name,
                        kind: t.kind,
                        size_allocated: t.size_allocated,
                        size_apparent: t.size_apparent,
                        file_count: t.file_count,
                    })
                    .collect(),
            },
        ),
    };
    if let Err(e) = result {
        tracing::warn!("emit scan event failed: {e}");
    }
}

#[tauri::command]
pub async fn cancel_scan(state: State<'_, AppState>, scan_id: u64) -> CmdResult<bool> {
    state.cancel_scan(scan_id)
}

#[tauri::command]
pub async fn get_summary(state: State<'_, AppState>) -> CmdResult<SummaryDto> {
    let r = state.result()?;
    Ok(SummaryDto::new(&r.tree, r.elapsed_ms))
}

#[tauri::command]
pub async fn get_node(state: State<'_, AppState>, id: u32) -> CmdResult<NodeDetailDto> {
    let r = state.result()?;
    let tree = &r.tree;
    let nid = NodeId(id);
    let node = NodeDto::new(tree, nid).ok_or("unknown node")?;
    let path = tree
        .path(nid)
        .ok_or("unknown node")?
        .to_string_lossy()
        .into_owned();
    let ancestors = view::ancestors(tree, nid)
        .into_iter()
        .filter_map(|a| {
            tree.get(a).map(|n| CrumbDto {
                id: a.0,
                name: n.display_name(),
            })
        })
        .collect();
    Ok(NodeDetailDto {
        node,
        path,
        ancestors,
    })
}

#[tauri::command]
pub async fn get_children(
    state: State<'_, AppState>,
    id: u32,
    sort: SortKey,
    mode: SizeMode,
    offset: usize,
    limit: usize,
) -> CmdResult<ChildrenPageDto> {
    let r = state.result()?;
    let page = view::children_page(&r.tree, NodeId(id), sort, mode, offset, limit);
    Ok(ChildrenPageDto {
        total: page.total,
        offset,
        items: page
            .items
            .into_iter()
            .filter_map(|c| NodeDto::new(&r.tree, c))
            .collect(),
    })
}

#[tauri::command]
pub async fn get_treemap(
    state: State<'_, AppState>,
    id: u32,
    depth: usize,
    min_bytes: u64,
    mode: SizeMode,
    max_nodes: Option<usize>,
) -> CmdResult<TreemapDto> {
    let r = state.result()?;
    let opts = TreemapOptions {
        depth: depth.min(8),
        min_bytes,
        max_nodes: max_nodes.unwrap_or(4_000).min(MAX_VIEW_ITEMS),
        mode,
    };
    view::treemap(&r.tree, NodeId(id), opts)
        .map(TreemapDto::from)
        .ok_or_else(|| "unknown node".to_string())
}

#[tauri::command]
pub async fn get_largest(
    state: State<'_, AppState>,
    kind: LargestKind,
    limit: usize,
    mode: SizeMode,
) -> CmdResult<Vec<LargestItemDto>> {
    let r = state.result()?;
    let tree = &r.tree;
    Ok(tree
        .largest(kind, limit.min(MAX_LARGEST), mode)
        .into_iter()
        .filter_map(|id| {
            Some(LargestItemDto {
                node: NodeDto::new(tree, id)?,
                path: tree.path(id)?.to_string_lossy().into_owned(),
            })
        })
        .collect())
}

#[tauri::command]
pub async fn get_errors(
    state: State<'_, AppState>,
    offset: usize,
    limit: usize,
) -> CmdResult<ErrorsPageDto> {
    let r = state.result()?;
    let errors = r.tree.errors();
    Ok(ErrorsPageDto {
        total: errors.len(),
        items: errors
            .iter()
            .skip(offset)
            .take(limit.min(MAX_VIEW_ITEMS))
            .map(ErrorDto::from)
            .collect(),
    })
}

/// Reveals a scanned node in Explorer / Finder. Takes a node id, never a
/// path, so the frontend can only point at things the scan produced.
#[tauri::command]
pub async fn reveal_node(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    let path = node_path(&state, id)?;
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| e.to_string())
}

/// Opens a scanned node with its default application.
#[tauri::command]
pub async fn open_node(state: State<'_, AppState>, id: u32) -> CmdResult<()> {
    let path = node_path(&state, id)?;
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())
}

fn node_path(state: &State<'_, AppState>, id: u32) -> CmdResult<PathBuf> {
    let r = state.result()?;
    r.tree
        .path(NodeId(id))
        .ok_or_else(|| "unknown node".to_string())
}
