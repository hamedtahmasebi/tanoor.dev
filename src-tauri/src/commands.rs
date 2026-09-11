use std::{
    collections::HashMap,
    process::Command,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

use crate::{
    db,
    editors::{self, EditorInfo},
    error::AppError,
    execution::{self, AgentSelection, RunState},
    models::{AppSettings, Project, ReviewComment, Task, TaskTurn},
    naming, runner,
    worktree::WorktreeManager,
};

// ---------------------------------------------------------------------------
// Managed state
// ---------------------------------------------------------------------------

/// Wraps the SQLite connection in a Mutex so it can be shared across Tauri's
/// async command executor threads.  `rusqlite::Connection` is `Send` but not
/// `Sync`; the Mutex provides the required `Sync`.
pub struct DbState(pub Mutex<rusqlite::Connection>);

#[derive(Debug, Clone, serde::Serialize)]
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
    agents: Vec<runner::HealthStatus>,
}

/// In-memory catalog cache. A refresh request replaces successful entries and
/// keeps the previous models available if an agent is temporarily unavailable.
#[derive(Clone, Default)]
pub struct AgentCatalogState(pub Arc<Mutex<HashMap<String, AgentModelCatalog>>>);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// A single entry returned by the context-file picker. Directories are
/// expanded lazily so large repositories never read their entire tree.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    pub name: String,
    /// Path relative to the project root, using `/` separators.
    pub path: String,
    pub is_dir: bool,
}

/// Directory names excluded from the context picker at any depth, both for
/// performance and to keep dependency/build noise out of the results.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    "target",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".cache",
    "coverage",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".gradle",
    ".dart_tool",
    ".idea",
    ".vscode",
    "vendor",
    ".terraform",
];

const SEARCH_RESULT_LIMIT: usize = 50;

