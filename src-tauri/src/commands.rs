use std::{process::Command, sync::Mutex};

use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    execution::{self, RunState},
    models::{AppSettings, Project, ReviewComment, Task, TaskTurn},
    runner,
    worktree::WorktreeManager,
};

// ---------------------------------------------------------------------------
// Managed state
// ---------------------------------------------------------------------------

/// Wraps the SQLite connection in a Mutex so it can be shared across Tauri's
/// async command executor threads.  `rusqlite::Connection` is `Send` but not
/// `Sync`; the Mutex provides the required `Sync`.
pub struct DbState(pub Mutex<rusqlite::Connection>);

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryHealthStatus {
    binary_found: bool,
    version: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemHealthStatus {
    codex: runner::HealthStatus,
    git: BinaryHealthStatus,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// Reject file-ref paths that are absolute or contain `..` components.
/// The paths are stored as strings relative to the project root; deeper
/// filesystem validation (e.g. that the file actually exists) is deferred to
/// Phase III when worktrees are created.
fn validate_file_ref(rel_path: &str) -> Result<(), AppError> {
    use std::path::{Component, Path};
    let path = Path::new(rel_path);
    if path.is_absolute() {
        return Err(AppError::InvalidOperation(format!(
            "File reference must be relative, got absolute path: '{rel_path}'"
        )));
    }
    for component in path.components() {
        if component == Component::ParentDir {
            return Err(AppError::InvalidOperation(format!(
                "File reference may not contain '..': '{rel_path}'"
            )));
        }
    }
    Ok(())
}

/// Return an `InvalidOperation` error if the given path is not an existing
/// directory on the host filesystem.
fn validate_project_root(root_path: &str) -> Result<(), AppError> {
    let path = std::path::Path::new(root_path);
    if !path.exists() {
        return Err(AppError::InvalidOperation(format!(
            "Path does not exist: '{root_path}'"
        )));
    }
    if !path.is_dir() {
        return Err(AppError::InvalidOperation(format!(
            "Path is not a directory: '{root_path}'"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Project commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn create_project(state: State<'_, DbState>, root_path: String) -> Result<Project, AppError> {
    validate_project_root(&root_path)?;

    // Enforce the git-repository requirement (Phase III).
    // WorktreeManager::is_git_repo shells out to `git rev-parse --git-dir`.
    let git_bin = {
        let conn = state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::select_settings(&conn)?.git_bin
    };
    let wm = WorktreeManager::new(git_bin);
    if !wm.is_git_repo(std::path::Path::new(&root_path)) {
        return Err(AppError::InvalidOperation(format!(
            "'{root_path}' is not a git repository. \
             Run 'git init' inside the folder first, or choose a different folder."
        )));
    }

    let project = Project {
        id: Uuid::new_v4().to_string(),
        root_path,
        created_at: now(),
    };

    let conn = state.0.lock().unwrap();
    db::insert_project(&conn, &project).map_err(|e| {
        // Surface a friendlier message for the UNIQUE constraint violation.
        if let AppError::Database(rusqlite::Error::SqliteFailure(ref fe, _)) = e {
            if fe.code == rusqlite::ErrorCode::ConstraintViolation {
                return AppError::InvalidOperation(format!(
                    "A project at '{}' is already tracked",
                    project.root_path
                ));
            }
        }
        e
    })?;

    Ok(project)
}

#[tauri::command]
pub fn list_projects(state: State<'_, DbState>) -> Result<Vec<Project>, AppError> {
    let conn = state.0.lock().unwrap();
    db::select_all_projects(&conn)
}

// ---------------------------------------------------------------------------
// Task commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn create_task(
    state: State<'_, DbState>,
    project_id: String,
    title: String,
    prompt: String,
    file_refs: Vec<String>,
) -> Result<Task, AppError> {
    // Validate every ref before touching the DB.
    for rel in &file_refs {
        validate_file_ref(rel)?;
    }

    let conn = state.0.lock().unwrap();

    if db::select_project(&conn, &project_id)?.is_none() {
        return Err(AppError::NotFound(format!(
            "Project '{project_id}' not found"
        )));
    }

    let now_str = now();
    let task = Task {
        id: Uuid::new_v4().to_string(),
        project_id,
        title,
        prompt,
        file_refs,
        status: "draft".to_string(),
        base_ref: None,
        worktree_path: None,
        branch_name: None,
        agent_thread_id: None,
        diff: None,
        created_at: now_str.clone(),
        updated_at: now_str,
    };

    db::insert_task(&conn, &task)?;
    Ok(task)
}

#[tauri::command]
pub fn list_tasks(state: State<'_, DbState>, project_id: String) -> Result<Vec<Task>, AppError> {
    let conn = state.0.lock().unwrap();
    db::select_tasks_by_project(&conn, &project_id)
}

#[tauri::command]
pub fn get_task(state: State<'_, DbState>, task_id: String) -> Result<Task, AppError> {
    let conn = state.0.lock().unwrap();
    db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))
}

#[tauri::command]
pub fn delete_task(state: State<'_, DbState>, task_id: String) -> Result<(), AppError> {
    let conn = state.0.lock().unwrap();
    if let Some(task) = db::select_task(&conn, &task_id)? {
        if task.status == "running" {
            return Err(AppError::InvalidOperation(
                "Cancel the active run before deleting this task".to_string(),
            ));
        }
    }
    if !db::delete_task_by_id(&conn, &task_id)? {
        return Err(AppError::NotFound(format!("Task '{task_id}' not found")));
    }
    Ok(())
}

/// Update a task's prompt, title, and/or file refs. **Only allowed while the
/// task is in `draft` status** — the DB layer enforces this constraint.
#[tauri::command]
pub fn update_task_prompt(
    state: State<'_, DbState>,
    task_id: String,
    title: Option<String>,
    prompt: Option<String>,
    file_refs: Option<Vec<String>>,
) -> Result<Task, AppError> {
    if let Some(refs) = &file_refs {
        for rel in refs {
            validate_file_ref(rel)?;
        }
    }

    let now_str = now();
    let conn = state.0.lock().unwrap();

    db::update_draft_task(
        &conn,
        &task_id,
        title.as_deref(),
        prompt.as_deref(),
        file_refs.as_deref(),
        &now_str,
    )?;

    db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))
}

// ---------------------------------------------------------------------------
// Agent runner commands
// ---------------------------------------------------------------------------

/// Check whether the `codex` binary is present on PATH and detect API-key
/// environment variables.  Safe to call at any time — it does not make
/// network requests.
///
#[tauri::command]
pub fn check_codex_health(state: State<'_, DbState>) -> Result<runner::HealthStatus, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    let settings = db::select_settings(&conn)?;
    Ok(runner::health_check(&settings.codex_bin))
}

#[tauri::command]
pub fn get_settings(state: State<'_, DbState>) -> Result<AppSettings, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::select_settings(&conn)
}

// ---------------------------------------------------------------------------
// Agent model catalog
// ---------------------------------------------------------------------------

/// One reasoning-effort level offered by a model.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffortLevel {
    pub id: String,
    pub description: String,
}

/// One model entry returned to the frontend.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    /// The slug that is passed to `codex exec --model`.
    pub id: String,
    /// Human-readable name shown in the UI.
    pub label: String,
    pub description: String,
    /// Ordered list of effort levels supported by this model.
    /// Empty → model does not support effort control.
    pub effort_levels: Vec<EffortLevel>,
    /// The effort level that should be pre-selected when the user first picks
    /// this model (matches one of `effort_levels[].id`, or empty string).
    pub default_effort: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelCatalog {
    pub agent_id: String,
    pub agent_label: String,
    pub models: Vec<ModelOption>,
    pub selected_model: String,
    pub selected_effort: String,
}

// ---------------------------------------------------------------------------
// Raw types for parsing `codex debug models` JSON output
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct RawModelsResponse {
    models: Vec<RawModel>,
}

#[derive(Debug, serde::Deserialize)]
struct RawModel {
    slug: String,
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    default_reasoning_level: String,
    #[serde(default)]
    supported_reasoning_levels: Vec<RawReasoningLevel>,
    #[serde(default)]
    visibility: String,
}

#[derive(Debug, serde::Deserialize)]
struct RawReasoningLevel {
    effort: String,
    #[serde(default)]
    description: String,
}

/// Run `<codex_bin> debug models` and parse the JSON catalog.
/// Returns an error if the process fails or the output is not valid JSON.
fn fetch_codex_models(codex_bin: &str) -> Result<Vec<ModelOption>, AppError> {
    let output = Command::new(codex_bin)
        .args(["debug", "models"])
        .output()
        .map_err(|e| {
            AppError::InvalidOperation(format!("Failed to run '{codex_bin} debug models': {e}"))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::InvalidOperation(format!(
            "'{codex_bin} debug models' exited with status {}. stderr: {stderr}",
            output.status
        )));
    }

    let raw: RawModelsResponse = serde_json::from_slice(&output.stdout).map_err(|e| {
        AppError::InvalidOperation(format!(
            "Failed to parse '{codex_bin} debug models' output: {e}"
        ))
    })?;

    let models = raw
        .models
        .into_iter()
        // Only show models the CLI marks as visible in its own picker
        .filter(|m| m.visibility != "hide")
        .map(|m| {
            let effort_levels = m
                .supported_reasoning_levels
                .into_iter()
                .map(|r| EffortLevel {
                    id: r.effort,
                    description: r.description,
                })
                .collect::<Vec<_>>();
            ModelOption {
                id: m.slug,
                label: m.display_name,
                description: m.description,
                default_effort: m.default_reasoning_level,
                effort_levels,
            }
        })
        .collect();

    Ok(models)
}

#[tauri::command]
pub fn get_agent_models(state: State<'_, DbState>) -> Result<AgentModelCatalog, AppError> {
    let settings = {
        let conn = state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::select_settings(&conn)?
    };

    let models = fetch_codex_models(&settings.codex_bin)?;

    Ok(AgentModelCatalog {
        agent_id: "codex".to_string(),
        agent_label: "Codex".to_string(),
        models,
        selected_model: settings.codex_model,
        selected_effort: settings.codex_effort,
    })
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, DbState>,
    runs: State<'_, RunState>,
    codex_bin: String,
    git_bin: String,
    max_concurrent_tasks: usize,
    merge_on_confirm: bool,
    codex_model: String,
    codex_effort: String,
) -> Result<AppSettings, AppError> {
    let codex_bin = codex_bin.trim().to_string();
    let git_bin = git_bin.trim().to_string();
    let codex_model = codex_model.trim().to_string();
    let codex_effort = codex_effort.trim().to_string();
    if codex_bin.is_empty() || git_bin.is_empty() {
        return Err(AppError::InvalidOperation(
            "Codex and git binary paths cannot be empty".to_string(),
        ));
    }
    if !(1..=16).contains(&max_concurrent_tasks) {
        return Err(AppError::InvalidOperation(
            "Maximum concurrent tasks must be between 1 and 16".to_string(),
        ));
    }
    if codex_model.is_empty() {
        return Err(AppError::InvalidOperation(
            "Codex model cannot be empty".to_string(),
        ));
    }
    if codex_effort.is_empty() {
        return Err(AppError::InvalidOperation(
            "Codex effort cannot be empty".to_string(),
        ));
    }
    let settings = AppSettings {
        codex_bin,
        git_bin,
        max_concurrent_tasks,
        merge_on_confirm,
        codex_model,
        codex_effort,
        ..AppSettings::default()
    };
    let mut conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::replace_settings(&mut conn, &settings)?;
    runs.set_max_concurrent(max_concurrent_tasks);
    Ok(settings)
}

#[tauri::command]
pub fn check_system_health(state: State<'_, DbState>) -> Result<SystemHealthStatus, AppError> {
    let settings = {
        let conn = state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::select_settings(&conn)?
    };
    let git = match Command::new(&settings.git_bin).arg("--version").output() {
        Ok(output) if output.status.success() => BinaryHealthStatus {
            binary_found: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            detail: None,
        },
        Ok(output) => BinaryHealthStatus {
            binary_found: true,
            version: None,
            detail: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        },
        Err(error) => BinaryHealthStatus {
            binary_found: false,
            version: None,
            detail: Some(error.to_string()),
        },
    };
    Ok(SystemHealthStatus {
        codex: runner::health_check(&settings.codex_bin),
        git,
    })
}

// ---------------------------------------------------------------------------
// Execution commands
// ---------------------------------------------------------------------------

/// Create the task worktree, start its initial Codex turn, and return the
/// running task. Progress is delivered asynchronously on
/// task:{task_id}:event.
#[tauri::command]
pub fn run_task(
    app: tauri::AppHandle,
    runs: tauri::State<'_, RunState>,
    task_id: String,
) -> Result<Task, AppError> {
    execution::start_task(&app, &runs, task_id)
}

/// Cancel the active Codex process tree. The background consumer computes and
/// persists the partial diff before publishing the cancelled state.
///
/// If there is no live run for the task but it is still marked `running` in
/// the database (dangling after a crash), the task is transitioned to `failed`
/// so the UI can recover gracefully.
#[tauri::command]
pub fn cancel_task(
    state: State<'_, DbState>,
    runs: tauri::State<'_, RunState>,
    task_id: String,
) -> Result<Task, AppError> {
    match execution::cancel_task(&runs, &task_id) {
        Ok(()) => {
            // Live process signalled — return the current task row so the
            // frontend can update its status immediately (the background thread
            // will emit a final status event shortly after).
            let conn = state
                .0
                .lock()
                .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
            db::select_task(&conn, &task_id)?
                .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))
        }
        Err(AppError::NotFound(_)) => {
            // No live run — check if the task is stuck in `running` in the DB
            // (dangling after a crash or hard-kill) and recover it.
            let conn = state
                .0
                .lock()
                .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
            let task = db::select_task(&conn, &task_id)?
                .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
            if task.status == "running" {
                let now_str = now();
                // Close any open turn rows for this task.
                conn.execute(
                    "UPDATE turn SET status = 'failed', ended_at = ?1
                     WHERE task_id = ?2 AND status = 'running'",
                    rusqlite::params![&now_str, &task_id],
                )?;
                db::finish_task(&conn, &task_id, "failed", "", &now_str)?;
                db::select_task(&conn, &task_id)?
                    .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))
            } else {
                // Task is not running — nothing to cancel, return it as-is.
                Ok(task)
            }
        }
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------
// Review commands
// ---------------------------------------------------------------------------

