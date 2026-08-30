# Forge v1 implementation status

This is the cross-conversation handoff file. The full phase specifications live in [docs/PHASES.md](docs/PHASES.md) and `docs/phases/`.

| Phase | Scope | Status |
|---|---|---|
| [I — Scaffold](docs/phases/phase-I-scaffold.md) | Tauri + React/Vite shell, plugins, capabilities | **Implemented; validation pending** |
| [II — Persistence and CRUD](docs/phases/phase-II-persistence-crud.md) | SQLite, projects, draft task CRUD | **Implemented; validation pending** |
| [III — Git worktrees](docs/phases/phase-III-git-worktrees.md) | Repository/worktree/diff/commit helpers | **Implemented; validation pending** |
| [IV — Codex runner](docs/phases/phase-IV-codex-runner.md) | Codex process, JSONL, logs, health, cancellation | **Implemented; validation pending** |
| [V — Execution orchestration](docs/phases/phase-V-execution-orchestration.md) | Run lifecycle, streaming, concurrency | **Implemented; validation passed** |
| [VI — Diff review UI](docs/phases/phase-VI-diff-review-ui.md) | Diff display, inline comments, review actions | Pending |
| [VII — Follow-up loop](docs/phases/phase-VII-follow-up-loop.md) | Resume turns, confirmation, cleanup | Pending |
| [VIII — Settings](docs/phases/phase-VIII-settings.md) | Binary status, concurrency, merge preferences | Pending |
| [IX — Packaging](docs/phases/phase-IX-packaging.md) | Windows installer and cross-platform smoke checks | Pending |

## Current handoff

- Current phase: **V — Execution orchestration** (implemented; validation passed)
- Next phase: **VI — Diff review UI**
- Proposal baseline: `project-proposal.md`, Batch 3
- Functional scope changes: the frontend is now an editor-first desktop toolchain UI. The dashboard/card shell was replaced with a compact activity rail, project/task tree, inline task composer, command palette, and `@` project-file / `/` command suggestions.
- UI validation: `npm run build` passes. In-app browser visual smoke test was unavailable in this environment because no browser surface is connected.
- Smoke-test item carried forward: verify empirically whether the pinned Codex CLI accepts `--json`/`--sandbox`/`--full-auto` on `resume` (see Phase IV handoff below)

---

## Phase II details

### Database

| Item | Value |
|---|---|
| Engine | SQLite via `rusqlite` (bundled feature, no system lib required) |
| File location | `{app_data_dir}/forge.db` — resolved at startup via `app.path().app_data_dir()` |
| WAL mode | Yes (`PRAGMA journal_mode=WAL`) |
| Foreign keys | On (`PRAGMA foreign_keys=ON`) — cascade deletes are active |
| Migration table | `_schema_version (version INTEGER PRIMARY KEY)` |
| Migration 1 | Creates `project`, `task`, `turn`, `review_comment`, `settings` |

### Rust module layout

```
src-tauri/src/
  lib.rs          — plugin init, DB setup in .setup(), invoke_handler registration
  main.rs         — binary entry point (unchanged)
  error.rs        — AppError enum; implements Display, Error, From<rusqlite::Error>, Serialize
  models.rs       — Project and Task structs (serde rename_all = "camelCase")
  db.rs           — open(), migrations, Project CRUD, Task CRUD, update_draft_task()
  commands.rs     — DbState(Mutex<Connection>), all 7 Tauri commands, validation helpers
```

### Tauri commands

All commands live in `src-tauri/src/commands.rs` and are registered with `tauri::generate_handler!`.

| Command | Rust signature | JS invoke name | Notes |
|---|---|---|---|
| `create_project` | `(root_path: String) → Project` | `create_project` | Validates path exists and is a directory; rejects duplicate root_path |
| `list_projects` | `() → Vec<Project>` | `list_projects` | Ordered by `created_at DESC` |
| `create_task` | `(project_id, title, prompt, file_refs) → Task` | `create_task` | Status always `draft`; validates file_refs are relative and contain no `..` |
| `list_tasks` | `(project_id: String) → Vec<Task>` | `list_tasks` | Ordered by `created_at DESC` |
| `get_task` | `(task_id: String) → Task` | `get_task` | Returns `NotFound` if missing |
| `delete_task` | `(task_id: String) → ()` | `delete_task` | Cascades to `turn` and `review_comment` via FK |
| `update_task_prompt` | `(task_id, title?, prompt?, file_refs?) → Task` | `update_task_prompt` | Rejected with `InvalidOperation` if task is not `draft` |

