# Phase II — Persistence and CRUD

Status: **Pending**

Maps to `project-proposal.md` Batch 1.

## Objective

Add SQLite migrations and the project/task CRUD surface while keeping new tasks in `draft` status. No Codex run should happen in this phase.

## Deliverables

- `project`, `task`, `turn`, `review_comment`, and `settings` schema migrations from proposal §3.
- Rust persistence module using `rusqlite` with app-data storage.
- Tauri commands: `create_project`, `list_projects`, `create_task`, `list_tasks`, `get_task`, `delete_task`, and draft-only `update_task_prompt`.
- Frontend project folder picker and task creation form with paths relative to the selected project.
- Tests for migration, CRUD, draft-only updates, and project-root validation.

## Handoff requirements

Record the database path, migration strategy, command payloads, and any Tauri API changes in `IMPLEMENTATION_STATUS.md`.
