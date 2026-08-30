use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    error::AppError,
    models::{AppSettings, Project, ReviewComment, Task},
};

// ---------------------------------------------------------------------------
// Startup recovery
// ---------------------------------------------------------------------------

/// On every startup the in-memory `RunState` is empty, so any task that is
/// still marked `running` in the database was orphaned by a previous crash or
/// forced-quit. Mark every such task `failed` and close any of their open turn
/// rows so the UI can present a consistent, actionable state.
pub fn mark_dangling_tasks_failed(conn: &Connection, now: &str) -> Result<usize, AppError> {
    // Close open turn rows first (FK-ordered).
    conn.execute(
        "UPDATE turn SET status = 'failed', ended_at = ?1
         WHERE status = 'running'",
        params![now],
    )?;
    // Mark the tasks themselves failed.
    let changed = conn.execute(
        "UPDATE task SET status = 'failed', updated_at = ?1
         WHERE status = 'running'",
        params![now],
    )?;
    Ok(changed)
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Migration 1 — core schema (all five tables from proposal §3).
const MIGRATION_1: &str = "
CREATE TABLE IF NOT EXISTS project (
    id         TEXT PRIMARY KEY,
    root_path  TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task (
    id               TEXT PRIMARY KEY,
    project_id       TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    prompt           TEXT NOT NULL,
    file_refs        TEXT NOT NULL DEFAULT '[]',
    status           TEXT NOT NULL DEFAULT 'draft',
    base_ref         TEXT,
    worktree_path    TEXT,
    branch_name      TEXT,
    agent_thread_id  TEXT,
    diff             TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_project ON task(project_id);

CREATE TABLE IF NOT EXISTS turn (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    prompt     TEXT NOT NULL,
    status     TEXT NOT NULL,
    log_path   TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at   TEXT
);

CREATE TABLE IF NOT EXISTS review_comment (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    turn_id     TEXT NOT NULL REFERENCES turn(id) ON DELETE CASCADE,
    file_path   TEXT NOT NULL,
    line_number INTEGER,
    side        TEXT,
    body        TEXT NOT NULL,
    resolved    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

pub fn open(path: &std::path::Path) -> Result<Connection, AppError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    run_migrations(&conn)?;
    Ok(conn)
}

fn run_migrations(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_version (version INTEGER PRIMARY KEY);",
    )?;
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
        [],
        |r| r.get(0),
    )?;
    if version < 1 {
        conn.execute_batch(MIGRATION_1)?;
        conn.execute("INSERT OR REPLACE INTO _schema_version VALUES (1)", [])?;
    }
    if version < 2 {
        // Migration 2 adds the cached cumulative review diff.  Keep this as a
        // separate migration so databases created by Phase II upgrade safely.
        let has_diff: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('task') WHERE name = 'diff'",
            [],
            |r| r.get(0),
        )?;
        if !has_diff {
            conn.execute("ALTER TABLE task ADD COLUMN diff TEXT", [])?;
        }
        conn.execute("INSERT OR REPLACE INTO _schema_version VALUES (2)", [])?;
    }
    if version < 3 {
        conn.execute_batch(
            "INSERT OR IGNORE INTO settings (key, value) VALUES
                ('codex_bin', 'codex'),
                ('git_bin', 'git'),
                ('max_concurrent_tasks', '2'),
                ('merge_on_confirm', 'false');
             INSERT OR REPLACE INTO _schema_version VALUES (3);",
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub fn select_settings(conn: &Connection) -> Result<AppSettings, AppError> {
    let mut settings = AppSettings::default();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            "codex_bin" if !value.trim().is_empty() => settings.codex_bin = value,
            "git_bin" if !value.trim().is_empty() => settings.git_bin = value,
            "max_concurrent_tasks" => {
                if let Ok(value @ 1..=16) = value.parse::<usize>() {
                    settings.max_concurrent_tasks = value;
                }
            }
            "merge_on_confirm" => settings.merge_on_confirm = value == "true",
            _ => {}
        }
    }
    Ok(settings)
}

pub fn replace_settings(conn: &mut Connection, settings: &AppSettings) -> Result<(), AppError> {
    let tx = conn.transaction()?;
    for (key, value) in [
        ("codex_bin", settings.codex_bin.clone()),
        ("git_bin", settings.git_bin.clone()),
        (
            "max_concurrent_tasks",
            settings.max_concurrent_tasks.to_string(),
        ),
        ("merge_on_confirm", settings.merge_on_confirm.to_string()),
    ] {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Project CRUD
// ---------------------------------------------------------------------------

pub fn insert_project(conn: &Connection, project: &Project) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO project (id, root_path, created_at) VALUES (?1, ?2, ?3)",
        params![project.id, project.root_path, project.created_at],
    )?;
    Ok(())
}

pub fn select_all_projects(conn: &Connection) -> Result<Vec<Project>, AppError> {
    let mut stmt =
        conn.prepare("SELECT id, root_path, created_at FROM project ORDER BY created_at DESC")?;
    let projects = stmt
        .query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                root_path: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(projects)
}

pub fn select_project(conn: &Connection, id: &str) -> Result<Option<Project>, AppError> {
    conn.query_row(
        "SELECT id, root_path, created_at FROM project WHERE id = ?1",
        params![id],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                root_path: row.get(1)?,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

// ---------------------------------------------------------------------------
// Task CRUD
// ---------------------------------------------------------------------------

/// Column order must match every SELECT that uses this mapping.
fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let file_refs_json: String = row.get(4)?;
    let file_refs: Vec<String> = serde_json::from_str(&file_refs_json).unwrap_or_default();
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        prompt: row.get(3)?,
        file_refs,
        status: row.get(5)?,
        base_ref: row.get(6)?,
        worktree_path: row.get(7)?,
        branch_name: row.get(8)?,
        agent_thread_id: row.get(9)?,
        diff: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// SELECT clause that matches `row_to_task`'s column indices.
const TASK_SELECT: &str = "SELECT id, project_id, title, prompt, file_refs, status,
            base_ref, worktree_path, branch_name, agent_thread_id,
            diff, created_at, updated_at
     FROM task";

pub fn insert_task(conn: &Connection, task: &Task) -> Result<(), AppError> {
    let file_refs_json = serde_json::to_string(&task.file_refs)
        .map_err(|e| AppError::InvalidOperation(e.to_string()))?;
    conn.execute(
        "INSERT INTO task (id, project_id, title, prompt, file_refs, status,
                           base_ref, worktree_path, branch_name, agent_thread_id,
                           diff, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            task.id,
            task.project_id,
            task.title,
            task.prompt,
            file_refs_json,
            task.status,
            task.base_ref,
            task.worktree_path,
            task.branch_name,
            task.agent_thread_id,
            task.diff,
            task.created_at,
            task.updated_at,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Execution persistence
// ---------------------------------------------------------------------------

pub fn insert_turn(
    conn: &Connection,
    id: &str,
    task_id: &str,
    kind: &str,
    prompt: &str,
    status: &str,
    log_path: &str,
    started_at: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO turn (id, task_id, kind, prompt, status, log_path, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, task_id, kind, prompt, status, log_path, started_at],
    )?;
    Ok(())
}

pub fn prepare_task_run(
    conn: &Connection,
    task_id: &str,
    base_ref: &str,
    worktree_path: &str,
    branch_name: &str,
    now: &str,
) -> Result<(), AppError> {
    let changed = conn.execute(
        "UPDATE task
         SET status = 'running', base_ref = ?1, worktree_path = ?2,
             branch_name = ?3, agent_thread_id = NULL, diff = NULL, updated_at = ?4
         WHERE id = ?5 AND status = 'draft'",
        params![base_ref, worktree_path, branch_name, now, task_id],
    )?;
    if changed == 0 {
        return Err(AppError::InvalidOperation(format!(
            "Task '{task_id}' is no longer a draft"
        )));
    }
    Ok(())
}

pub fn set_task_thread_id(
    conn: &Connection,
    task_id: &str,
    thread_id: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE task SET agent_thread_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![thread_id, now, task_id],
    )?;
    Ok(())
}

pub fn finish_turn(
    conn: &Connection,
    turn_id: &str,
    status: &str,
    ended_at: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE turn SET status = ?1, ended_at = ?2 WHERE id = ?3",
        params![status, ended_at, turn_id],
    )?;
    Ok(())
}

pub fn finish_task(
    conn: &Connection,
    task_id: &str,
    status: &str,
    diff: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE task SET status = ?1, diff = ?2, updated_at = ?3 WHERE id = ?4",
        params![status, diff, now, task_id],
    )?;
    Ok(())
}

pub fn reset_task_run(conn: &Connection, task_id: &str, now: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE task
         SET status = 'draft', base_ref = NULL, worktree_path = NULL,
             branch_name = NULL, agent_thread_id = NULL, diff = NULL, updated_at = ?1
         WHERE id = ?2",
        params![now, task_id],
    )?;
    Ok(())
}