fn validate_comment_anchor(
    file_path: &str,
    line_number: Option<i64>,
    side: Option<&str>,
    body: &str,
) -> Result<(), AppError> {
    validate_file_ref(file_path)?;
    if file_path.trim().is_empty() {
        return Err(AppError::InvalidOperation(
            "Review comment file path cannot be empty".to_string(),
        ));
    }
    if body.trim().is_empty() {
        return Err(AppError::InvalidOperation(
            "Review comment body cannot be empty".to_string(),
        ));
    }
    match (line_number, side) {
        (None, None) => Ok(()),
        (Some(line), Some("old" | "new")) if line > 0 => Ok(()),
        (Some(line), _) if line <= 0 => Err(AppError::InvalidOperation(
            "Review comment line number must be positive".to_string(),
        )),
        (Some(_), _) => Err(AppError::InvalidOperation(
            "Line comments must specify side 'old' or 'new'".to_string(),
        )),
        (None, Some(_)) => Err(AppError::InvalidOperation(
            "File-level comments cannot specify a diff side".to_string(),
        )),
    }
}

#[tauri::command]
pub fn add_review_comment(
    state: State<'_, DbState>,
    task_id: String,
    file_path: String,
    line_number: Option<i64>,
    side: Option<String>,
    body: String,
) -> Result<ReviewComment, AppError> {
    validate_comment_anchor(&file_path, line_number, side.as_deref(), &body)?;
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    let task = db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
    if !matches!(
        task.status.as_str(),
        "awaiting_review" | "changes_requested"
    ) {
        return Err(AppError::InvalidOperation(format!(
            "Task '{task_id}' is not available for review"
        )));
    }
    let turn_id = db::select_latest_turn_id(&conn, &task_id)?.ok_or_else(|| {
        AppError::InvalidOperation(format!("Task '{task_id}' has no turn to review"))
    })?;
    let comment = ReviewComment {
        id: Uuid::new_v4().to_string(),
        task_id,
        turn_id,
        file_path,
        line_number,
        side,
        body: body.trim().to_string(),
        resolved: false,
        created_at: now(),
    };
    db::insert_review_comment(&conn, &comment)?;
    Ok(comment)
}

