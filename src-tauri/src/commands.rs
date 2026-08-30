use std::sync::Mutex;

use chrono::Utc;
use tauri::State;
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    execution::{self, RunState},
    models::{Project, Task},
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
pub fn create_project(
    state: State<'_, DbState>,
    root_path: String,
) -> Result<Project, AppError> {
    validate_project_root(&root_path)?;

    // Enforce the git-repository requirement (Phase III).
    // WorktreeManager::is_git_repo shells out to `git rev-parse --git-dir`.
    let wm = WorktreeManager::default();
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
        return Err(AppError::NotFound(format!("Project '{project_id}' not found")));
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
pub fn list_tasks(
    state: State<'_, DbState>,
    project_id: String,
) -> Result<Vec<Task>, AppError> {
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
/// `codex_bin` overrides the binary name/path; when `null` the default
/// `"codex"` is used.  Phase VIII settings will populate this field from
/// the user's stored preference.
#[tauri::command]
pub fn check_codex_health(codex_bin: Option<String>) -> runner::HealthStatus {
    let bin = codex_bin.as_deref().unwrap_or("codex");
    runner::health_check(bin)
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
    codex_bin: Option<String>,
) -> Result<Task, AppError> {
    execution::start_task(&app, &runs, task_id, codex_bin)
}

/// Cancel the active Codex process tree. The background consumer computes and
/// persists the partial diff before publishing the cancelled state.
#[tauri::command]
pub fn cancel_task(
    runs: tauri::State<'_, RunState>,
    task_id: String,
) -> Result<(), AppError> {
    execution::cancel_task(&runs, &task_id)
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
}
