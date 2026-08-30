# Coding Agent Task Automation — Architecture & Implementation Plan (v1)

## 0. Feature recap → where it's addressed

| Requested feature | Section |
|---|---|
| Add/delete tasks, reference files/folders, run, review, confirm or comment+request changes | §2.4, Batches 1, 4, 5, 6 |
| Codex only, via terminal process or MCP | §5 (decision: terminal process) |
| Rust + Tauri | §3 |
| Windows primary, Linux/macOS secondary | §6 Batch 8, §8 |

---

## 1. Architecture

### 1.1 System shape
Tauri v2 desktop app.
- **Backend (Rust)**: owns the Codex CLI process lifecycle, git worktree management, SQLite persistence, and the task/turn state machine.
- **Frontend (React + TS, WebView)**: task list, thread/turn view, GitHub-style diff review UI, settings.
- **IPC**: Tauri `invoke` commands for request/response actions (create task, run, add comment, confirm). Tauri events for streaming (live agent output, status transitions), namespaced per task: `task:{id}:event`.

### 1.2 Core domain model
- **Project** — a folder the user has added; must be a git repository.
- **Task** — a unit of work: title, prompt, referenced file/folder paths, status, and a link to one **Thread**.
- **Thread** — the ongoing Codex conversation for a Task. 1:1 with Task in v1: the initial run and every later "request changes" round are turns in the *same* conversation, not separate threads. This maps directly onto Codex's own session/thread concept.
- **Turn** — one Codex invocation inside a Thread (initial or follow-up). Has its own raw transcript log and status.
- **Review** — not a separate entity: it's the cumulative git diff of the task's worktree against its base commit, plus comments anchored to file/line, plus a verdict (approved / changes requested).

### 1.3 Key architectural decisions

1. **One git worktree per task, not a shared working directory.** On first run, the backend creates a linked worktree (`git worktree add <path> -b task/<id> <base_ref>`) off the project's current HEAD. This gives free isolation between concurrently running tasks, a stable base point to diff against, and trivial cleanup. **v1 requirement: the project folder must already be a git repo** — surface a clear error otherwise.

2. **Diffing/review is git-based, not derived from Codex's event stream.** After every turn, the backend runs `git diff <base_ref>` inside the worktree. This is always the *cumulative* diff since the task started (no intermediate commits needed) — it's what the review UI renders and what comments attach to. This keeps review logic reusable if another agent backend is added later.

3. **Agent backend sits behind a small trait** (`AgentRunner`) so Codex-specific process/protocol details don't leak into the rest of the app:
   ```rust
   trait AgentRunner {
       fn start_turn(&self, cwd: &Path, prompt: &str) -> EventStream;
       fn resume_turn(&self, cwd: &Path, thread_id: &str, prompt: &str) -> EventStream;
       fn cancel(&self, handle: RunHandle);
   }
   ```
   v1 ships exactly one implementation, `CodexExecRunner`. This exists so Codex's CLI quirks stay in one module.

4. **Codex is driven as a spawned terminal process (`codex exec --json`), not `codex mcp-server`.** Tasks run unattended/asynchronously — there's no live human in the loop mid-turn, so a one-shot subprocess per turn is simpler to spawn, log, and cancel than a persistent JSON-RPC session. `codex mcp-server` is a real, documented alternative (see §5) worth revisiting only if a future version wants live, per-tool-call approval UX instead of blanket autonomous sandboxed runs.

5. **"Request changes" is just the next turn.** Comments are packaged into one structured follow-up message and sent via `codex exec resume <thread_id> "<message>"` on the same thread — no separate comment-resolution API on the agent side.

6. **Persistence**: SQLite (`rusqlite`) holds structured rows (tasks, turns, comments, settings). Raw JSONL transcripts and stdout/stderr are written to files under the app data dir and referenced by path — keeps the DB small and lets the UI tail a live log directly.

### 1.4 Data flow for one task run
1. User creates a Task: picks a project folder, optionally references specific files/folders inside it, writes a prompt.
2. On **Run**: backend creates the worktree, builds the initial prompt (task prompt + an explicit list of the referenced paths), spawns `codex exec --json ...` (exact invocation in §5) with the worktree as its working directory.
3. Backend parses the JSONL stream line-by-line, forwards each event to the frontend over `task:{id}:event`, and appends raw lines to the turn's log file. It captures the Codex `thread_id` from the first `thread.started` event for later resume.
4. On `turn.completed`/`turn.failed`: backend runs `git diff <base_ref>` in the worktree, stores the diff text, sets Task status to `awaiting_review`.
5. User reviews the diff, leaves inline comments, and either:
   - **Confirms** → backend commits the worktree (`git add -A && git commit`), optionally merges the task branch into the target branch (setting-controlled, default: leave as an unmerged branch), marks Task `approved`, removes the worktree.
   - **Requests changes** → backend composes a follow-up prompt from the unresolved comments, calls `resume_turn`, repeats step 3 as a new `turn` row, diff is recomputed (still cumulative against the same `base_ref`).
