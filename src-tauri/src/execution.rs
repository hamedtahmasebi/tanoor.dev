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
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::{
    commands::DbState,
    db,
    error::AppError,
    models::{ReviewComment, Task},
    runner::{AgentRunner, CodexExecRunner, EventStream, RunHandle},
    worktree::WorktreeManager,
};

/// Global companion channel used by the frontend to keep non-selected tasks
/// synchronized. Task-scoped channels remain available to focused consumers.
pub const TASK_EVENT_CHANNEL: &str = "forge:task-event";

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn load_settings(app: &AppHandle) -> Result<crate::models::AppSettings, AppError> {
    let state = app.state::<DbState>();
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::select_settings(&conn)
}

struct ActiveRun {
    handle: Option<RunHandle>,
    cancelled: Arc<AtomicBool>,
    counts_toward_limit: bool,
}

/// Managed execution state. The default limit is intentionally conservative
/// because each task can launch Codex and its own command subprocesses.
#[derive(Clone)]
pub struct RunState {
    active: Arc<Mutex<HashMap<String, ActiveRun>>>,
    max_concurrent: Arc<AtomicUsize>,
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
            max_concurrent: Arc::new(AtomicUsize::new(max_concurrent.max(1))),
        }
    }

    pub fn set_max_concurrent(&self, max_concurrent: usize) {
        self.max_concurrent
            .store(max_concurrent.max(1), Ordering::SeqCst);
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
        let running_count = active
            .values()
            .filter(|run| run.counts_toward_limit)
            .count();
        let max_concurrent = self.max_concurrent.load(Ordering::SeqCst);
        if running_count >= max_concurrent {
            return Err(AppError::InvalidOperation(format!(
                "Maximum of {} concurrent tasks is already running",
                max_concurrent
            )));
        }
        // The reservation prevents another run command from using this slot
        // while setup is creating the worktree and turn row.
        active.insert(
            task_id.to_string(),
            ActiveRun {
                handle: None,
                cancelled: Arc::new(AtomicBool::new(false)),
                counts_toward_limit: true,
            },
        );
        Ok(())
    }

    fn reserve_exclusive(&self, task_id: &str) -> Result<(), AppError> {
        let mut active = self.active.lock().map_err(|_| {
            AppError::InvalidOperation("Execution state is unavailable".to_string())
        })?;
        if active.contains_key(task_id) {
            return Err(AppError::InvalidOperation(format!(
                "Task '{task_id}' already has an operation in progress"
            )));
        }
        active.insert(
            task_id.to_string(),
            ActiveRun {
                handle: None,
                cancelled: Arc::new(AtomicBool::new(false)),
                counts_toward_limit: false,
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
/// final Tanoor state update. raw is retained so the UI can evolve without
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
    git_bin: String,
}

struct PreparedFollowUp {
    run: PreparedRun,
    thread_id: String,
    comment_ids: Vec<String>,
}

/// Start an initial turn for a draft task and return its persisted running
/// representation. The actual stream consumer runs on a background thread.
pub fn start_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    run_state.reserve(&task_id)?;

    let prepared = match prepare_run(app, &task_id, &settings.git_bin) {
        Ok(prepared) => prepared,
        Err(error) => {
            run_state.release(&task_id);
            return Err(error);
        }
    };

    let runner = CodexExecRunner {
        codex_bin: settings.codex_bin,
        model: settings.codex_model,
        effort: Some(settings.codex_effort),
        ..Default::default()
    };
    let (handle, stream) = match runner.start_turn(
        &prepared.worktree_path,
        &prepared.prompt,
        &prepared.log_path,
    ) {
        Ok(result) => result,
        Err(error) => {
            let wm = WorktreeManager::new(&prepared.git_bin);
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

/// Start a follow-up turn on the task's existing Codex thread. Unresolved
/// comments are retired only after the resume process has started, so a launch
/// failure can be retried without losing review feedback.
pub fn start_follow_up(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    reviewer_note: Option<String>,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    run_state.reserve(&task_id)?;

    let prepared =
        match prepare_follow_up(app, &task_id, reviewer_note.as_deref(), &settings.git_bin) {
            Ok(prepared) => prepared,
            Err(error) => {
                run_state.release(&task_id);
                return Err(error);
            }
        };

    let runner = CodexExecRunner {
        codex_bin: settings.codex_bin,
        model: settings.codex_model,
        effort: Some(settings.codex_effort),
        ..Default::default()
    };
    let (handle, stream) = match runner.resume_turn(
        &prepared.run.worktree_path,
        &prepared.thread_id,
        &prepared.run.prompt,
        &prepared.run.log_path,
    ) {
        Ok(result) => result,
        Err(error) => {
            mark_follow_up_start_failed(app, &prepared.run.task_id, &prepared.run.turn_id);
            run_state.release(&task_id);
            return Err(error);
        }
    };

    if let Err(error) = run_state.register(&task_id, handle.clone()) {
        handle.cancel();
        mark_follow_up_start_failed(app, &prepared.run.task_id, &prepared.run.turn_id);
        run_state.release(&task_id);
        return Err(error);
    }

    let comment_result = app
        .state::<DbState>()
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))
        .and_then(|conn| {
            db::resolve_review_comments(&conn, &prepared.run.task_id, &prepared.comment_ids)
        });
    if let Err(error) = comment_result {
        handle.cancel();
        run_state.take(&task_id);
        mark_follow_up_start_failed(app, &prepared.run.task_id, &prepared.run.turn_id);
        return Err(error);
    }

    let worker_app = app.clone();
    let worker_state = run_state.clone();
    std::thread::spawn(move || {
        consume_turn(worker_app, worker_state, prepared.run, handle, stream);
    });

    let db_state = app.state::<DbState>();
    let conn = db_state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::select_task(&conn, &task_id)?.ok_or_else(|| {
        AppError::NotFound(format!(
            "Task '{task_id}' disappeared after follow-up setup"
        ))
    })
}

/// Commit a reviewed task and optionally merge it into the repository's
/// current branch. Git operations complete before the approved state is
/// persisted, keeping failures reviewable and retryable.
pub fn confirm_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: &str,
    merge: bool,
) -> Result<Task, AppError> {
    run_state.reserve_exclusive(task_id)?;
    let result = confirm_task_inner(app, task_id, merge);
    run_state.release(task_id);
    result
}

fn confirm_task_inner(app: &AppHandle, task_id: &str, merge: bool) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    let db_state = app.state::<DbState>();
    // Retain the DB guard through the Git sequence. Together with the run-state
    // reservation this prevents comments or a resumed turn from racing the
    // approval decision after validation.
    let conn = db_state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    let task = db::select_task(&conn, task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
    let project = db::select_project(&conn, &task.project_id)?
        .ok_or_else(|| AppError::NotFound(format!("Project '{}' not found", task.project_id)))?;
    let unresolved_count = db::select_unresolved_review_comments(&conn, task_id)?.len();

    if task.status != "awaiting_review" {
        return Err(AppError::InvalidOperation(format!(
            "Task '{task_id}' has status '{}'; only tasks awaiting review can be confirmed",
            task.status
        )));
    }
    if unresolved_count > 0 {
        return Err(AppError::InvalidOperation(format!(
            "Resolve or submit all review comments before confirming ({unresolved_count} unresolved)"
        )));
    }

    let repo_root = PathBuf::from(project.root_path);
    let worktree_path = task
        .worktree_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| {
            AppError::InvalidOperation(format!("Task '{task_id}' has no worktree to confirm"))
        })?;
    let branch_name = task.branch_name.as_deref().ok_or_else(|| {
        AppError::InvalidOperation(format!("Task '{task_id}' has no branch to confirm"))
    })?;
    if !worktree_path.exists() {
        return Err(AppError::InvalidOperation(format!(
            "Task worktree '{}' is missing; confirmation cannot continue",
            worktree_path.display()
        )));
    }

    let wm = WorktreeManager::new(settings.git_bin);
    let title = task.title.trim();
    let commit_message = if title.is_empty() {
        format!("Tanoor: approve task {task_id}")
    } else {
        format!("Tanoor: {title}")
    };
    wm.commit_all(&worktree_path, &commit_message)?;

    if merge {
        wm.merge_branch(&repo_root, branch_name)?;
        wm.remove_worktree(&repo_root, &worktree_path, branch_name)
            .map_err(|error| cleanup_error(&worktree_path, error))?;
    } else {
        wm.remove_worktree_keep_branch(&repo_root, &worktree_path)
            .map_err(|error| cleanup_error(&worktree_path, error))?;
    }

    db::approve_task(&conn, task_id, !merge, &now())?;
    db::select_task(&conn, task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' disappeared after approval")))
}

pub fn cancel_task(run_state: &RunState, task_id: &str) -> Result<(), AppError> {
    run_state.cancel(task_id)
}

fn prepare_run(app: &AppHandle, task_id: &str, git_bin: &str) -> Result<PreparedRun, AppError> {
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
    let wm = WorktreeManager::new(git_bin);
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
        git_bin: git_bin.to_string(),
    })
}

