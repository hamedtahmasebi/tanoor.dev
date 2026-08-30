#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod db;
mod error;
mod execution;
mod models;
pub mod runner;
pub mod worktree;

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
            app.manage(RunState::new(settings.max_concurrent_tasks));
            app.manage(DbState(Mutex::new(conn)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_project,
            commands::list_projects,
            commands::create_task,
            commands::list_tasks,
            commands::get_task,
            commands::delete_task,
            commands::update_task_prompt,
            commands::check_codex_health,
            commands::get_settings,
            commands::update_settings,
            commands::check_system_health,
            commands::run_task,
            commands::cancel_task,
            commands::add_review_comment,
            commands::resolve_review_comment,
            commands::list_review_comments,
            commands::request_changes,
            commands::confirm_task,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AI Workflow Automation");
}