6. **Delete** (any time): cancel any in-flight process, remove the worktree and branch, delete the Task's DB rows and log files.

---

## 2. Tech stack

- Rust (2024 edition) + Tauri v2.
- Plugins: `tauri-plugin-shell` (spawn `codex` and `git` as two scoped external commands — treated as system binaries located via PATH/settings, **not** bundled sidecars, since both are independently installed/updated/authenticated tools), `tauri-plugin-dialog` (native folder picker), `tauri-plugin-fs` (scoped file preview), `tauri-plugin-window-state`.
- `rusqlite` (bundled feature) for storage.
- Git operations: shell out to the system `git` CLI via the scoped shell plugin (simpler and more future-proof for worktree semantics than a `git2`/libgit2 binding).
- `tokio` for async process management; `serde`/`serde_json` for parsing the Codex JSONL protocol.
- Frontend: React + TypeScript + Vite.
  - **State**: model tasks/threads/review state in a Zustand store with imperative, promise-based action functions (`runTask()`, `confirmTask()`, `requestChanges()`), rather than deriving the run→review→confirm flow from chains of local `useState` + conditional renders. This flow has several cross-cutting states (running, awaiting review, changes requested) and stacked modals (diff review, confirm dialog) that are much easier to reason about as store actions than prop-drilled component state.
  - Diff rendering: a unified-diff parser + a diff-view component (e.g. `react-diff-view`) for the GitHub-style hunk display with inline comment anchors.

---

## 3. Data model (SQLite)