fn prepare_follow_up(
    app: &AppHandle,
    task_id: &str,
    reviewer_note: Option<&str>,
    git_bin: &str,
) -> Result<PreparedFollowUp, AppError> {
    let db_state = app.state::<DbState>();
    let (task, project, comments) = {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        let task = db::select_task(&conn, task_id)?
            .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
        if !matches!(
            task.status.as_str(),
            "awaiting_review" | "changes_requested"
        ) {
            return Err(AppError::InvalidOperation(format!(
                "Task '{task_id}' has status '{}'; it cannot accept review feedback",
                task.status
            )));
        }
        let project = db::select_project(&conn, &task.project_id)?.ok_or_else(|| {
            AppError::NotFound(format!("Project '{}' not found", task.project_id))
        })?;
        let comments = db::select_unresolved_review_comments(&conn, task_id)?;
        (task, project, comments)
    };

    let prompt = build_follow_up_prompt(&comments, reviewer_note)?;
    let base_ref = task.base_ref.ok_or_else(|| {
        AppError::InvalidOperation(format!("Task '{task_id}' has no base revision"))
    })?;
    let worktree_path = task
        .worktree_path
        .map(PathBuf::from)
        .ok_or_else(|| AppError::InvalidOperation(format!("Task '{task_id}' has no worktree")))?;
    let branch_name = task
        .branch_name
        .ok_or_else(|| AppError::InvalidOperation(format!("Task '{task_id}' has no branch")))?;
    let thread_id = task.agent_thread_id.ok_or_else(|| {
        AppError::InvalidOperation(format!("Task '{task_id}' has no Codex thread to resume"))
    })?;
    if !worktree_path.exists() {
        return Err(AppError::InvalidOperation(format!(
            "Task worktree '{}' is missing; review feedback cannot be sent",
            worktree_path.display()
        )));
    }

    let repo_root = PathBuf::from(project.root_path);
    let app_data_dir = app.path().app_data_dir().map_err(|e| {
        AppError::InvalidOperation(format!("Cannot resolve app data directory: {e}"))
    })?;
    let turn_id = Uuid::new_v4().to_string();
    let log_path = app_data_dir
        .join("logs")
        .join(task_id)
        .join(format!("{turn_id}.jsonl"));
    let now_str = now();
    {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::begin_follow_up(
            &conn,
            task_id,
            &turn_id,
            &prompt,
            &log_path.to_string_lossy(),
            &now_str,
        )?;
    }

    Ok(PreparedFollowUp {
        run: PreparedRun {
            task_id: task_id.to_string(),
            turn_id,
            prompt,
            base_ref,
            repo_root,
            worktree_path,
            branch_name,
            log_path,
            git_bin: git_bin.to_string(),
        },
        thread_id,
        comment_ids: comments.into_iter().map(|comment| comment.id).collect(),
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

fn build_follow_up_prompt(
    comments: &[ReviewComment],
    reviewer_note: Option<&str>,
) -> Result<String, AppError> {
    let reviewer_note = reviewer_note.map(str::trim).filter(|note| !note.is_empty());
    if comments.is_empty() && reviewer_note.is_none() {
        return Err(AppError::InvalidOperation(
            "Add an unresolved review comment or reviewer note before requesting changes"
                .to_string(),
        ));
    }

    let comments = comments
        .iter()
        .map(|comment| {
            serde_json::json!({
                "filePath": comment.file_path,
                "lineNumber": comment.line_number,
                "side": comment.side,
                "body": comment.body,
            })
        })
        .collect::<Vec<_>>();
    let feedback = serde_json::json!({
        "comments": comments,
        "reviewerNote": reviewer_note,
    });
    Ok(format!(
        "Address the following review feedback in the current task. Make the requested changes, verify the result, and keep all work inside the existing repository.\n\n{}",
        serde_json::to_string_pretty(&feedback)
            .expect("review feedback JSON contains only serializable values")
    ))
}

fn mark_follow_up_start_failed(app: &AppHandle, task_id: &str, turn_id: &str) {
    if let Some(db_state) = app.try_state::<DbState>() {
        if let Ok(conn) = db_state.0.lock() {
            let _ = db::fail_follow_up_start(&conn, task_id, turn_id, &now());
        }
    }
}

fn cleanup_error(worktree_path: &std::path::Path, error: AppError) -> AppError {
    AppError::Git(format!(
        "Changes were committed, but Tanoor could not remove worktree '{}'. The task remains awaiting review and confirmation can be retried. {error}",
        worktree_path.display()
    ))
}

fn consume_turn(
    app: AppHandle,
    run_state: RunState,
    prepared: PreparedRun,
    handle: RunHandle,
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
        let outcome = handle.wait();
        let cancelled = active.cancelled.load(Ordering::SeqCst);
        let (process_status, turn_status, error) = if cancelled {
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
                Some(non_terminal_exit_error(outcome.exit_code, &outcome.stderr)),
            )
        };

        let wm = WorktreeManager::new(&prepared.git_bin);
        let diff_result = wm.diff(&prepared.worktree_path, &prepared.base_ref);
        let (diff, diff_error) = match diff_result {
            Ok(diff) => (diff, None),
            Err(error) => (String::new(), Some(error.to_string())),
        };
        let task_status = terminal_task_status(process_status, diff_error.is_none());
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

fn non_terminal_exit_error(exit_code: Option<i32>, stderr: &str) -> String {
    const MAX_STDERR_CHARS: usize = 4_000;
    let stderr = stderr.trim();
    let detail: String = stderr.chars().take(MAX_STDERR_CHARS).collect();
    let truncated = stderr.chars().count() > MAX_STDERR_CHARS;
    let exit = exit_code
        .map(|code| format!(" with exit code {code}"))
        .unwrap_or_default();

    if detail.is_empty() {
        format!("Codex exited{exit} without a terminal turn event")
    } else {
        format!(
            "Codex exited{exit} before emitting a terminal turn event: {detail}{}",
            if truncated { "…" } else { "" }
        )
    }
}

fn terminal_task_status(process_status: &'static str, diff_succeeded: bool) -> &'static str {
    if process_status == "awaiting_review" && !diff_succeeded {
        "failed"
    } else {
        process_status
    }
}

fn emit_event(app: &AppHandle, event_name: &str, payload: TaskEventPayload) {
    let _ = app.emit(event_name, payload.clone());
    let _ = app.emit(TASK_EVENT_CHANNEL, payload);
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
    fn run_state_applies_updated_concurrency_without_cancelling_active_runs() {
        let state = RunState::new(2);
        state.reserve("one").unwrap();
        state.reserve("two").unwrap();
        state.set_max_concurrent(3);
        state.reserve("three").unwrap();
        state.set_max_concurrent(1);
        assert!(state.reserve("four").is_err());
        assert_eq!(state.active.lock().unwrap().len(), 3);
    }

    #[test]
    fn run_state_rejects_duplicate_task_reservation() {
        let state = RunState::new(2);
        state.reserve("same-task").unwrap();
        let error = state.reserve("same-task").unwrap_err();
        assert!(error.to_string().contains("already running"));
    }

    #[test]
    fn non_terminal_exit_reports_stderr() {
        let error = non_terminal_exit_error(
            Some(2),
            "error: unexpected argument '--full-auto' found\n\nUsage: codex exec",
        );
        assert!(error.contains("exit code 2"));
        assert!(error.contains("unexpected argument '--full-auto'"));
    }

    #[test]
    fn completed_turn_requires_a_review_diff() {
        assert_eq!(
            terminal_task_status("awaiting_review", true),
            "awaiting_review"
        );
        assert_eq!(terminal_task_status("awaiting_review", false), "failed");
        assert_eq!(terminal_task_status("cancelled", false), "cancelled");
    }

    #[test]
    fn follow_up_prompt_contains_structured_anchors_and_note() {
        let comments = vec![ReviewComment {
            id: "comment-1".to_string(),
            task_id: "task-1".to_string(),
            turn_id: "turn-1".to_string(),
            file_path: "src/main.rs".to_string(),
            line_number: Some(12),
            side: Some("new".to_string()),
            body: "Handle this error".to_string(),
            resolved: false,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        }];

        let prompt = build_follow_up_prompt(&comments, Some("Run the focused test")).unwrap();
        assert!(prompt.contains("\"filePath\": \"src/main.rs\""));
        assert!(prompt.contains("\"lineNumber\": 12"));
        assert!(prompt.contains("\"side\": \"new\""));
        assert!(prompt.contains("\"reviewerNote\": \"Run the focused test\""));
    }

    #[test]
    fn follow_up_requires_feedback() {
        let error = build_follow_up_prompt(&[], Some("   ")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("review comment or reviewer note")
        );
    }
}
