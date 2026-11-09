//! Tauri commands. Each is a thin adapter: look up state, call `hoard-core`,
//! convert to DTOs. All are `async` so they run off the main (UI) thread.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use hoard_core::classify::category_for;
use hoard_core::cleanup::{
    required_confirmation, run_cleanup, CleanupContext, CleanupMode, SystemRemover, Tray,
};
use hoard_core::rules::{suggest, MatchContext, RuleSet, Safety};
use hoard_core::view::{self, TreemapOptions, MAX_VIEW_ITEMS};
use hoard_core::{LargestKind, NodeId, ScanEvent, ScanOptions, SizeMode, SortKey, Tree};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::dto::SuggestionsDto;
use crate::dto::{
    BatchDto, BatchEntryDto, ChildrenPageDto, CleanupProgressDto, CleanupReportDto, CrumbDto,
    DiskDto, DoneDto, ErrorDto, ErrorsPageDto, LargestItemDto, NodeDetailDto, NodeDto, ProgressDto,
    ScanOptsDto, SummaryDto, TrayDto, TreemapDto,
};
use crate::state::{describe_add, AppState};

/// Upper bound for `get_largest`.
const MAX_LARGEST: usize = 1_000;

type CmdResult<T> = Result<T, String>;

// ------------------------------------------------------------------ disks, scan

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
/// is cancelled. Refused while a cleanup runs. Emits `scan://progress`,
/// `scan://batch` and `scan://done`.
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
            if !state.finish_scan(scan_id, Some(tree), elapsed_ms) {
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
            state.finish_scan(scan_id, None, elapsed_ms);
            DoneDto {
                scan_id,
                summary: None,
                error: Some(e.to_string()),
            }
        }
        Err(_) => {
            tracing::error!(scan_id, "scan thread panicked");
            state.finish_scan(scan_id, None, elapsed_ms);
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

// ------------------------------------------------------------------ queries

#[tauri::command]
pub async fn get_summary(state: State<'_, AppState>) -> CmdResult<SummaryDto> {
    let r = state.result()?;
    let tree = r.read()?;
    Ok(SummaryDto::new(&tree, r.elapsed_ms))
}

#[tauri::command]
pub async fn get_node(state: State<'_, AppState>, id: u32) -> CmdResult<NodeDetailDto> {
    let r = state.result()?;
    let tree = r.read()?;
    let nid = NodeId(id);
    let node = NodeDto::new(&tree, nid).ok_or("unknown node")?;
    let path = tree
        .path(nid)
        .ok_or("unknown node")?
        .to_string_lossy()
        .into_owned();
    let ancestors = view::ancestors(&tree, nid)
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
    let tree = r.read()?;
    let page = view::children_page(&tree, NodeId(id), sort, mode, offset, limit);
    Ok(ChildrenPageDto {
        total: page.total,
        offset,
        items: page
            .items
            .into_iter()
            .filter_map(|c| NodeDto::new(&tree, c))
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
    let tree = r.read()?;
    let opts = TreemapOptions {
        depth: depth.min(8),
        min_bytes,
        max_nodes: max_nodes.unwrap_or(4_000).min(MAX_VIEW_ITEMS),
        mode,
    };
    view::treemap(&tree, NodeId(id), opts)
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
    let tree = r.read()?;
    Ok(tree
        .largest(kind, limit.min(MAX_LARGEST), mode)
        .into_iter()
        .filter_map(|id| {
            Some(LargestItemDto {
                node: NodeDto::new(&tree, id)?,
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
    let tree = r.read()?;
    let errors = tree.errors();
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
    let tree = r.read()?;
    tree.path(NodeId(id))
        .ok_or_else(|| "unknown node".to_string())
}

// ------------------------------------------------------------------ tray

fn cleanup_context(app: &AppHandle, tree: &Tree) -> CleanupContext {
    CleanupContext::for_current_user(tree.root_path().to_path_buf(), action_log_path(app))
}

/// `<app data dir>/actions.jsonl`, falling back to the OS data dir.
/// Debug builds honour `HOARD_ACTION_LOG` so automated runs never write to
/// the real log.
fn action_log_path(app: &AppHandle) -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("HOARD_ACTION_LOG") {
        return PathBuf::from(p);
    }
    app.path()
        .app_data_dir()
        .map(|d| d.join("actions.jsonl"))
        .ok()
        .or_else(CleanupContext::default_log_path)
        .unwrap_or_else(|| PathBuf::from("hoard-actions.jsonl"))
}

/// Adds nodes to the tray. Returns the whole tray (versioned) plus notes on
/// anything covered, replaced or rejected.
#[tauri::command]
pub async fn tray_add(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<u32>,
) -> CmdResult<TrayDto> {
    let ids: Vec<NodeId> = ids.into_iter().map(NodeId).collect();
    state.with_tray(
        |tray, tree, ctx| {
            let out = tray.add(tree, &ids, ctx);
            tray_dto(tray, tree, ctx, describe_add(&out, tree))
        },
        |tree| cleanup_context(&app, tree),
    )
}

#[tauri::command]
pub async fn tray_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<u32>,
) -> CmdResult<TrayDto> {
    let ids: Vec<NodeId> = ids.into_iter().map(NodeId).collect();
    state.with_tray(
        |tray, tree, ctx| {
            tray.remove(&ids);
            tray_dto(tray, tree, ctx, Vec::new())
        },
        |tree| cleanup_context(&app, tree),
    )
}

#[tauri::command]
pub async fn tray_clear(app: AppHandle, state: State<'_, AppState>) -> CmdResult<TrayDto> {
    state.with_tray(
        |tray, tree, ctx| {
            tray.clear();
            tray_dto(tray, tree, ctx, Vec::new())
        },
        |tree| cleanup_context(&app, tree),
    )
}

#[tauri::command]
pub async fn tray_list(app: AppHandle, state: State<'_, AppState>) -> CmdResult<TrayDto> {
    let (tray, result) = state.tray()?;
    match result {
        Some(r) => {
            let tree = r.read()?;
            let ctx = cleanup_context(&app, &tree);
            Ok(tray_dto(&tray, &tree, &ctx, Vec::new()))
        }
        None => Ok(TrayDto::empty(&tray)),
    }
}

fn tray_dto(tray: &Tray, tree: &Tree, ctx: &CleanupContext, notes: Vec<String>) -> TrayDto {
    let confirmation = required_confirmation(tree, tray.items(), CleanupMode::Permanent, ctx);
    TrayDto::new(tray, tree, notes, confirmation)
}

// ------------------------------------------------------------------ suggestions

fn builtin_rules() -> CmdResult<&'static RuleSet> {
    static RULES: OnceLock<Result<RuleSet, String>> = OnceLock::new();
    RULES
        .get_or_init(|| RuleSet::builtin().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

/// Reclaimable space found by the built-in rules in the current scan.
#[tauri::command]
pub async fn get_suggestions(state: State<'_, AppState>) -> CmdResult<SuggestionsDto> {
    let rules = builtin_rules()?;
    let r = state.result()?;
    let tree = r.read()?;
    let groups = suggest(&tree, rules, &MatchContext::current());
    Ok(SuggestionsDto::new(&tree, &groups))
}

/// Adds suggestions to the tray. `rule_id = None` means every Safe item.
/// Only Safe, addable rules can be added in bulk (PRD R3); Review and
/// Caution items must be added one by one with `tray_add`.
#[tauri::command]
pub async fn add_suggestions_to_tray(
    app: AppHandle,
    state: State<'_, AppState>,
    rule_id: Option<String>,
) -> CmdResult<TrayDto> {
    let rules = builtin_rules()?;
    if let Some(id) = &rule_id {
        let rule = rules.get(id).ok_or("unknown rule")?;
        if rule.safety != Safety::Safe || !rule.addable {
            return Err(format!(
                "{} items must be reviewed and added one at a time",
                rule.name
            ));
        }
    }
    state.with_tray(
        |tray, tree, ctx| {
            let ids: Vec<NodeId> = suggest(tree, rules, &MatchContext::current())
                .iter()
                .filter(|g| g.rule.safety == Safety::Safe && g.rule.addable)
                .filter(|g| rule_id.as_deref().is_none_or(|id| g.rule.id == id))
                .flat_map(|g| g.items.iter().map(|s| s.node))
                .collect();
            let out = tray.add(tree, &ids, ctx);
            tray_dto(tray, tree, ctx, describe_add(&out, tree))
        },
        |tree| cleanup_context(&app, tree),
    )
}

// ------------------------------------------------------------------ cleanup

/// Runs the tray through the safety pipeline and returns the full report.
///
/// The caller supplies `run_id` before calling, so every
/// `cleanup://progress` event carries an id the UI already knows; the
/// result arrives as this command's return value, not as an event.
#[tauri::command]
pub async fn execute_cleanup(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    mode: CleanupMode,
    confirmation: Option<String>,
) -> CmdResult<CleanupReportDto> {
    let start = state.begin_cleanup(&run_id)?;
    let ctx = {
        let tree = start.result.read();
        match tree {
            Ok(t) => cleanup_context(&app, &t),
            Err(e) => {
                state.finish_cleanup(&run_id);
                return Err(e);
            }
        }
    };
    let worker_app = app.clone();
    let worker_run_id = run_id.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let mut on_progress = |p: hoard_core::cleanup::CleanupProgress| {
            let _ = worker_app.emit(
                "cleanup://progress",
                CleanupProgressDto {
                    run_id: worker_run_id.clone(),
                    done: p.done,
                    total: p.total,
                    current_path: p.current.to_string_lossy().into_owned(),
                },
            );
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_cleanup(
                &start.result.tree,
                &start.items,
                mode,
                confirmation.as_deref(),
                &ctx,
                &SystemRemover,
                &worker_run_id,
                &start.cancel,
                &mut on_progress,
            )
        }))
    })
    .await;
    // Always leave the busy state, whatever happened.
    state.finish_cleanup(&run_id);
    let _ = app.emit("cleanup://done", run_id.clone());

    match joined {
        Ok(Ok(Ok(report))) => Ok(CleanupReportDto::from(&report)),
        Ok(Ok(Err(e))) => Err(e.to_string()),
        Ok(Err(_)) | Err(_) => {
            tracing::error!(run_id, "cleanup worker panicked");
            Err("internal error during cleanup; see the action log for what was done".into())
        }
    }
}

#[tauri::command]
pub async fn cancel_cleanup(state: State<'_, AppState>, run_id: String) -> CmdResult<bool> {
    state.cancel_cleanup(&run_id)
}
