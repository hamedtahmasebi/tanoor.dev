//! Phase V task execution orchestration.
//!
//! This module owns the bridge between the database, git worktrees, and the
//! agent runner. The runner remains process/protocol focused; this module is
//! responsible for task state, turn rows, event forwarding, and cleanup.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::{
    commands::DbState,
    db,
    error::AppError,
    models::{ReviewComment, Task},
    runner::{AgentEventKind, EventStream, RunHandle, build_runner},
    worktree::WorktreeManager,
};

/// Global companion channel used by the frontend to keep non-selected tasks
/// synchronized. Task-scoped channels remain available to focused consumers.
pub const TASK_EVENT_CHANNEL: &str = "forge:task-event";

fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn load_settings(app: &AppHandle) -> Result<crate::models::AppSettings, AppError> {
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
    agent_id: String,
}

struct PreparedFollowUp {
    run: PreparedRun,
    thread_id: Option<String>,
    comment_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentSelection {
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

pub(crate) fn resolve_agent(
    settings: &crate::models::AppSettings,
    selection: &AgentSelection,
) -> Result<(String, String, String, Option<String>), AppError> {
    let agent_id = selection
        .agent_id
        .clone()
        .unwrap_or_else(|| settings.default_agent.clone());
    let (binary, default_model, default_effort) = match agent_id.as_str() {
        "codex" => (
            settings.codex_bin.clone(),
            settings.codex_model.clone(),
            Some(settings.codex_effort.clone()),
        ),
        "claude" => (
            settings.claude_bin.clone(),
            settings.claude_model.clone(),
            None,
        ),
        "opencode" => (
            settings.opencode_bin.clone(),
            settings.opencode_model.clone(),
            None,
        ),
        other => {
            return Err(AppError::InvalidOperation(format!(
                "Unsupported agent '{other}'"
            )));
        }
    };
    let model = selection.model.clone().unwrap_or(default_model);
    if model.trim().is_empty() {
        return Err(AppError::InvalidOperation(format!(
            "Model cannot be empty for agent '{agent_id}'"
        )));
    }
    let effort = selection.effort.clone().or(default_effort);
    Ok((agent_id, binary, model, effort))
}

fn merge_selection(
    override_selection: AgentSelection,
    group_selection: AgentSelection,
) -> AgentSelection {
    AgentSelection {
        agent_id: group_selection.agent_id.or(override_selection.agent_id),
        model: group_selection.model.or(override_selection.model),
        effort: group_selection.effort.or(override_selection.effort),
    }
}

fn load_follow_up_groups(
    app: &AppHandle,
    task_id: &str,
) -> Result<Vec<(AgentSelection, Vec<String>)>, AppError> {
    let db_state = app.state::<DbState>();
    let conn = db_state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    let comments = db::select_unresolved_review_comments(&conn, task_id)?;
    let mut groups = BTreeMap::<String, (AgentSelection, Vec<String>)>::new();
    for comment in comments {
        let selection = AgentSelection {
            agent_id: comment.assigned_agent_id.clone(),
            model: comment.assigned_model.clone(),
            effort: comment.assigned_effort.clone(),
        };
        let key = format!(
            "{}\0{}\0{}",
            selection.agent_id.as_deref().unwrap_or(""),
            selection.model.as_deref().unwrap_or(""),
            selection.effort.as_deref().unwrap_or("")
        );
        groups
            .entry(key)
            .or_insert_with(|| (selection, Vec::new()))
            .1
            .push(comment.id);
    }
    if groups.is_empty() {
        groups.insert(String::new(), (AgentSelection::default(), Vec::new()));
    }
    Ok(groups.into_values().collect())
}

/// Start an initial turn for a draft task and return its persisted running
/// representation. The actual stream consumer runs on a background thread.
pub fn start_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    selection: AgentSelection,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    let task_status = {
        let db_state = app.state::<DbState>();
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
        db::select_task(&conn, &task_id)?
            .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?
            .status
    };
    if task_status == "ready" {
        return start_ready_task(app, run_state, task_id, selection);
    }
    let selection = if selection.agent_id.is_none() && selection.model.is_none() {
        let db_state = app.state::<DbState>();
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        let task = db::select_task(&conn, &task_id)?
            .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
        AgentSelection {
            agent_id: task.agent_id,
            model: task.agent_model,
            effort: task.agent_effort,
        }
    } else {
        selection
    };
    let (agent_id, binary, model, effort) = resolve_agent(&settings, &selection)?;
    run_state.reserve(&task_id)?;

    let prepared = match prepare_run(app, &task_id, &settings.git_bin, &agent_id, &model, &effort) {
        Ok(prepared) => prepared,
        Err(error) => {
            run_state.release(&task_id);
            return Err(error);
        }
    };

    let runner = build_runner(&agent_id, binary, model, effort).map_err(|error| {
        run_state.release(&task_id);
        error
    })?;
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
pub fn submit_review(
    app: &AppHandle,
    task_id: String,
    reviewer_note: Option<String>,
    selection: AgentSelection,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    let (agent_id, _binary, model, effort) = resolve_agent(&settings, &selection)?;
    let db_state = app.state::<DbState>();
    let app_data_dir = app.path().app_data_dir().map_err(|error| {
        AppError::InvalidOperation(format!("Cannot resolve app data directory: {error}"))
    })?;
    let turn_id = Uuid::new_v4().to_string();
    let log_path = app_data_dir
        .join("logs")
        .join(&task_id)
        .join(format!("{turn_id}.jsonl"));
    let conn = db_state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
    let task = db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
    if !matches!(
        task.status.as_str(),
        "awaiting_review" | "changes_requested"
    ) {
        return Err(AppError::InvalidOperation(
            "Task is not open for review".into(),
        ));
    }
    let comments = db::select_unresolved_review_comments(&conn, &task_id)?;
    let prompt = build_follow_up_prompt(&comments, reviewer_note.as_deref())?;
    db::submit_review(
        &conn,
        &task_id,
        &turn_id,
        &prompt,
        &log_path.to_string_lossy(),
        &now(),
        Some(&agent_id),
        Some(&model),
        effort.as_deref(),
    )?;
    db::select_task(&conn, &task_id)?.ok_or_else(|| {
        AppError::NotFound(format!(
            "Task '{task_id}' disappeared after review submission"
        ))
    })
}

fn start_ready_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    selection: AgentSelection,
) -> Result<Task, AppError> {
    let groups = load_follow_up_groups(app, &task_id)?;
    let pending = {
        let state = app.state::<DbState>();
        let conn = state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
        db::select_pending_turn(&conn, &task_id)?.ok_or_else(|| {
            AppError::InvalidOperation("Task has no pending follow-up turn".into())
        })?
    };
    let first = groups.first().cloned().unwrap_or_default();
    let pending_selection = AgentSelection {
        agent_id: pending.agent_id.clone(),
        model: pending.agent_model.clone(),
        effort: pending.agent_effort.clone(),
    };
    // The selection chosen when review was submitted is the default for
    // unassigned feedback; a per-comment assignment always takes precedence.
    let first_selection = merge_selection(merge_selection(pending_selection, selection), first.0);
    let task = start_follow_up_group(
        app,
        run_state,
        task_id.clone(),
        None,
        first_selection,
        Some(first.1),
        Some(pending),
    )?;
    let remaining = groups.into_iter().skip(1).collect::<Vec<_>>();
    if !remaining.is_empty() {
        let worker_app = app.clone();
        let worker_state = run_state.clone();
        std::thread::spawn(move || {
            for (selection, comment_ids) in remaining {
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    let status =
                        worker_app
                            .try_state::<DbState>()
                            .and_then(|state| {
                                state.0.lock().ok().and_then(|conn| {
                                    db::select_task(&conn, &task_id).ok().flatten()
                                })
                            })
                            .map(|task| task.status);
                    match status.as_deref() {
                        Some("awaiting_review") | Some("changes_requested") => break,
                        Some("running") => continue,
                        _ => return,
                    }
                }
                if start_follow_up_group(
                    &worker_app,
                    &worker_state,
                    task_id.clone(),
                    None,
                    selection,
                    Some(comment_ids),
                    None,
                )
                .is_err()
                {
                    return;
                }
            }
        });
    }
    Ok(task)
}