pub fn begin_follow_up(
    conn: &Connection,
    task_id: &str,
    turn_id: &str,
    prompt: &str,
    log_path: &str,
    now: &str,
) -> Result<(), AppError> {
    let changed = conn.execute(
        "UPDATE task SET status = 'running', updated_at = ?1
         WHERE id = ?2 AND status IN ('awaiting_review', 'changes_requested')",
        params![now, task_id],
    )?;
    if changed == 0 {
        return Err(AppError::InvalidOperation(format!(
            "Task '{task_id}' is not ready for a follow-up turn"
        )));
    }

    if let Err(error) = insert_turn(
        conn,
        turn_id,
        task_id,
        "follow_up",
        prompt,
        "running",
        log_path,
        now,
    ) {
        let _ = conn.execute(
            "UPDATE task SET status = 'changes_requested', updated_at = ?1 WHERE id = ?2",
            params![now, task_id],
        );
        return Err(error);
    }
    Ok(())
}

pub fn fail_follow_up_start(
    conn: &Connection,
    task_id: &str,
    turn_id: &str,
    now: &str,
) -> Result<(), AppError> {
    finish_turn(conn, turn_id, "failed", now)?;
    conn.execute(
        "UPDATE task SET status = 'changes_requested', updated_at = ?1 WHERE id = ?2",
        params![now, task_id],
    )?;
    Ok(())
}

