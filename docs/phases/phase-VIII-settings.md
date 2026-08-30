# Phase VIII — Settings

Status: **Implemented; validation passed**

Maps to `project-proposal.md` Batch 7.

## Objective

Expose safe operational settings without allowing the user to weaken the v1 sandbox policy.

## Deliverables

- Codex and git binary path overrides.
- Detected versions and Codex health/auth status.
- Configurable maximum concurrent tasks.
- Merge-on-confirm toggle, defaulting to disabled.
- Read-only `workspace-write` sandbox display.

## Handoff requirements

Record defaults, validation, persistence behavior, and what restart is required for each setting.

## Implemented contract

Settings are stored in the existing SQLite `settings` table by migration 3 and exposed through typed `get_settings`, `update_settings`, and `check_system_health` commands. The settings dialog is available from both the activity rail and the Codex status row.

| Setting | Default | Validation | Takes effect |
|---|---|---|---|
| Codex binary | `codex` | Trimmed, non-empty executable name or path | Next initial/follow-up turn and health check |
| Git binary | `git` | Trimmed, non-empty executable name or path | Next project check or Git operation |
| Maximum concurrent tasks | `2` | Integer from 1 through 16 | Immediately for new reservations; active runs continue |
| Merge on confirm | Disabled | Boolean | Next confirmation screen; remains overridable per task |
| Sandbox | `workspace-write` | Read-only constant, never persisted from user input | Always |

Updates are atomic and persist across launches. No setting requires a restart. An operation snapshots its binary paths when it starts, so saving a new path does not mutate an already-running process. Lowering concurrency never cancels active tasks; it prevents new reservations until the active count falls below the new limit.

Health checks detect `codex --version`, `git --version`, API-key environment variables, and the local `codex login status` result. An invalid path can be saved deliberately so it can be diagnosed in the same screen; execution then fails with the existing actionable launch error.

The persisted values drive project repository validation, worktree creation and cleanup, cumulative diff capture, commits, merges, initial Codex turns, and resumed Codex turns. The confirmation checkbox initializes from `merge_on_confirm` but can still be changed for an individual task.

## Validation

- `cargo test --manifest-path src-tauri/Cargo.toml`: 56 passed.
- `npm test`: diff fixtures passed.
- `npm run build`: TypeScript and Vite production build passed.
- Visual smoke testing is pending when an in-app browser surface is available.
