//! Tauri shell for Hoard: IPC commands, DTOs and app state only.
//! All scanning and query logic lives in `hoard-core`.

mod commands;
mod dto;
mod state;

/// Starts the desktop app.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("HOARD_LOG"))
        .init();

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_disks,
            commands::home_dir,
            commands::start_scan,
            commands::cancel_scan,
            commands::get_summary,
            commands::get_node,
            commands::get_children,
            commands::get_treemap,
            commands::get_largest,
            commands::get_errors,
            commands::reveal_node,
            commands::open_node,
            commands::tray_add,
            commands::tray_remove,
            commands::tray_clear,
            commands::tray_list,
            commands::get_suggestions,
            commands::add_suggestions_to_tray,
            commands::execute_cleanup,
            commands::cancel_cleanup,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!("tauri exited with an error: {e}");
        std::process::exit(1);
    }
}