pub fn approve_task(
    conn: &Connection,
    task_id: &str,
    keep_branch: bool,
    now: &str,
) -> Result<(), AppError> {
    let changed = conn.execute(
        "UPDATE task
         SET status = 'approved', worktree_path = NULL,
             branch_name = CASE WHEN ?1 THEN branch_name ELSE NULL END,
             updated_at = ?2
         WHERE id = ?3 AND status = 'awaiting_review'",
        params![keep_branch, now, task_id],
    )?;
    if changed == 0 {
        return Err(AppError::InvalidOperation(format!(
            "Task '{task_id}' is no longer awaiting review"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Review comments
// ---------------------------------------------------------------------------

pub fn select_latest_turn_id(conn: &Connection, task_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT id FROM turn WHERE task_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT 1",
        params![task_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

pub fn insert_review_comment(conn: &Connection, comment: &ReviewComment) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO review_comment
         (id, task_id, turn_id, file_path, line_number, side, body, resolved, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            comment.id,
            comment.task_id,
            comment.turn_id,
            comment.file_path,
            comment.line_number,
            comment.side,
            comment.body,
            i64::from(comment.resolved),
            comment.created_at,
        ],
    )?;
    Ok(())
}

pub fn select_review_comments(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<ReviewComment>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, task_id, turn_id, file_path, line_number, side, body, resolved, created_at
         FROM review_comment WHERE task_id = ?1 ORDER BY created_at ASC, rowid ASC",
    )?;
    let comments = stmt
        .query_map(params![task_id], row_to_review_comment)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(comments)
}

pub fn select_unresolved_review_comments(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<ReviewComment>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, task_id, turn_id, file_path, line_number, side, body, resolved, created_at
         FROM review_comment
         WHERE task_id = ?1 AND resolved = 0
         ORDER BY created_at ASC, rowid ASC",
    )?;
    let comments = stmt
        .query_map(params![task_id], row_to_review_comment)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(comments)
}

pub fn resolve_review_comments(
    conn: &Connection,
    task_id: &str,
    comment_ids: &[String],
) -> Result<(), AppError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let mut stmt =
            conn.prepare("UPDATE review_comment SET resolved = 1 WHERE task_id = ?1 AND id = ?2")?;
        for comment_id in comment_ids {
            stmt.execute(params![task_id, comment_id])?;
        }
        Ok::<_, AppError>(())
    })();
    match result {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    Ok(())
}

pub fn resolve_review_comment(
    conn: &Connection,
    comment_id: &str,
) -> Result<Option<ReviewComment>, AppError> {
    let changed = conn.execute(
        "UPDATE review_comment SET resolved = 1 WHERE id = ?1",
        params![comment_id],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    conn.query_row(
        "SELECT id, task_id, turn_id, file_path, line_number, side, body, resolved, created_at
         FROM review_comment WHERE id = ?1",
        params![comment_id],
        row_to_review_comment,
    )
    .optional()
    .map_err(AppError::from)
}

fn row_to_review_comment(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewComment> {
    Ok(ReviewComment {
        id: row.get(0)?,
        task_id: row.get(1)?,
        turn_id: row.get(2)?,
        file_path: row.get(3)?,
        line_number: row.get(4)?,
        side: row.get(5)?,
        body: row.get(6)?,
        resolved: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
    })
}

pub fn select_tasks_by_project(conn: &Connection, project_id: &str) -> Result<Vec<Task>, AppError> {
    let sql = format!("{TASK_SELECT} WHERE project_id = ?1 ORDER BY created_at DESC");
    let mut stmt = conn.prepare(&sql)?;
    let tasks = stmt
        .query_map(params![project_id], row_to_task)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tasks)
}

pub fn select_task(conn: &Connection, id: &str) -> Result<Option<Task>, AppError> {
    let sql = format!("{TASK_SELECT} WHERE id = ?1");
    conn.query_row(&sql, params![id], row_to_task)
        .optional()
        .map_err(AppError::from)
}

pub fn select_turns_for_task(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<crate::models::TaskTurn>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, task_id, kind, prompt, status, log_path, started_at, ended_at
         FROM turn WHERE task_id = ?1 ORDER BY started_at ASC, rowid ASC",
    )?;
    let turns = stmt
        .query_map(params![task_id], |row| {
            Ok(crate::models::TaskTurn {
                id: row.get(0)?,
                task_id: row.get(1)?,
                kind: row.get(2)?,
                prompt: row.get(3)?,
                status: row.get(4)?,
                log_path: row.get(5)?,
                started_at: row.get(6)?,
                ended_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(turns)
}

pub fn delete_task_by_id(conn: &Connection, id: &str) -> Result<bool, AppError> {
    let n = conn.execute("DELETE FROM task WHERE id = ?1", params![id])?;
    Ok(n > 0)
}

/// Update mutable prompt fields. **Only allowed when `status = 'draft'`.**
/// Uses COALESCE so callers may pass `None` to leave a field unchanged.
pub fn update_draft_task(
    conn: &Connection,
    id: &str,
    title: Option<&str>,
    prompt: Option<&str>,
    file_refs: Option<&[String]>,
    now: &str,
) -> Result<(), AppError> {
    let status: Option<String> = conn
        .query_row("SELECT status FROM task WHERE id = ?1", params![id], |r| {
            r.get(0)
        })
        .optional()?;

    match status.as_deref() {
        None => {
            return Err(AppError::NotFound(format!("Task '{id}' not found")));
        }
        Some(s) if s != "draft" => {
            return Err(AppError::InvalidOperation(format!(
                "Task '{id}' has status '{s}'; only draft tasks may be updated"
            )));
        }
        _ => {}
    }

    let file_refs_json: Option<String> = file_refs
        .map(|refs| {
            serde_json::to_string(refs).map_err(|e| AppError::InvalidOperation(e.to_string()))
        })
        .transpose()?;

    conn.execute(
        "UPDATE task
         SET title      = COALESCE(?1, title),
             prompt     = COALESCE(?2, prompt),
             file_refs  = COALESCE(?3, file_refs),
             updated_at = ?4
         WHERE id = ?5",
        params![title, prompt, file_refs_json, now, id],
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Connection) {
        let dir = TempDir::new().expect("create tempdir");
        let path = dir.path().join("test.db");
        let conn = open(&path).expect("open db");
        (dir, conn)
    }

    fn project(id: &str, root: &str) -> Project {
        Project {
            id: id.to_string(),
            root_path: root.to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn task(id: &str, project_id: &str) -> Task {
        Task {
            id: id.to_string(),
            project_id: project_id.to_string(),
            title: "Test task".to_string(),
            prompt: "Do something useful".to_string(),
            file_refs: vec!["src/main.rs".to_string()],
            status: "draft".to_string(),
            base_ref: None,
            worktree_path: None,
            branch_name: None,
            agent_thread_id: None,
            diff: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn migration_creates_all_tables() {
        let (_dir, conn) = setup();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table'
                 AND name IN ('project','task','turn','review_comment','settings')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 5,
            "all five schema tables must exist after migration"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let (_dir, conn) = setup();
        run_migrations(&conn).expect("second migration pass must be a no-op");
    }

    #[test]
    fn settings_defaults_and_atomic_update_roundtrip() {
        let (_dir, mut conn) = setup();
        assert_eq!(select_settings(&conn).unwrap(), AppSettings::default());

        let updated = AppSettings {
            codex_bin: "C:/tools/codex.exe".to_string(),
            git_bin: "C:/tools/git.exe".to_string(),
            max_concurrent_tasks: 6,
            merge_on_confirm: true,
            ..AppSettings::default()
        };
        replace_settings(&mut conn, &updated).unwrap();
        assert_eq!(select_settings(&conn).unwrap(), updated);
        assert_eq!(
            select_settings(&conn).unwrap().sandbox_mode,
            "workspace-write"
        );
    }

    #[test]
    fn invalid_persisted_concurrency_falls_back_safely() {
        let (_dir, conn) = setup();
        conn.execute(
            "UPDATE settings SET value = '0' WHERE key = 'max_concurrent_tasks'",
            [],
        )
        .unwrap();
        assert_eq!(select_settings(&conn).unwrap().max_concurrent_tasks, 2);
    }

    #[test]
    fn migration_adds_cached_diff_column() {
        let (_dir, conn) = setup();
        let has_diff: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('task') WHERE name = 'diff'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(has_diff);
    }

    #[test]
    fn project_insert_and_list() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        let all = select_all_projects(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "p1");
    }

    #[test]
    fn project_duplicate_path_rejected() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        let err = insert_project(&conn, &project("p2", "/tmp/repo"));
        assert!(
            err.is_err(),
            "duplicate root_path must violate UNIQUE constraint"
        );
    }

    #[test]
    fn task_crud_roundtrip() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();

        let t = task("t1", "p1");
        insert_task(&conn, &t).unwrap();

        let loaded = select_task(&conn, "t1").unwrap().expect("task must exist");
        assert_eq!(loaded.title, "Test task");
        assert_eq!(loaded.file_refs, vec!["src/main.rs"]);
        assert_eq!(loaded.status, "draft");

        let list = select_tasks_by_project(&conn, "p1").unwrap();
        assert_eq!(list.len(), 1);

        let deleted = delete_task_by_id(&conn, "t1").unwrap();
        assert!(deleted);
        assert!(select_task(&conn, "t1").unwrap().is_none());
    }

    #[test]
    fn draft_task_update_allowed() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        insert_task(&conn, &task("t1", "p1")).unwrap();

        update_draft_task(
            &conn,
            "t1",
            Some("Updated title"),
            None,
            Some(&["docs/README.md".to_string()]),
            "2024-06-01T00:00:00Z",
        )
        .expect("draft update must succeed");

        let loaded = select_task(&conn, "t1").unwrap().unwrap();
        assert_eq!(loaded.title, "Updated title");
        assert_eq!(loaded.file_refs, vec!["docs/README.md"]);
    }

    #[test]
    fn non_draft_task_update_rejected() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        let mut t = task("t1", "p1");
        t.status = "running".to_string();
        insert_task(&conn, &t).unwrap();

        let err = update_draft_task(
            &conn,
            "t1",
            Some("New title"),
            None,
            None,
            "2024-06-01T00:00:00Z",
        );
        assert!(err.is_err());
        assert!(
            matches!(err.unwrap_err(), AppError::InvalidOperation(_)),
            "must be InvalidOperation for non-draft task"
        );
    }

    #[test]
    fn task_delete_cascades_to_turns() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        insert_task(&conn, &task("t1", "p1")).unwrap();

        conn.execute(
            "INSERT INTO turn (id, task_id, kind, prompt, status, log_path, started_at)
             VALUES ('r1','t1','initial','prompt','running','/tmp/log','2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        delete_task_by_id(&conn, "t1").unwrap();

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn WHERE task_id = 't1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "ON DELETE CASCADE must remove child turns");
    }

    #[test]
    fn review_comment_roundtrip_and_resolution() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        insert_task(&conn, &task("t1", "p1")).unwrap();
        insert_turn(
            &conn,
            "turn-1",
            "t1",
            "initial",
            "prompt",
            "completed",
            "/tmp/log",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        let comment = ReviewComment {
            id: "c1".to_string(),
            task_id: "t1".to_string(),
            turn_id: "turn-1".to_string(),
            file_path: "src/main.rs".to_string(),
            line_number: Some(12),
            side: Some("new".to_string()),
            body: "Handle the error here".to_string(),
            resolved: false,
            created_at: "2024-01-01T00:01:00Z".to_string(),
        };
        insert_review_comment(&conn, &comment).unwrap();

        assert_eq!(
            select_latest_turn_id(&conn, "t1").unwrap().as_deref(),
            Some("turn-1")
        );
        let listed = select_review_comments(&conn, "t1").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].resolved);

        let resolved = resolve_review_comment(&conn, "c1").unwrap().unwrap();
        assert!(resolved.resolved);
    }

    #[test]
    fn follow_up_lifecycle_resolves_only_submitted_comments() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        let mut t = task("t1", "p1");
        t.status = "awaiting_review".to_string();
        insert_task(&conn, &t).unwrap();
        insert_turn(
            &conn,
            "turn-1",
            "t1",
            "initial",
            "prompt",
            "completed",
            "/tmp/initial.log",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        for id in ["c1", "c2"] {
            insert_review_comment(
                &conn,
                &ReviewComment {
                    id: id.to_string(),
                    task_id: "t1".to_string(),
                    turn_id: "turn-1".to_string(),
                    file_path: "src/main.rs".to_string(),
                    line_number: None,
                    side: None,
                    body: format!("feedback {id}"),
                    resolved: false,
                    created_at: format!("2024-01-01T00:01:0{}Z", &id[1..]),
                },
            )
            .unwrap();
        }

        begin_follow_up(
            &conn,
            "t1",
            "turn-2",
            "follow-up prompt",
            "/tmp/follow-up.log",
            "2024-01-01T00:02:00Z",
        )
        .unwrap();
        assert_eq!(select_task(&conn, "t1").unwrap().unwrap().status, "running");

        resolve_review_comments(&conn, "t1", &["c1".to_string()]).unwrap();
        let unresolved = select_unresolved_review_comments(&conn, "t1").unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].id, "c2");
    }

    #[test]
    fn approval_clears_worktree_and_can_preserve_branch() {
        let (_dir, conn) = setup();
        insert_project(&conn, &project("p1", "/tmp/repo")).unwrap();
        let mut t = task("t1", "p1");
        t.status = "awaiting_review".to_string();
        t.worktree_path = Some("/tmp/worktree".to_string());
        t.branch_name = Some("task/t1".to_string());
        insert_task(&conn, &t).unwrap();

        approve_task(&conn, "t1", true, "2024-01-01T00:03:00Z").unwrap();
        let approved = select_task(&conn, "t1").unwrap().unwrap();
        assert_eq!(approved.status, "approved");
        assert!(approved.worktree_path.is_none());
        assert_eq!(approved.branch_name.as_deref(), Some("task/t1"));
    }
}