### JS/TS invoke parameter naming

Rust command parameters are **snake_case**; they must be passed with matching snake_case keys from JavaScript (Tauri v2 does **not** automatically convert camelCase to snake_case for command arguments). Return values use **camelCase** because the Rust structs carry `#[serde(rename_all = "camelCase")]`.

The `src/api.ts` wrapper layer handles this translation.

### Frontend module layout

```
src/
  types.ts                    — Project, Task, TaskStatus, CreateTaskInput, UpdateTaskInput
  api.ts                      — typed invoke() wrappers (snake_case args → camelCase response)
  store.ts                    — Zustand store: projects, tasks, loading flags, all actions
  App.tsx                     — editor-first shell; activity rail, project/task tree, task detail, command palette
  components/
    TaskCard.tsx              — compact selectable task-tree row with status and delete action
    NewTaskDialog.tsx         — inline task composer with project-file `@` picker and `/` command palette
```

### New dependencies added

**Rust** (`src-tauri/Cargo.toml`):
- `rusqlite = { version = "0.32", features = ["bundled"] }`
- `serde = { version = "1", features = ["derive"] }`
- `serde_json = "1"`
- `uuid = { version = "1", features = ["v4"] }`
- `chrono = "0.4"`
- `tempfile = "3"` (dev-dependency, for DB tests)

**JavaScript** (`package.json`):
- `zustand = "^4.5.0"` (state management for tasks/projects/loading/error)

### Tests

Unit tests live in `src-tauri/src/db.rs` and `src-tauri/src/commands.rs`:

| Test | Location | Covers |
|---|---|---|
| `migration_creates_all_tables` | db.rs | Migration 1 creates all 5 tables |
| `migration_is_idempotent` | db.rs | Re-running migrations is a no-op |
| `project_insert_and_list` | db.rs | Project insert + list roundtrip |
| `project_duplicate_path_rejected` | db.rs | UNIQUE constraint on `root_path` |
| `task_crud_roundtrip` | db.rs | Insert, select, list, delete |
| `draft_task_update_allowed` | db.rs | `update_draft_task` succeeds for draft tasks |
| `non_draft_task_update_rejected` | db.rs | `update_draft_task` returns `InvalidOperation` for non-draft |
| `task_delete_cascades_to_turns` | db.rs | ON DELETE CASCADE removes child turns |
| `valid_file_refs_accepted` | commands.rs | Relative paths without `..` pass validation |
| `parent_traversal_rejected` | commands.rs | `../` paths are rejected |
| `absolute_path_rejected` | commands.rs | Absolute paths are rejected |
| `project_root_must_exist` | commands.rs | Non-existent path returns `InvalidOperation` |

Run with: `cargo test --manifest-path src-tauri/Cargo.toml` (requires Rust 1.85+).

---

## Phase I verification (carried forward)

- `rustfmt --edition 2021 --check src-tauri/src/lib.rs src-tauri/src/main.rs src-tauri/build.rs`: passed against the source files.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: blocked; Cargo 1.77.1 cannot parse the Rust 2024 edition; re-run with Rust 1.85+.
- `cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1`: blocked for the same Rust 2024/Cargo 1.77.1 incompatibility; re-run with Rust 1.85+.
- JSON parsing for `package.json`, `tsconfig.json`, `src-tauri/tauri.conf.json`, and `src-tauri/capabilities/default.json`: passed.
- `npm run build`: not run; Node.js/npm are unavailable in the current environment.
- `cargo check`: not completed; the available Rust is 1.77.1, below the Rust 2024 requirement, and crates.io access is unavailable.

---

## Phase IV details

### New module

```
src-tauri/src/
  runner.rs  — AgentRunner trait, CodexExecRunner, CodexEvent, RunHandle,
               EventStream, health_check, platform-specific process-tree kill
```

