//! Phase V task execution orchestration.
//!
//! This module owns the bridge between the database, git worktrees, and the
//! agent runner. The runner remains process/protocol focused; this module is
//! responsible for task state, turn rows, event forwarding, and cleanup.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::{
    commands::DbState,
    db,
    error::AppError,
    models::Task,
    runner::{AgentRunner, CodexExecRunner, EventStream, RunHandle},
    worktree::WorktreeManager,
};

fn now() -> String {
    Utc::now().to_rfc3339()
}

struct ActiveRun {
    handle: Option<RunHandle>,
    cancelled: Arc<AtomicBool>,
}

/// Managed execution state. The default limit is intentionally conservative
/// because each task can launch Codex and its own command subprocesses.
#[derive(Clone)]
pub struct RunState {
    active: Arc<Mutex<HashMap<String, ActiveRun>>>,
    max_concurrent: usize,
}

impl Default for RunState {
    fn default() -> Self {
        Self::new(2)
    }
}

impl RunState {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
            max_concurrent: max_concurrent.max(1),
        }
    }

    fn reserve(&self, task_id: &str) -> Result<(), AppError> {
        let mut active = self.active.lock().map_err(|_| {
            AppError::InvalidOperation("Execution state is unavailable".to_string())
        })?;
        if active.contains_key(task_id) {
            return Err(AppError::InvalidOperation(format!(
                "Task '{task_id}' is already running"
            )));
        }
        if active.len() >= self.max_concurrent {
            return Err(AppError::InvalidOperation(format!(
                "Maximum of {} concurrent tasks is already running",
                self.max_concurrent
            )));
        }
        // The reservation prevents another run command from using this slot
        // while setup is creating the worktree and turn row.
        active.insert(
            task_id.to_string(),
            ActiveRun {
                handle: None,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        );
        Ok(())
    }

    fn register(&self, task_id: &str, handle: RunHandle) -> Result<(), AppError> {
        let mut active = self.active.lock().map_err(|_| {
            AppError::InvalidOperation("Execution state is unavailable".to_string())
        })?;
        let Some(run) = active.get_mut(task_id) else {
            return Err(AppError::InvalidOperation(format!(
                "Task '{task_id}' lost its execution slot"
            )));
        };
        run.handle = Some(handle);
        Ok(())
    }

    fn release(&self, task_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(task_id);
        }
    }

    fn take(&self, task_id: &str) -> Option<ActiveRun> {
        self.active.lock().ok()?.remove(task_id)
    }

    pub fn cancel(&self, task_id: &str) -> Result<(), AppError> {
        let (handle, cancelled) = {
            let active = self.active.lock().map_err(|_| {
                AppError::InvalidOperation("Execution state is unavailable".to_string())
            })?;
            let Some(run) = active.get(task_id) else {
                return Err(AppError::NotFound(format!(
                    "No active run for task '{task_id}'"
                )));
            };
            let Some(handle) = run.handle.clone() else {
                return Err(AppError::InvalidOperation(format!(
                    "Task '{task_id}' is still being prepared"
                )));
            };
            (handle, Arc::clone(&run.cancelled))
        };
        cancelled.store(true, Ordering::SeqCst);
        handle.cancel();
        Ok(())
    }
}

/// Payload emitted on task:{task_id}:event for both Codex events and the
/// final Forge state update. raw is retained so the UI can evolve without
/// changing the runner contract.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEventPayload {
    pub task_id: String,
    pub turn_id: String,
    pub event_type: String,
    pub raw: serde_json::Value,
    pub status: Option<String>,
    pub diff: Option<String>,
    pub error: Option<String>,
}

struct PreparedRun {
    task_id: String,
    turn_id: String,
    prompt: String,
    base_ref: String,
    repo_root: PathBuf,
    worktree_path: PathBuf,
    branch_name: String,
    log_path: PathBuf,
}