fn is_ignored_dir(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

/// Resolve a project-relative subpath, rejecting absolute paths and `..`
/// traversal so the picker can never escape the project root.
fn resolve_subpath(root: &std::path::Path, rel: &str) -> Result<std::path::PathBuf, AppError> {
    use std::path::Component;
    let rel_path = std::path::Path::new(rel);
    if rel_path.is_absolute() {
        return Err(AppError::InvalidOperation(
            "Directory path must be relative to the project".into(),
        ));
    }
    for component in rel_path.components() {
        if component == Component::ParentDir {
            return Err(AppError::InvalidOperation(
                "Directory path may not contain '..'".into(),
            ));
        }
    }
    Ok(root.join(rel_path))
}

fn search_project_dir(
    dir: &std::path::Path,
    root: &std::path::Path,
    query: &str,
    results: &mut Vec<String>,
) -> std::io::Result<()> {
    if results.len() >= SEARCH_RESULT_LIMIT {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if results.len() >= SEARCH_RESULT_LIMIT {
            break;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if is_ignored_dir(&name) {
                continue;
            }
            search_project_dir(&entry.path(), root, query, results)?;
        } else if file_type.is_file() {
            if let Ok(relative) = entry.path().strip_prefix(root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if relative.to_lowercase().contains(query) {
                    results.push(relative);
                }
            }
        }
    }
    Ok(())
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

/// List the immediate children of a project directory (root when `path` is
/// `None`), skipping ignored directories. The frontend expands folders lazily.
#[tauri::command]
pub fn list_project_dir(
    state: State<'_, DbState>,
    project_id: String,
    path: Option<String>,
) -> Result<Vec<ProjectEntry>, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
    let project = db::select_project(&conn, &project_id)?
        .ok_or_else(|| AppError::NotFound(format!("Project '{project_id}' not found")))?;
    let root = std::path::Path::new(&project.root_path);
    let rel = path.unwrap_or_default().replace('\\', "/");
    let rel = rel.trim_matches('/').to_string();
    let dir = resolve_subpath(root, &rel)?;
    let read = std::fs::read_dir(&dir)
        .map_err(|error| AppError::InvalidOperation(format!("Cannot read directory: {error}")))?;
    let mut entries = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let is_dir = file_type.is_dir();
        if !is_dir && !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_dir && is_ignored_dir(&name) {
            continue;
        }
        let entry_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        entries.push(ProjectEntry {
            name,
            path: entry_path,
            is_dir,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Recursively search project files whose relative path contains `query`
/// (case-insensitive), skipping ignored directories and capping results.
#[tauri::command]
pub fn search_project_files(
    state: State<'_, DbState>,
    project_id: String,
    query: String,
) -> Result<Vec<String>, AppError> {
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
    let project = db::select_project(&conn, &project_id)?
        .ok_or_else(|| AppError::NotFound(format!("Project '{project_id}' not found")))?;
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let root = std::path::Path::new(&project.root_path);
    let mut results = Vec::new();
    let _ = search_project_dir(root, root, &query, &mut results);
    Ok(results)
}

#[tauri::command]
pub fn create_task(
    state: State<'_, DbState>,
    project_id: String,
    title: Option<String>,
    prompt: String,
    file_refs: Vec<String>,
    agent_id: Option<String>,
    agent_model: Option<String>,
    agent_effort: Option<String>,
    app: tauri::AppHandle,
) -> Result<Task, AppError> {
    // Validate every ref before touching the DB.
    for rel in &file_refs {
        validate_file_ref(rel)?;
    }

    let fallback_title = naming::fallback_title(&prompt);
    let title = title.unwrap_or_default().trim().to_string();
    let title = if title.is_empty() {
        fallback_title
    } else {
        title
    };
    let task_id = Uuid::new_v4().to_string();
    let branch_name = naming::fallback_branch_name(&prompt, &task_id);
    let (task, project_root) = {
        let conn = state.0.lock().unwrap();
        let project = db::select_project(&conn, &project_id)?
            .ok_or_else(|| AppError::NotFound(format!("Project '{project_id}' not found")))?;
        let now_str = now();
        let task = Task {
            id: task_id,
            project_id,
            title,
            prompt,
            file_refs,
            status: "draft".to_string(),
            base_ref: None,
            worktree_path: None,
            branch_name: Some(branch_name),
            agent_thread_id: None,
            agent_id,
            agent_model,
            agent_effort,
            diff: None,
            created_at: now_str.clone(),
            updated_at: now_str,
        };
        db::insert_task(&conn, &task)?;
        (task, project.root_path)
    };
    let naming_app = app.clone();
    let naming_task = task.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), AppError> {
            let settings = execution::load_settings(&naming_app)?;
            let selection = AgentSelection::default();
            let (agent_id, binary, model, effort) =
                execution::resolve_agent(&settings, &selection)?;
            let generated = naming::generate_task_naming(
                &agent_id,
                &binary,
                &model,
                effort.as_deref(),
                &naming_task.prompt,
                std::path::Path::new(&project_root),
            );
            let suffix: String = naming_task.id.chars().take(6).collect();
            let branch_name = format!("task/{}-{suffix}", generated.branch);
            let changed = {
                let db_state = naming_app.state::<DbState>();
                let conn = db_state
                    .0
                    .lock()
                    .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
                db::apply_generated_naming(
                    &conn,
                    &naming_task.id,
                    &naming_task.title,
                    &generated.title,
                    &branch_name,
                    &now(),
                )?
            };
            if changed > 0 {
                let _ = naming_app.emit(execution::TASK_EVENT_CHANNEL, execution::TaskEventPayload {
                    task_id: naming_task.id.clone(), turn_id: String::new(), event_type: "forge.task.named".into(),
                    raw: serde_json::json!({"title": generated.title, "branchName": branch_name}), status: None, diff: None, error: None,
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("[forge] task naming failed: {error}");
        }
    });
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelCatalog {
    pub agent_id: String,
    pub agent_label: String,
    pub models: Vec<ModelOption>,
    pub selected_model: String,
    pub selected_effort: String,
    pub available: bool,
    pub error: Option<String>,
    pub refreshed_at: String,
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

pub(crate) fn extract_json_objects(text: &str) -> Vec<serde_json::Value> {
    let mut objects = Vec::new();
    let mut start = None;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(begin) = start.take() {
                        if let Ok(value) = serde_json::from_str(&text[begin..=index]) {
                            objects.push(value);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    objects
}

fn fetch_opencode_models(opencode_bin: &str) -> Result<Vec<ModelOption>, AppError> {
    let output = Command::new(opencode_bin)
        .args(["models", "--refresh", "--verbose"])
        .output()
        .map_err(|error| {
            AppError::InvalidOperation(format!(
                "Failed to run '{opencode_bin} models --refresh': {error}"
            ))
        })?;
    if !output.status.success() {
        return Err(AppError::InvalidOperation(format!(
            "'{opencode_bin} models --refresh' exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let mut output_text = String::from_utf8_lossy(&output.stdout).into_owned();
    output_text.push_str(&String::from_utf8_lossy(&output.stderr));
    let mut models = Vec::new();
    for raw in extract_json_objects(&output_text) {
        let Some(id) = raw.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        let provider = raw
            .get("providerID")
            .and_then(|value| value.as_str())
            .unwrap_or("opencode");
        let full_id = format!("{provider}/{id}");
        let effort_levels = raw
            .get("variants")
            .and_then(|value| value.as_object())
            .map(|variants| {
                variants
                    .keys()
                    .map(|variant| EffortLevel {
                        id: variant.clone(),
                        description: "Provider model variant".to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        models.push(ModelOption {
            id: full_id,
            label: raw
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or(id)
                .to_string(),
            description: format!("Provided by {provider}"),
            default_effort: effort_levels
                .first()
                .map(|level| level.id.clone())
                .unwrap_or_default(),
            effort_levels,
        });
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    if models.is_empty() {
        return Err(AppError::InvalidOperation(
            "opencode returned no parseable models".to_string(),
        ));
    }
    Ok(models)
}

fn fetch_claude_models(claude_bin: &str) -> Result<Vec<ModelOption>, AppError> {
    let output = Command::new(claude_bin)
        .arg("--help")
        .output()
        .map_err(|error| {
            AppError::InvalidOperation(format!("Failed to run '{claude_bin} --help': {error}"))
        })?;
    if !output.status.success() {
        return Err(AppError::InvalidOperation(format!(
            "'{claude_bin} --help' exited with status {}",
            output.status
        )));
    }

    // Claude Code currently exposes aliases, not a model-list command. Read
    // the aliases documented by the installed binary so this remains current
    // as Claude adds or renames aliases.
    let help = String::from_utf8_lossy(&output.stdout);
    let mut in_model_option = false;
    let mut model_lines = Vec::new();
    for line in help.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--model") {
            in_model_option = true;
        } else if in_model_option && trimmed.starts_with('-') {
            break;
        }
        if in_model_option {
            model_lines.push(line);
        }
    }
    let model_line = model_lines.join(" ");
    let mut ids = model_line
        .split('`')
        .enumerate()
        .filter_map(|(index, value)| (index % 2 == 1).then_some(value))
        .filter(|value| !value.is_empty() && !value.starts_with('<'))
        .map(str::to_string)
        .collect::<Vec<_>>();
    // Some builds render examples with single or double quotes instead.
    if ids.is_empty() {
        ids = model_line
            .split('\'')
            .enumerate()
            .filter_map(|(index, value)| (index % 2 == 1).then_some(value))
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    if ids.is_empty() {
        ids = model_line
            .split('"')
            .enumerate()
            .filter_map(|(index, value)| (index % 2 == 1).then_some(value))
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Err(AppError::InvalidOperation(
            "Claude Code did not advertise any model aliases".to_string(),
        ));
    }
    Ok(ids
        .into_iter()
        .map(|id| ModelOption {
            label: id.clone(),
            id,
            description: "Alias advertised by the installed Claude Code CLI".to_string(),
            effort_levels: ["low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .map(|id| EffortLevel {
                    id: id.to_string(),
                    description: "Claude Code effort level".to_string(),
                })
                .collect(),
            default_effort: "medium".to_string(),
        })
        .collect())
}

fn health_for_agent(agent_id: &str, binary: &str) -> runner::HealthStatus {
    if agent_id == "codex" {
        runner::health_check(binary)
    } else {
        runner::binary_health_check(binary)
    }
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
        available: true,
        error: None,
        refreshed_at: now(),
    })
}

#[tauri::command]
pub fn list_agent_catalogs(
    state: State<'_, DbState>,
    cache: State<'_, AgentCatalogState>,
) -> Result<Vec<AgentModelCatalog>, AppError> {
    let settings = {
        let conn = state
            .0
            .lock()
            .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
        db::select_settings(&conn)?
    };
    let discoveries = [
        (
            "codex",
            "Codex",
            settings.codex_model.clone(),
            settings.codex_effort.clone(),
            fetch_codex_models(&settings.codex_bin),
        ),
        (
            "claude",
            "Claude Code",
            settings.claude_model.clone(),
            String::new(),
            fetch_claude_models(&settings.claude_bin),
        ),
        (
            "opencode",
            "opencode",
            settings.opencode_model.clone(),
            String::new(),
            fetch_opencode_models(&settings.opencode_bin),
        ),
    ];
    let mut refreshed = Vec::new();
    let mut state_cache = cache.0.lock().map_err(|_| {
        AppError::InvalidOperation("Agent catalog cache is unavailable".to_string())
    })?;
    for (agent_id, label, selected_model, selected_effort, result) in discoveries {
        let catalog = match result {
            Ok(models) => AgentModelCatalog {
                agent_id: agent_id.to_string(),
                agent_label: label.to_string(),
                models,
                selected_model,
                selected_effort,
                available: true,
                error: None,
                refreshed_at: now(),
            },
            Err(error) => {
                let message = error.to_string();
                let mut previous = state_cache.remove(agent_id).unwrap_or(AgentModelCatalog {
                    agent_id: agent_id.to_string(),
                    agent_label: label.to_string(),
                    models: Vec::new(),
                    selected_model: selected_model.clone(),
                    selected_effort: selected_effort.clone(),
                    available: false,
                    error: None,
                    refreshed_at: now(),
                });
                previous.selected_model = selected_model;
                previous.selected_effort = selected_effort;
                previous.available = false;
                previous.error = Some(message);
                previous.refreshed_at = now();
                previous
            }
        };
        state_cache.insert(agent_id.to_string(), catalog.clone());
        refreshed.push(catalog);
    }
    Ok(refreshed)
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
    default_agent: Option<String>,
    claude_bin: Option<String>,
    claude_model: Option<String>,
    opencode_bin: Option<String>,
    opencode_model: Option<String>,
) -> Result<AppSettings, AppError> {
    let codex_bin = codex_bin.trim().to_string();
    let git_bin = git_bin.trim().to_string();
    let codex_model = codex_model.trim().to_string();
    let codex_effort = codex_effort.trim().to_string();
    let default_agent = default_agent
        .unwrap_or_else(|| "codex".to_string())
        .trim()
        .to_string();
    let claude_bin = claude_bin
        .unwrap_or_else(|| "claude".to_string())
        .trim()
        .to_string();
    let claude_model = claude_model
        .unwrap_or_else(|| "sonnet".to_string())
        .trim()
        .to_string();
    let opencode_bin = opencode_bin
        .unwrap_or_else(|| "opencode".to_string())
        .trim()
        .to_string();
    let opencode_model = opencode_model
        .unwrap_or_else(|| "anthropic/claude-sonnet-4-5".to_string())
        .trim()
        .to_string();
    if codex_bin.is_empty()
        || git_bin.is_empty()
        || claude_bin.is_empty()
        || opencode_bin.is_empty()
    {
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
    if !matches!(default_agent.as_str(), "codex" | "claude" | "opencode") {
        return Err(AppError::InvalidOperation(
            "Unknown default agent".to_string(),
        ));
    }
    let settings = AppSettings {
        codex_bin,
        git_bin,
        max_concurrent_tasks,
        merge_on_confirm,
        codex_model,
        codex_effort,
        default_agent,
        claude_bin,
        claude_model,
        opencode_bin,
        opencode_model,
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
        agents: vec![
            health_for_agent("claude", &settings.claude_bin),
            health_for_agent("opencode", &settings.opencode_bin),
        ],
        git,
    })
}

// ---------------------------------------------------------------------------
// Editor commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_editors() -> Result<Vec<EditorInfo>, AppError> {
    Ok(editors::detect_editors())
}

#[tauri::command]
pub fn open_worktree_in_editor(
    state: State<'_, DbState>,
    app: tauri::AppHandle,
    task_id: String,
    editor_id: String,
) -> Result<Task, AppError> {
    let settings = execution::load_settings(&app)?;
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".into()))?;
    let task = db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' not found")))?;
    let project = db::select_project(&conn, &task.project_id)?
        .ok_or_else(|| AppError::NotFound(format!("Project '{}' not found", task.project_id)))?;
    let worktree_path = task
        .worktree_path
        .as_deref()
        .map(std::path::Path::new)
        .ok_or_else(|| {
            AppError::InvalidOperation("This task has no worktree yet — run it first".into())
        })?;
    let branch_name = task.branch_name.as_deref().ok_or_else(|| {
        AppError::InvalidOperation("This task has no worktree yet — run it first".into())
    })?;
    WorktreeManager::new(settings.git_bin).ensure_worktree(
        std::path::Path::new(&project.root_path),
        worktree_path,
        branch_name,
    )?;
    editors::launch_editor(&editor_id, worktree_path)?;
    db::select_task(&conn, &task_id)?
        .ok_or_else(|| AppError::NotFound(format!("Task '{task_id}' disappeared")))
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
    agent_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Task, AppError> {
    execution::start_task(
        &app,
        &runs,
        task_id,
        AgentSelection {
            agent_id,
            model,
            effort,
        },
    )
}

/// Reset a failed or cancelled task and run it again from a clean state.
#[tauri::command]
pub fn retry_task(
    app: tauri::AppHandle,
    runs: tauri::State<'_, RunState>,
    task_id: String,
    agent_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Task, AppError> {
    execution::retry_task(
        &app,
        &runs,
        task_id,
        AgentSelection {
            agent_id,
            model,
            effort,
        },
    )
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
    line_end_number: Option<i64>,
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
        (None, None) if line_end_number.is_none() => Ok(()),
        (Some(line), Some("old" | "new"))
            if line > 0 && line_end_number.is_none_or(|end| end >= line) =>
        {
            Ok(())
        }
        (Some(line), _) if line <= 0 => Err(AppError::InvalidOperation(
            "Review comment line number must be positive".to_string(),
        )),
        (Some(_), _) => Err(AppError::InvalidOperation(
            "Line comments must specify side 'old' or 'new'".to_string(),
        )),
        (None, Some(_)) => Err(AppError::InvalidOperation(
            "File-level comments cannot specify a diff side".to_string(),
        )),
        (None, None) => Err(AppError::InvalidOperation(
            "File-level comments cannot specify a line range".to_string(),
        )),
    }
}

#[tauri::command]
pub fn add_review_comment(
    state: State<'_, DbState>,
    task_id: String,
    file_path: String,
    line_number: Option<i64>,
    line_end_number: Option<i64>,
    side: Option<String>,
    body: String,
) -> Result<ReviewComment, AppError> {
    validate_comment_anchor(
        &file_path,
        line_number,
        line_end_number,
        side.as_deref(),
        &body,
    )?;
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
        line_end_number,
        side,
        body: body.trim().to_string(),
        resolved: false,
        assigned_agent_id: None,
        assigned_model: None,
        assigned_effort: None,
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
pub fn assign_review_comment(
    state: State<'_, DbState>,
    comment_id: String,
    agent_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<ReviewComment, AppError> {
    if let Some(agent) = agent_id.as_deref() {
        if !matches!(agent, "codex" | "claude" | "opencode") {
            return Err(AppError::InvalidOperation(format!(
                "Unsupported agent '{agent}'"
            )));
        }
    }
    let conn = state
        .0
        .lock()
        .map_err(|_| AppError::InvalidOperation("Database is unavailable".to_string()))?;
    db::assign_review_comment(
        &conn,
        &comment_id,
        agent_id.as_deref(),
        model.as_deref(),
        effort.as_deref(),
    )?
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
pub fn submit_review(
    app: tauri::AppHandle,
    task_id: String,
    reviewer_note: Option<String>,
    agent_id: Option<String>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Task, AppError> {
    execution::submit_review(
        &app,
        task_id,
        reviewer_note,
        AgentSelection {
            agent_id,
            model,
            effort,
        },
    )
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
        validate_comment_anchor("src/main.rs", Some(4), None, Some("new"), "Please adjust")
            .unwrap();
        validate_comment_anchor("src/main.rs", Some(4), Some(6), Some("new"), "range").unwrap();
        validate_comment_anchor("src/main.rs", None, None, None, "File-level note").unwrap();
        assert!(
            validate_comment_anchor("src/main.rs", Some(0), None, Some("new"), "body").is_err()
        );
        assert!(
            validate_comment_anchor("src/main.rs", Some(4), Some(3), Some("new"), "body").is_err()
        );
        assert!(
            validate_comment_anchor("src/main.rs", Some(1), None, Some("right"), "body").is_err()
        );
        assert!(validate_comment_anchor("src/main.rs", None, None, Some("old"), "body").is_err());
        assert!(validate_comment_anchor("src/main.rs", Some(1), None, Some("old"), "  ").is_err());
    }

    #[test]
    fn model_discovery_json_extractor_ignores_cli_text_and_keeps_nested_objects() {
        let values = extract_json_objects(
            "Models cache refreshed\n{\"id\":\"provider/model\",\"variants\":{\"high\":{\"reasoningEffort\":\"high\"}}}\nprovider/model",
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["id"], "provider/model");
        assert_eq!(values[0]["variants"]["high"]["reasoningEffort"], "high");
    }

    #[test]
    fn claude_model_discovery_reads_aliases_from_help_text() {
        let help =
            "  --model <model> Model alias (e.g. 'sonnet', 'opus')\n  --output-format <format>";
        let model_line = help
            .lines()
            .take_while(|line| !line.trim_start().starts_with("--output-format"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(model_line.contains("sonnet"));
        assert!(model_line.contains("opus"));
    }
}