#[tauri::command]
pub fn resolve_review_comment(
    state: State<'_, DbState>,
    comment_id: String,
) -> Result<ReviewComment, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::resolve_review_comment(&conn, &comment_id)?
        .ok_or_else(|| AppError::NotFound(format!("Review comment '{comment_id}' not found")))
}

#[tauri::command]
pub fn list_review_comments(
    state: State<'_, DbState>,
    task_id: String,
) -> Result<Vec<ReviewComment>, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    if db::select_task(&conn, &task_id)?.is_none() {
        return Err(AppError::NotFound(format!("Task '{task_id}' not found")));
    }
    db::select_review_comments(&conn, &task_id)
}

/// Package unresolved comments and an optional note into one structured
/// follow-up prompt, then resume the task's existing Codex thread.
#[tauri::command]
pub fn request_changes(
    app: tauri::AppHandle,
    runs: tauri::State<'_, RunState>,
    task_id: String,
    reviewer_note: Option<String>,
) -> Result<Task, AppError> {
    execution::start_follow_up(&app, &runs, task_id, reviewer_note)
}

/// Commit a reviewed task, optionally merge its branch into the current main
/// checkout, remove its worktree, and persist the approved state.
#[tauri::command]
pub fn confirm_task(
    app: tauri::AppHandle,
    runs: tauri::State<'_, RunState>,
    task_id: String,
    merge: bool,
) -> Result<Task, AppError> {
    execution::confirm_task(&app, &runs, &task_id, merge)
}