fn start_follow_up_group(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    reviewer_note: Option<String>,
    selection: AgentSelection,
    comment_ids: Option<Vec<String>>,
    pending_turn: Option<crate::models::TaskTurn>,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    let (agent_id, binary, model, effort) = resolve_agent(&settings, &selection)?;
    run_state.reserve(&task_id)?;

    let prepared = match prepare_follow_up(
        app,
        &task_id,
        reviewer_note.as_deref(),
        &settings.git_bin,
        &agent_id,
        &model,
        &effort,
        comment_ids.as_deref(),
        pending_turn,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            run_state.release(&task_id);
            return Err(error);
        }
    };

    let runner = match build_runner(&agent_id, binary, model, effort) {
        Ok(runner) => runner,
        Err(error) => {
            mark_follow_up_start_failed(app, &prepared.run.task_id, &prepared.run.turn_id);
            run_state.release(&task_id);
            return Err(error);
        }
    };
    let handoff_prompt = if prepared.thread_id.is_none() {
        build_handoff_prompt(&prepared.run.prompt, &prepared.run.base_ref)
    } else {
        prepared.run.prompt.clone()
    };
    let process = if let Some(thread_id) = prepared.thread_id.as_deref() {
        runner.resume_turn(
            &prepared.run.worktree_path,
            thread_id,
            &handoff_prompt,
            &prepared.run.log_path,
        )
    } else {
        runner.start_turn(
            &prepared.run.worktree_path,
            &handoff_prompt,
            &prepared.run.log_path,
        )
    };
    let (handle, stream) = match process {
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

/// Reset a failed or cancelled task back to `draft` (cleaning up any leftover
/// worktree) and start it again. Used by the "Retry" action for tasks that were
/// left dangling by a crash or that failed to launch.
pub fn retry_task(
    app: &AppHandle,
    run_state: &RunState,
    task_id: String,
    selection: AgentSelection,
) -> Result<Task, AppError> {
    let settings = load_settings(app)?;
    let db_state = app.state::<DbState>();
    let (task, project_root) = {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
        let task = db::select_task(&conn, &task_id)?
            .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
        let project = db::select_project(&conn, &task.project_id)?.ok_or_else(|| {
            AppError::NotFound(format!("Project '{}' not found", task.project_id))
        })?;
        (task, project.root_path)
    };
    if !matches!(task.status.as_str(), "failed" | "cancelled") {
        return Err(AppError::InvalidOperation(
            "Only failed or cancelled tasks can be retried".into(),
        ));
    }

    // Best-effort cleanup of any worktree left behind by the failed run so the
    // fresh run starts from a clean state.
    if let (Some(worktree_path), Some(branch_name)) =
        (task.worktree_path.as_deref(), task.branch_name.as_deref())
    {
        let wm = WorktreeManager::new(&settings.git_bin);
        let _ = wm.remove_worktree(
            std::path::Path::new(&project_root),
            std::path::Path::new(worktree_path),
            branch_name,
        );
    }

    {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
        db::reset_task_run(&conn, &task_id, &now())?;
    }

    start_task(app, run_state, task_id, selection)
}

fn prepare_run(
    app: &AppHandle,
    task_id: &str,
    git_bin: &str,
    agent_id: &str,
    agent_model: &str,
    agent_effort: &Option<String>,
) -> Result<PreparedRun, AppError> {
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

    let branch_name = task
        .branch_name
        .unwrap_or_else(|| crate::naming::fallback_branch_name(&task.prompt, task_id));
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
        db::set_task_agent(
            &conn,
            task_id,
            agent_id,
            agent_model,
            agent_effort.as_deref(),
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
            Some(agent_id),
            Some(agent_model),
            agent_effort.as_deref(),
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
        agent_id: agent_id.to_string(),
    })
}

