use serde::{Deserialize, Serialize};

/// A folder the user has registered as a project. Must be a git repository
/// (enforced in Phase III).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub root_path: String,
    pub created_at: String,
}

/// A unit of work assigned to Codex. `file_refs` is serialised as a JSON
/// array in SQLite and deserialised back to `Vec<String>` here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    /// Paths relative to the project root.
    pub file_refs: Vec<String>,
    /// draft | running | awaiting_review | changes_requested | approved | failed | cancelled
    pub status: String,
    pub base_ref: Option<String>,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    pub agent_thread_id: Option<String>,
    /// Cumulative unified diff against `base_ref`, captured after each turn.
    pub diff: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A review note anchored to the cumulative diff for a task turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: String,
    pub task_id: String,
    pub turn_id: String,
    pub file_path: String,
    /// One-based line number on `side`; `None` denotes a file-level comment.
    pub line_number: Option<i64>,
    /// `old` or `new` for line comments; `None` for file-level comments.
    pub side: Option<String>,
    pub body: String,
    pub resolved: bool,
    pub created_at: String,
}

/// Persisted operational settings. The sandbox is deliberately read-only in
/// the UI and is not stored: v1 always runs Codex with `workspace-write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub codex_bin: String,
    pub git_bin: String,
    pub max_concurrent_tasks: usize,
    pub merge_on_confirm: bool,
    pub sandbox_mode: String,
}

/// A single agent turn associated with a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTurn {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub prompt: String,
    pub status: String,
    pub log_path: String,
    pub started_at: String,
    pub ended_at: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            codex_bin: "codex".to_string(),
            git_bin: "git".to_string(),
            max_concurrent_tasks: 2,
            merge_on_confirm: false,
            sandbox_mode: "workspace-write".to_string(),
        }
    }
}
