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