/// Return all turns for a task in chronological order.
#[tauri::command]
pub fn list_task_turns(
    state: State<'_, DbState>,
    task_id: String,
) -> Result<Vec<TaskTurn>, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::select_turns_for_task(&conn, &task_id)
}

/// Read the JSONL event log for a specific turn and return raw lines.
/// Returns an empty list if the log file does not yet exist.
#[tauri::command]
pub fn get_turn_output(turn_log_path: String) -> Result<Vec<String>, AppError> {
    let path = std::path::Path::new(&turn_log_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::InvalidOperation(format!("Cannot read log: {e}")))?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.to_string())
        .collect();
    Ok(content)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_file_refs_accepted() {
        validate_file_ref("src/main.rs").expect("simple relative path");
        validate_file_ref("docs/README.md").expect("nested relative path");
        validate_file_ref("Cargo.toml").expect("file in project root");
    }

    #[test]
    fn parent_traversal_rejected() {
        let err = validate_file_ref("../outside/secret.rs");
        assert!(err.is_err());
        assert!(matches!(err.unwrap_err(), AppError::InvalidOperation(_)));
    }

    #[test]
    fn absolute_path_rejected() {
        let absolute = std::env::current_dir()
            .unwrap()
            .join("outside")
            .join("secret.rs");
        let err = validate_file_ref(absolute.to_str().unwrap());
        assert!(err.is_err());
        assert!(matches!(err.unwrap_err(), AppError::InvalidOperation(_)));
    }

    #[test]
    fn project_root_must_exist() {
        let err = validate_project_root("/this/path/does/not/exist/12345");
        assert!(err.is_err());
        assert!(matches!(err.unwrap_err(), AppError::InvalidOperation(_)));
    }

    #[test]
    fn review_comment_anchor_validation() {
        validate_comment_anchor("src/main.rs", Some(4), Some("new"), "Please adjust").unwrap();
        validate_comment_anchor("src/main.rs", None, None, "File-level note").unwrap();
        assert!(validate_comment_anchor("src/main.rs", Some(0), Some("new"), "body").is_err());
        assert!(validate_comment_anchor("src/main.rs", Some(1), Some("right"), "body").is_err());
        assert!(validate_comment_anchor("src/main.rs", None, Some("old"), "body").is_err());
        assert!(validate_comment_anchor("src/main.rs", Some(1), Some("old"), "  ").is_err());
    }
}
