#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod db;
mod editors;
mod error;
mod execution;
mod models;
mod naming;
pub mod runner;
pub mod worktree;

use chrono::Utc;
use commands::DbState;
use execution::RunState;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("Failed to resolve app data directory");
            std::fs::create_dir_all(&app_data_dir).expect("Failed to create app data directory");
            let db_path = app_data_dir.join("forge.db");
            let conn = db::open(&db_path).expect("Failed to open database");
            let settings = db::select_settings(&conn).expect("Failed to load settings");
            // Recover any tasks that were left in `running` state by a
            // previous crash or forced-quit. The in-memory RunState is always
            // empty at this point, so those tasks have no live process.
            let now = Utc::now().to_rfc3339();
            let recovered = db::mark_dangling_tasks_failed(&conn, &now)
                .expect("Failed to recover dangling tasks");
            if recovered > 0 {
                eprintln!("[forge] Recovered {recovered} dangling task(s) → failed");
            }
            app.manage(RunState::new(settings.max_concurrent_tasks));
            app.manage(commands::AgentCatalogState::default());
            app.manage(DbState(Mutex::new(conn)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_project,
            commands::list_projects,
            commands::list_project_dir,
            commands::search_project_files,
            commands::create_task,
            commands::list_tasks,
            commands::get_task,
            commands::delete_task,
            commands::update_task_prompt,
            commands::check_codex_health,
            commands::get_settings,
            commands::get_agent_models,
            commands::list_agent_catalogs,
            commands::update_settings,
            commands::check_system_health,
            commands::run_task,
            commands::retry_task,
            commands::cancel_task,
            commands::add_review_comment,
            commands::resolve_review_comment,
            commands::assign_review_comment,
            commands::list_review_comments,
            commands::submit_review,
            commands::list_editors,
            commands::open_worktree_in_editor,
            commands::confirm_task,
            commands::list_task_turns,
            commands::get_turn_output,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AI Workflow Automation");
}