fn prepare_follow_up(
    app: &AppHandle,
    task_id: &str,
    reviewer_note: Option<&str>,
    git_bin: &str,
    agent_id: &str,
    agent_model: &str,
    agent_effort: &Option<String>,
    selected_comment_ids: Option<&[String]>,
    pending_turn: Option<crate::models::TaskTurn>,
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
            "awaiting_review" | "changes_requested" | "ready"
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
        let comments = if let Some(ids) = selected_comment_ids {
            comments
                .into_iter()
                .filter(|comment| ids.iter().any(|id| id == &comment.id))
                .collect()
        } else {
            comments
        };
        (task, project, comments)
    };

    let prompt = pending_turn
        .as_ref()
        .map(|turn| turn.prompt.clone())
        .unwrap_or(build_follow_up_prompt(&comments, reviewer_note)?);
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
    let thread_id = {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::select_latest_agent_turn(&conn, task_id)?
            .and_then(|(latest_agent, _, thread_id)| {
                (latest_agent == agent_id).then_some(thread_id).flatten()
            })
            .or_else(|| {
                (agent_id == "codex")
                    .then_some(task.agent_thread_id.clone())
                    .flatten()
            })
    };
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
    let turn_id = pending_turn
        .as_ref()
        .map(|turn| turn.id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let log_path = pending_turn
        .as_ref()
        .map(|turn| PathBuf::from(&turn.log_path))
        .unwrap_or_else(|| {
            app_data_dir
                .join("logs")
                .join(task_id)
                .join(format!("{turn_id}.jsonl"))
        });
    let now_str = now();
    {
        let conn = db_state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        if pending_turn.is_some() {
            db::activate_pending_turn(&conn, task_id, &turn_id, &now_str)?;
        } else {
            db::begin_follow_up(
                &conn,
                task_id,
                &turn_id,
                &prompt,
                &log_path.to_string_lossy(),
                &now_str,
                Some(agent_id),
                Some(agent_model),
                agent_effort.as_deref(),
            )?;
        }
        db::set_task_agent(
            &conn,
            task_id,
            agent_id,
            agent_model,
            agent_effort.as_deref(),
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
            agent_id: agent_id.to_string(),
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
                "lineEndNumber": comment.line_end_number,
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

fn build_handoff_prompt(follow_up_prompt: &str, base_ref: &str) -> String {
    format!(
        "You are taking over an existing coding task from another agent. Work in the current worktree and preserve correct existing changes. The original base revision was {base_ref}. Inspect the current files and git diff before editing.\n\n{follow_up_prompt}\n\nThis is a cross-agent handoff: independently verify the implementation and explain any structural concerns before making changes."
    )
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
    let mut terminal_kind: Option<AgentEventKind> = None;
    let mut terminal_error: Option<String> = None;

    for event in stream {
        if let Some(thread_id) = event.thread_id() {
            if let Some(db_state) = app.try_state::<DbState>() {
                if let Ok(conn) = db_state.0.lock() {
                    let _ = db::set_turn_thread_id(
                        &conn,
                        &prepared.task_id,
                        &prepared.turn_id,
                        thread_id,
                        &now(),
                    );
                }
            }
        }
        let is_terminal = event.is_terminal();
        if is_terminal {
            terminal_kind = Some(event.kind);
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
        // stdout reached EOF; a healthy agent exits right after. Bound the wait
        // so a process that closed its output but got stuck can't hang the turn.
        let outcome = handle.wait_bounded(Duration::from_secs(30));
        let cancelled = active.cancelled.load(Ordering::SeqCst);
        let (process_status, turn_status, error) = if cancelled {
            ("cancelled", "cancelled", None)
        } else if terminal_kind == Some(AgentEventKind::TurnCompleted) {
            ("awaiting_review", "completed", None)
        } else if terminal_kind == Some(AgentEventKind::TurnFailed) {
            (
                "failed",
                "failed",
                terminal_error
                    .or_else(|| Some(format!("{} reported a failed turn", prepared.agent_id))),
            )
        } else {
            (
                "failed",
                "failed",
                Some(non_terminal_exit_error(
                    &prepared.agent_id,
                    outcome.exit_code,
                    &outcome.stderr,
                )),
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

fn non_terminal_exit_error(agent_id: &str, exit_code: Option<i32>, stderr: &str) -> String {
    const MAX_STDERR_CHARS: usize = 4_000;
    let stderr = stderr.trim();
    let detail: String = stderr.chars().take(MAX_STDERR_CHARS).collect();
    let truncated = stderr.chars().count() > MAX_STDERR_CHARS;
    let exit = exit_code
        .map(|code| format!(" with exit code {code}"))
        .unwrap_or_default();

    if detail.is_empty() {
        format!("{agent_id} exited{exit} without a terminal turn event")
    } else {
        format!(
            "{agent_id} exited{exit} before emitting a terminal turn event: {detail}{}",
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
            "codex",
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
            line_end_number: None,
            side: Some("new".to_string()),
            body: "Handle this error".to_string(),
            resolved: false,
            assigned_agent_id: None,
            assigned_model: None,
            assigned_effort: None,
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

    #[test]
    fn assigned_group_selection_wins_over_default_selection() {
        let merged = merge_selection(
            AgentSelection {
                agent_id: Some("codex".to_string()),
                model: Some("o4-mini".to_string()),
                effort: None,
            },
            AgentSelection {
                agent_id: Some("claude".to_string()),
                model: Some("opus".to_string()),
                effort: None,
            },
        );
        assert_eq!(merged.agent_id.as_deref(), Some("claude"));
        assert_eq!(merged.model.as_deref(), Some("opus"));
    }

    #[test]
    fn handoff_prompt_requires_fresh_agent_context() {
        let prompt = build_handoff_prompt("Fix the review comments", "abc123");
        assert!(prompt.contains("cross-agent handoff"));
        assert!(prompt.contains("abc123"));
        assert!(prompt.contains("Fix the review comments"));
    }
}