```sql
CREATE TABLE project (
  id TEXT PRIMARY KEY,
  root_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE task (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES project(id),
  title TEXT NOT NULL,
  prompt TEXT NOT NULL,
  file_refs TEXT NOT NULL,        -- JSON array of paths relative to project root
  status TEXT NOT NULL,           -- draft|running|awaiting_review|changes_requested|approved|failed|cancelled
  base_ref TEXT,                  -- git commit sha the worktree branched from
  worktree_path TEXT,
  branch_name TEXT,
  agent_thread_id TEXT,           -- Codex thread id, set after first turn starts
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE turn (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES task(id),
  kind TEXT NOT NULL,             -- initial|followup
  prompt TEXT NOT NULL,
  status TEXT NOT NULL,           -- running|completed|failed|cancelled
  log_path TEXT NOT NULL,         -- raw JSONL transcript file
  started_at TEXT NOT NULL,
  ended_at TEXT
);

CREATE TABLE review_comment (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES task(id),
  turn_id TEXT NOT NULL REFERENCES turn(id),   -- turn that was current when the comment was made
  file_path TEXT NOT NULL,
  line_number INTEGER,            -- NULL = file-level comment
  side TEXT,                      -- old|new
  body TEXT NOT NULL,
  resolved INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

---

## 4. Codex integration details

- **Initial turn**:
  `codex exec --json --sandbox workspace-write "<prompt>"`, with the process's working directory set to the task's worktree (via the shell plugin's process-spawn cwd option; fall back to Codex's own `--cd <path>` flag if the spawn API doesn't expose cwd directly).
- **`--json`** turns stdout into a JSONL event stream: `thread.started` (carries `thread_id`), `turn.started`, `item.started|updated|completed` (item types: `agent_message`, `reasoning`, `command_execution`, `file_change`, `mcp_tool_call`, `web_search`, plan/`todo_list` updates), `turn.completed` (includes token usage) or `turn.failed`, and `error` (transient `"Reconnecting..."` errors are non-fatal, treat as progress). Human-readable progress also streams on **stderr** — persist it for debugging, but drive all UI state off the parsed **stdout** JSONL only.
- **Follow-up turns**: `codex exec resume <thread_id> "<followup prompt>"` (or `resume --last`).
  ⚠️ **Verify against the pinned Codex CLI version before relying on this**: some CLI builds reject `--json`/`--sandbox`/`--model` on `resume`, honoring only the flags set on the first turn of the thread. If `--json` is rejected on resume, fall back to treating resume's plain stdout as the final agent message (no structured events) until the CLI version supports otherwise. Pin an exact Codex CLI version rather than assuming flag support.
- **Sandbox/approval**: `--sandbox workspace-write` keeps writes confined to the worktree. Never use `danger-full-access` in v1. The obsolete `--full-auto` flag is not passed because current Codex CLI builds reject it.
- **Auth**: Codex must already be authenticated on the machine (`codex login`, or a `CODEX_API_KEY`/`OPENAI_API_KEY` env var). The app doesn't manage credentials — Settings should show whether `codex` is resolvable on PATH and authenticated, via a one-off health check (e.g. `codex exec --json "ok"` against a scratch temp dir) and link out to Codex's own login docs if it fails.
- **Cancellation**: kill the process tree mid-turn (see §7), mark the turn `cancelled`; leave whatever partial diff exists in the worktree so the user can still inspect or discard it.
- **Noted alternative for later**: `codex mcp-server` runs Codex as a standard MCP stdio server (JSON-RPC 2.0), exposing `codex`/`codex-reply` tools with native thread continuation and structured approval-request notifications. Worth adopting in a v2 if per-action approval UX becomes a requirement. Not used in v1, to keep the process model simple.

---

## 5. Implementation batches

Each batch is a self-contained unit an LLM can implement against mocked interfaces before the real ones exist.

**Batch 0 — Scaffold**
Tauri + React/TS/Vite app; wire `tauri-plugin-shell`, `tauri-plugin-dialog`, `tauri-plugin-fs`, `tauri-plugin-window-state`. Capabilities: scope `shell` to exactly two commands, `codex` and `git`. Basic layout: sidebar task list + empty main panel. *Output*: app launches, no backend logic.

**Batch 1 — Persistence + Task CRUD**
SQLite setup + migrations per §3. Commands: `create_project`, `list_projects`, `create_task`, `list_tasks`, `get_task`, `delete_task`, `update_task_prompt` (draft-only). Frontend: task list, "new task" form (folder picker, file/folder reference picker scoped to that folder, prompt textarea). *Output*: create/list/delete tasks in `draft` state; nothing runs.

**Batch 2 — Git worktree module**
Rust module wrapping the system `git` CLI: verify a path is a repo + get HEAD; `git worktree add`/`remove`; `git diff <base_ref>` → unified diff string; commit + merge helpers for the Confirm step. Unit-testable against a throwaway repo fixture, no dependency on Codex or UI. *Output*: standalone `WorktreeManager`.

**Batch 3 — Codex agent runner**
Define the `AgentRunner` trait (§1.3.3). Implement `CodexExecRunner`: spawns `codex` via the scoped shell plugin, builds argv per §4, parses JSONL into a typed `CodexEvent` enum, forwards events over a channel, persists raw lines to the turn's log, captures `thread_id`. Includes the Codex health-check used by Settings. *Output*: runner exercisable against a real Codex install and a scratch folder, independent of Tasks/DB/UI.

**Batch 4 — Task execution orchestration**
Wires Batches 1–3: Run → worktree created → runner starts turn → events streamed to `task:{id}:event` and appended to the `turn` row/log → on completion, diff computed, Task status updated. Bounded concurrency via a semaphore (configurable "max concurrent tasks", default 2). Cancel command wired to `AgentRunner::cancel` + process-tree kill. *Output*: create → run → live output → `awaiting_review`, works end to end (diff shown as raw text is fine here).

**Batch 5 — Diff review UI**
Render the stored diff GitHub-style: file list, expandable hunks, inline comment affordance per line. Commands: `add_review_comment`, `resolve_review_comment`, `list_review_comments`. Confirm / Request-changes actions on the task detail view. Model the run→review→confirm flow as Zustand store actions, not nested component state, since this is exactly the kind of multi-step, modal-heavy orchestration that gets tangled as ad hoc `useState`. *Output*: full review UX, developable against Batch 4's output or fixture data.

**Batch 6 — Review → follow-up loop**
"Request changes": gather unresolved comments (+ optional free-text note) → format into one follow-up prompt → `resume_turn` with the task's `agent_thread_id` → repeats Batch 4's pipeline as a new `turn`. "Confirm": commit the worktree, optionally merge per a settings toggle (default: leave as an unmerged branch), mark `approved`, remove the worktree. *Output*: full loop — run → review → request changes → re-run → re-review → confirm.

**Batch 7 — Settings**
Codex/git binary path overrides + detected versions + health/auth status. Max concurrent tasks. Merge-on-confirm toggle. Sandbox mode shown as fixed/read-only info (`workspace-write`) rather than a user-editable choice, to avoid accidentally enabling `danger-full-access`.

**Batch 8 — Packaging (Windows first)**
`tauri.conf.json` bundle config for Windows (MSI/NSIS); note where a code-signing cert would plug in, don't implement signing itself. Smoke-test the same build on Linux/macOS as a checklist (PATH discovery for `git`/`codex`, process-kill differences), not new feature work.

---

## 6. Out of scope for v1
- Any agent backend besides Codex.
- `codex mcp-server` / live per-action approval UX.
- Cloud sync, multi-user, remote/Codex-Cloud execution.
- Automatic merge-conflict resolution (surface conflicts as a plain error state).
- Non-git projects.

## 7. Risks to verify empirically while building
- Whether `resume` accepts `--json`/`--sandbox` on the specific pinned Codex CLI version (§4).
- Windows process-tree termination for a spawned `codex` and any of its own subprocesses — a plain child-kill may not reach grandchildren; test explicitly (`taskkill /T /F /PID` vs Unix process-group kill).
- Behavior when the project has uncommitted changes at task-creation time: v1 branches the worktree off HEAD regardless and does not carry over uncommitted changes from the main working copy — call this out to the user in the "new task" UI.