No new Cargo dependencies — uses `std::process::Command`, `std::thread`, and `std::sync::mpsc` from the standard library; JSON parsing uses the already-present `serde_json`.

### Public API

| Symbol | Kind | Purpose |
|---|---|---|
| `AgentRunner` | trait | Abstraction over agent backends; v1 has one impl |
| `CodexExecRunner` | struct | Spawns `codex exec --json`; configurable bin path and resume flag behavior |
| `CodexEvent` | struct | One JSONL event; has `event_type`, `raw` JSON, and helpers |
| `EventStream` | type alias | `std::sync::mpsc::Receiver<CodexEvent>` |
| `RunHandle` | struct | PID + child handle; `cancel()` kills the process tree, `wait()` reaps it |
| `HealthStatus` | struct | Result of `health_check`; serializable for Tauri |
| `health_check(bin)` | fn | Runs `codex --version` + checks env vars; no API calls |

### Exact CLI invocations

| Operation | Argv | Notes |
|---|---|---|
| Initial turn | `codex exec --json --sandbox workspace-write --full-auto "<prompt>"` | cwd = task worktree |
| Resume (`resume_with_flags=true`) | `codex exec --json --sandbox workspace-write --full-auto resume <thread_id> "<prompt>"` | Default path |
| Resume (`resume_with_flags=false`) | `codex exec resume <thread_id> "<prompt>"` | Fallback if CLI rejects flags on resume |

### JSONL event types

| Event type | Significance | Helpers |
|---|---|---|
| `thread.started` | Carries `thread_id` needed for resume | `event.thread_id()` |
| `turn.started` | Informational | — |
| `item.started/updated/completed` | Per-action progress | Forward raw to UI |
| `turn.completed` | Terminal — happy path | `event.is_terminal()` = true |
| `turn.failed` | Terminal — error path | `event.is_terminal()` = true, `event.error_message()` |
| `error` | Transient, non-terminal (e.g. `"Reconnecting..."`) | Not terminal; forward to UI |

### Log file layout

For each turn, two files are written alongside each other:

| File | Contents |
|---|---|
| `<log_path>` (e.g. `logs/{task_id}/{turn_id}.jsonl`) | Raw stdout JSONL, one line per event |
| `<stem>.stderr` (e.g. `logs/{task_id}/{turn_id}.stderr`) | Raw stderr (human-readable progress) |

The log directory is created by `spawn_codex` if it does not already exist.

### Cancellation

| Platform | Method | Reaches grandchildren? |
|---|---|---|
| Windows | `taskkill /F /T /PID <pid>` | Yes (`/T` kills the whole process tree) |
| Unix | `kill -9 -<pgid>` (negate PID = kill group) | Yes, because the child is started with `process_group(0)` making PGID = PID |

After `RunHandle::cancel()` the `EventStream` drains to empty as the I/O threads observe stdout EOF.

### New Tauri command

| Command | Signature | JS invoke name |
|---|---|---|
| `check_codex_health` | `(codex_bin: Option<String>) → HealthStatus` | `check_codex_health` |

`HealthStatus` fields (camelCase in JSON): `binaryFound`, `version`, `authEnvPresent`, `detail`.

### ⚠ Pinned CLI version and `resume` flag smoke-test

The `resume_with_flags` field defaults to `true` (pass `--json --sandbox --full-auto` on resume). **This must be verified empirically against the pinned Codex CLI version before Phase V depends on it.** Some builds reject those flags on `resume` and honour only the flags from the initial turn.

Procedure (after `npm run tauri dev` works):
1. Create a task, run it, note the thread_id.
2. Check that `codex exec --json --sandbox workspace-write --full-auto resume <thread_id> "follow up"` exits 0 and produces JSONL output.
3. If it exits non-zero with `"unknown argument '--json'"` or similar, set `resume_with_flags = false` in the runner and re-test.
4. Record the exact `codex --version` string and the resume flag behavior in this document.

### Tests

All tests live in `src-tauri/src/runner.rs`.