/// Start an initial turn for a draft task and return its persisted running
/// representation. The actual stream consumer runs on a background thread.
pub fn start_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    codex_bin: Option<String>,
) -> Result<Task, AppError> {
    run_state.reserve(&task_id)?;

    let prepared = match prepare_run(app, &task_id) {
        Ok(prepared) => prepared,
        Err(error) => {
            run_state.release(&task_id);
            return Err(error);
        }
    };

    let runner = CodexExecRunner::new(codex_bin.unwrap_or_else(|| "codex".to_string()));
    let (handle, stream) = match runner.start_turn(
        &prepared.worktree_path,
        &prepared.prompt,
        &prepared.log_path,
    ) {
        Ok(result) => result,
        Err(error) => {
            let wm = WorktreeManager::default();
            let _ = wm.remove_worktree(
                &prepared.repo_root,
                &prepared.worktree_path,
                &prepared.branch_name,
            );
            if let Some(db_state) = app.try_state::<DbState>() {
                if let Ok(conn) = db_state.0.lock() {
                    let _ = db::finish_turn(&conn, &prepared.turn_id, "failed", &now());
                    let _ = db::finish_task(&conn, &prepared.task_id, "failed", "", &now());
                }
            }
            run_state.release(&task_id);
            return Err(error);
        }
    };

    if let Err(error) = run_state.register(&task_id, handle.clone()) {
        handle.cancel();
        run_state.release(&task_id);
        return Err(error);
    }

    let worker_app = app.clone();
    let worker_state = run_state.clone();
    std::thread::spawn(move || {
        consume_turn(worker_app, worker_state, prepared, handle, stream);
    });

    let db_state = app.state::<DbState>();
    let conn = db_state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' disappeared after run setup")))
}

pub fn cancel_task(run_state: &RunState, task_id: &str) -> Result<(), AppError> {
    run_state.cancel(task_id)
}

fn prepare_run(app: &AppHandle, task_id: &str) -> Result<PreparedRun, AppError> {
    let db_state = app.state::<DbState>();
    let (task, project) = {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        let task = db::select_task(&conn, task_id)?
            .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
        if task.status != "draft" {
            return Err(AppError::InvalidOperation(format!(
                "Task '{task_id}' has status '{}'; only draft tasks can be started",
                task.status
            )));
        }
        let project = db::select_project(&conn, &task.project_id)?.ok_or_else(|| {
            AppError::NotFound(format!("Project '{}' not found", task.project_id))
        })?;
        (task, project)
    };

    let repo_root = PathBuf::from(project.root_path);
    let wm = WorktreeManager::default();
    if !wm.is_git_repo(&repo_root) {
        return Err(AppError::InvalidOperation(format!(
            "Project '{}' is no longer a git repository",
            repo_root.display()
        )));
    }
    let base_ref = wm.head_sha(&repo_root)?;
    let app_data_dir = app.path().app_data_dir().map_err(|e| {
        AppError::InvalidOperation(format!("Cannot resolve app data directory: {e}"))
    })?;
    let worktree_path = app_data_dir.join("worktrees").join(task_id);
    if worktree_path.exists() {
        return Err(AppError::InvalidOperation(format!(
            "Worktree '{}' already exists; clean up the previous run before retrying",
            worktree_path.display()
        )));
    }
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::InvalidOperation(format!("Cannot create worktree directory: {e}"))
        })?;
    }

    let branch_name = format!("task/{task_id}");
    wm.add_worktree(&repo_root, &worktree_path, &branch_name, &base_ref)?;

    let prompt = build_prompt(&task.prompt, &task.file_refs);
    let turn_id = Uuid::new_v4().to_string();
    let log_path = app_data_dir
        .join("logs")
        .join(task_id)
        .join(format!("{turn_id}.jsonl"));
    let now_str = now();
    let persistence = (|| {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::prepare_task_run(
            &conn,
            task_id,
            &base_ref,
            &worktree_path.to_string_lossy(),
            &branch_name,
            &now_str,
        )?;
        let turn_result = db::insert_turn(
            &conn,
            &turn_id,
            task_id,
            "initial",
            &prompt,
            "running",
            &log_path.to_string_lossy(),
            &now_str,
        );
        if turn_result.is_err() {
            let _ = db::reset_task_run(&conn, task_id, &now());
        }
        turn_result
    })();
    if let Err(error) = persistence {
        let _ = wm.remove_worktree(&repo_root, &worktree_path, &branch_name);
        return Err(error);
    }

    Ok(PreparedRun {
        task_id: task_id.to_string(),
        turn_id,
        prompt,
        base_ref,
        repo_root,
        worktree_path,
        branch_name,
        log_path,
    })
}