| Test | Covers |
|---|---|
| `parse_thread_started` | `thread_id()` extraction |
| `parse_turn_started` | Basic non-terminal event |
| `parse_turn_completed` | `is_terminal()` true, no error |
| `parse_turn_failed_string_error` | `error_message()` from string |
| `parse_turn_failed_object_error` | `error_message()` from `{"message":"..."}` |
| `parse_reconnecting_error_is_not_terminal` | Transient errors are non-terminal |
| `parse_item_event` | Arbitrary item events pass through |
| `parse_returns_none_for_non_json` | Graceful skip of non-JSON lines |
| `parse_returns_none_for_json_without_type_field` | Missing `type` = skip |
| `parse_strips_surrounding_whitespace` | Whitespace-padded lines parse OK |
| `thread_id_returns_none_for_non_thread_started` | No false positives on thread_id |
| `health_check_missing_binary` | Binary-not-found is reported cleanly |
| `runner_default_settings` | Defaults are `"codex"` + flags enabled |
| `io_threads_stream_and_log_events` (¹) | Full I/O thread pipeline: events received + log written |
| `invalid_lines_are_silently_skipped` (¹) | Non-JSONL lines are silently dropped |

(¹) Unix only — uses `sh -c 'printf ...'` to emit fake JSONL.

Run: `cargo test --manifest-path src-tauri/Cargo.toml` (Rust 1.85+).

---

## Phase III details

### New module

```
src-tauri/src/
  worktree.rs  — WorktreeManager struct; all git operations via std::process::Command
```

`WorktreeManager` is stateless (contains only the `git_bin` path string) and has no dependency on Tauri or SQLite, making it fully unit-testable in isolation.

### Exact git invocations

| Operation | Command | Notes |
|---|---|---|
| `is_git_repo` | `git -C <path> rev-parse --git-dir` | `-C` flag tolerates non-existent paths; any non-zero exit = false |
| `head_sha` | `git rev-parse HEAD` | Returns full 40-char SHA |
| `add_worktree` | `git worktree add <wt_path> -b <branch> <base_ref>` | `wt_path` must be absolute |
| `remove_worktree` (dir exists) | `git worktree remove --force <wt_path>` then `git branch -D <branch>` | `--force` handles dirty / untracked files |
| `remove_worktree` (dir gone) | `git worktree prune` then `git branch -D <branch>` | Cleans up stale index entries |
| `diff` | `git add -A`, `git diff --cached <base_ref>`, `git reset HEAD .` | Stage-diff-unstage pattern includes new untracked files; index is fully restored |
| `commit_all` | `git add -A`, (`git diff --cached --quiet` check), `git commit -m <msg>`, `git rev-parse HEAD` | Skips empty commit; returns current HEAD either way |
| `merge_branch` | `git merge --no-ff <branch>` | `--no-ff` always creates a named merge commit |

### Branch / path conventions

| Item | Convention |
|---|---|
| Branch name | `task/<task_id>` (e.g. `task/01HABCXYZ`) |
| Worktree directory | `{app_data_dir}/worktrees/{task_id}` (chosen by caller, not WorktreeManager) |
| `base_ref` | Full 40-char SHA returned by `head_sha` at task creation time |

### Integration with `create_project`

`create_project` (in `commands.rs`) now calls `WorktreeManager::default().is_git_repo()` after the existing directory-existence check. A non-git folder returns `AppError::InvalidOperation` with a message directing the user to run `git init`.

### Known constraints

- `git worktree add` fails if the branch name already exists. Phase V reports stale worktree paths clearly; automatic recovery of an orphaned branch remains deferred.
- `git merge --no-ff` may fail with a conflict. Conflict handling is deferred to Phase VII.
- The `git` binary path defaults to `"git"` (PATH lookup). Phase VIII settings will add a configurable override.
- Worktree tests require `git` on the host PATH. If unavailable, the test harness will panic early with a clear message.

### Tests

All tests are in `src-tauri/src/worktree.rs`.

| Test | Covers |
|---|---|
| `head_sha_returns_initial_commit` | `head_sha` returns the correct 40-char SHA |
| `is_git_repo_true_for_initialized_dir` | Recognises a real git repo |
| `is_git_repo_false_for_plain_directory` | Rejects a plain directory |
| `is_git_repo_false_for_nonexistent_path` | Returns false for missing paths |
| `add_and_remove_worktree` | Full lifecycle: add → inspect → remove; branch deleted |
| `dirty_worktree_is_force_removed` | `--force` removes worktree with untracked + modified files |
| `remove_worktree_when_directory_already_deleted` | Prune-path handles externally deleted directory |
| `diff_is_empty_on_fresh_worktree` | No diff on freshly created worktree |
| `diff_reflects_modified_file` | Modified file shows in diff; repeated diff is idempotent |
| `diff_reflects_new_untracked_file` | New untracked files appear in diff (stage-diff-unstage) |
| `commit_all_creates_new_commit` | Commit is created; working tree is clean; diff vs base still shows changes |
| `commit_all_is_noop_when_nothing_changed` | No empty commit; HEAD SHA is returned unchanged |
| `merge_branch_lands_changes_in_main_repo` | Merge commit created; file appears in main repo |

Run: `cargo test --manifest-path src-tauri/Cargo.toml` (requires Rust 1.85+ and `git` on PATH).

---

## Phase V details

### Execution module

src-tauri/src/execution.rs now owns the run lifecycle and is registered as managed RunState. The default maximum is 2 active tasks; a third start is rejected with a plain InvalidOperation error. A task can have only one active run.

### Commands

| Command | Signature | Behavior |
|---|---|---|
| run_task | (task_id: String, codex_bin: Option<String>) -> Task | Captures project HEAD, creates app-data/worktrees/task_id on task/task_id, inserts an initial running turn, then starts Codex in the worktree |
| cancel_task | (task_id: String) -> () | Kills the registered process tree; the worker preserves and stores the partial cumulative diff |

The frontend event channel is task:{id}:event. Each Codex event is forwarded as an object with taskId, turnId, eventType, raw, status, diff, and error. A final forge.task.updated payload carries the terminal task status and cached diff.

### State transitions

| Trigger | Turn status | Task status |
|---|---|---|
| run setup | running | running |
| turn.completed | completed | awaiting_review |
| turn.failed | failed | failed |
| process exits without a terminal event | failed | failed |
| cancel_task | cancelled | cancelled |

The worktree and branch remain after completion, failure, or cancellation so review and partial-diff inspection are possible. Cleanup and confirmation are deferred to Phase VII.

### Persistence

Migration 2 adds task.diff for the cached cumulative unified diff and upgrades databases created by Phase II. Turn rows store the raw JSONL path; the runner also writes the adjacent stderr transcript. The thread.started event updates task.agent_thread_id.

### Frontend

The typed API exposes runTask and cancelTask, the Zustand store retains the last 200 task events, and App listens to the selected task event channel. The execution dock enables Run/Cancel and displays live event names; the existing raw diff area displays the cached cumulative diff.

### Tests and recovery

The Rust suite passes with 41 tests, including default concurrency and duplicate-run reservation checks. Setup failures release the concurrency slot and remove a newly-created worktree. Codex spawn failures mark the turn and task failed and remove the worktree. A stale worktree path is reported clearly and is not overwritten.

## Decisions and carried-forward risks

- Tauri v2 with Rust backend and React/TypeScript frontend remains the selected architecture.
- External command capability is limited to `codex` and `git`; arguments are allowed because later phases construct their CLI invocations.
- The shell uses the system binaries through PATH, not bundled sidecars, as specified by the proposal.
- Tauri's current plugin guidance requires Rust 1.77.2; the proposal's Rust 2024 edition requires Rust 1.85+, so the project declares `rust-version = "1.85"`.
- Validate the generated Tauri capability schema after the first dependency install.
- Before Phase IV, pin and empirically test Codex CLI `exec`/`resume` flags and event shapes.
- Before release, test Windows process-tree cancellation and preserve the proposal's warning about uncommitted changes in the main project folder.
- Git repo validation (checking that the project root is actually a git repository) is **not** enforced in Phase II — the folder picker accepts any directory. This will be enforced in Phase III when `WorktreeManager` is introduced.