fn build_prompt(prompt: &str, file_refs: &[String]) -> String {
    if file_refs.is_empty() {
        return prompt.to_string();
    }
    format!(
        "{prompt}\n\nReferenced project paths (relative to the repository root):\n{}",
        file_refs
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn consume_turn(
    app: AppHandle,
    run_state: RunState,
    prepared: PreparedRun,
    _handle: RunHandle,
    stream: EventStream,
) {
    let event_name = format!("task:{}:event", prepared.task_id);
    let mut terminal_type: Option<String> = None;
    let mut terminal_error: Option<String> = None;

    for event in stream {
        if let Some(thread_id) = event.thread_id() {
            if let Some(db_state) = app.try_state::<DbState>() {
                if let Ok(conn) = db_state.0.lock() {
                    let _ = db::set_task_thread_id(&conn, &prepared.task_id, thread_id, &now());
                }
            }
        }
        let is_terminal = event.is_terminal();
        if is_terminal {
            terminal_type = Some(event.event_type.clone());
            terminal_error = event.error_message();
        }
        emit_event(
            &app,
            &event_name,
            TaskEventPayload {
                task_id: prepared.task_id.clone(),
                turn_id: prepared.turn_id.clone(),
                event_type: event.event_type,
                raw: event.raw,
                status: None,
                diff: None,
                error: terminal_error.clone(),
            },
        );
    }

    if let Some(active) = run_state.take(&prepared.task_id) {
        if let Some(handle) = active.handle {
            handle.wait();
        }
        let cancelled = active.cancelled.load(Ordering::SeqCst);
        let (task_status, turn_status, error) = if cancelled {
            ("cancelled", "cancelled", None)
        } else if terminal_type.as_deref() == Some("turn.completed") {
            ("awaiting_review", "completed", None)
        } else if terminal_type.as_deref() == Some("turn.failed") {
            (
                "failed",
                "failed",
                terminal_error.or_else(|| Some("Codex reported a failed turn".to_string())),
            )
        } else {
            (
                "failed",
                "failed",
                Some("Codex exited without a terminal turn event".to_string()),
            )
        };

        let wm = WorktreeManager::default();
        let diff_result = wm.diff(&prepared.worktree_path, &prepared.base_ref);
        let (diff, diff_error) = match diff_result {
            Ok(diff) => (diff, None),
            Err(error) => (String::new(), Some(error.to_string())),
        };
        let final_error = error.or(diff_error);

        if let Some(db_state) = app.try_state::<DbState>() {
            if let Ok(conn) = db_state.0.lock() {
                let _ = db::finish_turn(&conn, &prepared.turn_id, turn_status, &now());
                let _ = db::finish_task(&conn, &prepared.task_id, task_status, &diff, &now());
            }
        }

        emit_event(
            &app,
            &event_name,
            TaskEventPayload {
                task_id: prepared.task_id.clone(),
                turn_id: prepared.turn_id.clone(),
                event_type: "forge.task.updated".to_string(),
                raw: serde_json::json!({ "status": task_status, "diff": diff.clone() }),
                status: Some(task_status.to_string()),
                diff: Some(diff),
                error: final_error,
            },
        );
    }
}

fn emit_event(app: &AppHandle, event_name: &str, payload: TaskEventPayload) {
    let _ = app.emit(event_name, payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_enforces_default_concurrency_limit() {
        let state = RunState::default();
        state.reserve("one").unwrap();
        state.reserve("two").unwrap();
        let error = state.reserve("three").unwrap_err();
        assert!(error.to_string().contains("Maximum of 2"));
    }

    #[test]
    fn run_state_rejects_duplicate_task_reservation() {
        let state = RunState::new(2);
        state.reserve("same-task").unwrap();
        let error = state.reserve("same-task").unwrap_err();
        assert!(error.to_string().contains("already running"));
    }
}
