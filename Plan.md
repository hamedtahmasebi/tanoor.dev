# Tanoor — Implementation Plan: LLM Task Naming, Open-in-Editor, Review Screen

> Target: implementable phase-by-phase by a low-cost LLM. Each phase compiles and verifies independently.
> Verify after every phase: `cargo test` (in `src-tauri/`), `npm run build`, `npm test`; manual smoke via `npm run tauri dev`.
> Conventions (from AGENTS.md): components never call `invoke` directly — all backend calls go `CommandContract` (src/api.ts) → `api` helper → store action; new commands registered in `src-tauri/src/lib.rs` `invoke_handler`; serde camelCase; CSS kebab-case, reuse existing tokens/classes; every list/dropdown needs arrow-key + Enter/Escape navigation; no modal for review.

## Current state (verified in code)

- Branch is hardcoded `task/{task_id}` in `src-tauri/src/execution.rs:701` (`prepare_run`), persisted via `db::prepare_task_run` (`src-tauri/src/db.rs:382`).
- Task creation takes a user-typed title (`create_task`, `src-tauri/src/commands.rs:156`; title input `src/components/NewTaskDialog.tsx:322`). Title updates only allowed while `draft` (`db::update_draft_task`, db.rs:790).
- Worktree lives at `{app_data_dir}/worktrees/{task_id}`; exists during `running`/`awaiting_review`/`changes_requested`, removed on confirm (execution.rs:632-637). No external-open path, no recovery if the directory is deleted externally.
- Review is a modal overlay (`DiffReviewDialog`, `src/components/DiffReview.tsx:273`) driven by `reviewModal` in `src/store.ts:21`. `request_changes` (commands.rs:1064 → `execution::start_follow_up`, execution.rs:394) starts follow-ups **immediately**; there is no "submitted, waiting to start" state.
- Task statuses: `draft | running | awaiting_review | changes_requested | approved | failed | cancelled` (`src/types.ts:12`, `src-tauri/src/models.rs:24`). Turn statuses: `running | completed | failed | cancelled`.
- Agent runners: codex/claude/opencode CLI adapters in `src-tauri/src/runner.rs`; arg builders at runner.rs:338 (`CodexExecRunner::initial_args`), 407, 499.

---

## Phase A1 — LLM naming at task creation (backend)

**New file `src-tauri/src/naming.rs`** (pure, unit-testable, no Tauri deps):

1. `pub struct TaskNaming { pub title: String, pub branch: String }` (`#[serde(rename_all = "camelCase")]` for event payloads).
2. `pub fn sanitize_slug(input: &str) -> String` — lowercase; runs of chars outside `[a-z0-9]` → `-`; collapse repeated `-`; trim leading/trailing `-`; cap at 40 chars (char-boundary safe); empty → `"task"`.
3. `pub fn fallback_title(prompt: &str) -> String` — first line of prompt, trimmed, capped at 60 chars; empty → `"New task"`.
4. `pub fn fallback_branch_name(prompt: &str, task_id: &str) -> String` — `format!("task/{}-{}", sanitize_slug(&fallback_title(prompt)), &task_id[..6])`. The 6-char UUID suffix guarantees uniqueness, so no branch-existence check is needed.
5. `pub fn naming_prompt(prompt: &str) -> String` — one-shot prompt:
   *"You are naming a coding task. Reply with ONLY a JSON object, no markdown, in the form {"title":"<human-readable task title, max 60 chars>","branch":"<kebab-case slug, 2-6 words, lowercase letters/digits/hyphens>"}. Task:\n{first 800 chars of prompt}"*
6. Extraction helpers (unit-tested, no process spawn):
   - `pub fn extract_json_object(text: &str) -> Option<serde_json::Value>` — reuse the brace-scanning logic in `commands::extract_json_objects` (commands.rs:414); make that function `pub(crate)` and call it from here, taking the first object.
   - `pub fn extract_text_from_codex_jsonl(stdout: &str) -> Option<String>` — parse each line as JSON, find the **last** `item.completed` whose `item.type == "agent_message"`, read `item.content` as string (same shape as runner.rs:964 fixture).
7. `pub fn generate_task_naming(agent_id: &str, binary: &str, model: &str, effort: Option<&str>, prompt: &str, cwd: &Path) -> TaskNaming` — **never fails, never blocks more than 20 s**:
   - Args per agent (mirror runner.rs builders):
     - codex: `["exec", "--json", "--sandbox", "read-only", "--model", model]`, plus `["--effort", e]` when effort is Some and model starts with `o` (same rule as runner.rs:341-345), then the prompt.
     - claude: `["-p", prompt, "--model", model]`
     - opencode: `["run", prompt, "--model", model]`
   - Spawn with piped stdout/stderr and `configure_child` (make it `pub(crate)` in runner.rs:844 and reuse). A reader thread collects stdout and sends it over a channel; main thread `recv_timeout(Duration::from_secs(20))`.
   - On timeout → kill child, use fallback. On success → codex: `extract_text_from_codex_jsonl` → `extract_json_object`; claude/opencode: `extract_json_object(stdout)` directly (fall back to `extract_text_from_jsonl`-style plain-text first line if no JSON found).
   - Parse `title`/`branch` fields; sanitize branch via `sanitize_slug`; any missing/empty piece → fallback value for that piece. `title` also trimmed to 60 chars.
   - Returns title **without** the `task/` prefix; caller composes the full branch name.

**Wire into `create_task` (commands.rs:156):**

8. Change signature: `title: Option<String>` (Tauri arg stays `title`, now nullable). Determine fallback title/branch, insert the task with them, then launch naming in the background:
   - Add `app: tauri::AppHandle` parameter (and register unchanged).
   - `let fallback = fallback_title(&prompt);` — use `title.unwrap_or_default().trim()`, and if empty use `fallback`. Insert the task (status `draft`) with `branch_name = Some(fallback_branch_name(&prompt, &task.id))` so the branch is decided at creation time, as confirmed.
   - After `db::insert_task` succeeds, spawn `std::thread::spawn` that:
     1. Loads settings (`default_agent` + that agent's binary/model/effort — reuse the mapping in `execution::resolve_agent`, execution.rs:220; extract it into a shared helper or replicate).
     2. Calls `naming::generate_task_naming(...)` with `cwd = project.root_path` (project is already validated/loaded in the command).
     3. Persists via a **new guarded db function** (below) — this is the only place the background result lands.
     4. Emits a `forge:task-event` (channel constant `execution::TASK_EVENT_CHANNEL`) with `event_type = "forge.task.named"`, `turn_id = ""`, `raw = {"title": ..., "branchName": ...}`, `status/diff/error = None`. Skip the emit if the guarded update changed 0 rows.
9. **New db function (db.rs):**
   ```rust
   pub fn apply_generated_naming(conn, task_id, expected_current_title, new_title, branch_name, now) -> Result<usize, AppError>
   ```
   - `UPDATE task SET title = ?1, branch_name = ?2, updated_at = ?3 WHERE id = ?4 AND title = ?5 AND status = 'draft'`.
   - The `title = expected` guard prevents clobbering a user rename; **`status = 'draft'` is critical** — without it the thread could overwrite `branch_name` after the task already started running, which would break `confirm_task` (it merges `task.branch_name`, execution.rs:631). Return the changed-row count so the thread knows whether to emit.
10. **`prepare_run` (execution.rs:648-766):** replace line 701 `let branch_name = format!("task/{task_id}");` with: use `task.branch_name` if `Some`, else `naming::fallback_branch_name(&task.prompt, task_id)`. Everything downstream (worktree add, `db::prepare_task_run`, follow-ups) is unchanged. Update the convention doc comment in worktree.rs:18.
11. `lib.rs`: no new commands in this phase (`create_task` signature change only).

**Tests:** naming.rs — sanitize edge cases (empty, unicode, long, leading/trailing/duplicate hyphens); `extract_json_object` finds object amid prose and ignores invalid JSON; codex-JSONL extraction picks last agent_message and skips non-JSON lines; fallback title/branch formats. db.rs — `apply_generated_naming` succeeds while draft+title-unchanged, and refuses when title was renamed or status moved past draft.

**Verify:** `cargo test`.

---

## Phase A2 — Frontend: no title field, LLM title in sidebar, inline rename

1. **types.ts:** `CreateTaskInput.title` → optional (`title?: string`). Task shape unchanged (title/branchName already exist).
2. **api.ts:** `create_task` contract `title: string | null`; `api.createTask` passes `input.title ?? null`. Add contract entry `rename_task: { args: { taskId: string; title: string }; result: Task }` + `api.renameTask`.
3. **commands.rs:** new `#[tauri::command] pub fn rename_task(state, task_id: String, title: String) -> Result<Task, AppError>` — trim, reject empty / > 200 chars; new db fn `rename_task(conn, id, title, now)` = plain `UPDATE task SET title=?… WHERE id=?` (0 rows → `NotFound`), **allowed in any status** (title is display-only; branch is untouched). Register in `lib.rs` `invoke_handler`.
4. **store.ts:**
   - `renameTask(taskId, title)` action → updates `tasks` entry from the returned Task.
   - `createTask`: keep as-is (returns the task with fallback title).
   - `appendTaskEvent` (store.ts:221): when `event.eventType === "forge.task.named"`, update the matching task's `title` and `branchName` from `event.raw` **and do not append the event** to `taskEvents` (so it never shows as an agent-output line); otherwise current behavior.
5. **NewTaskDialog.tsx:** remove the title state, the `composer-title` input (line 322), and title validation/guards (lines 283, 292 — `canSubmit` now only requires non-empty prompt). `onCreate({ prompt, fileRefs, agentId, agentModel, agentEffort })` — no title.
6. **TaskCard.tsx (sidebar task list):** inline rename, desktop-style:
   - Title span gets a small "✎" button (focusable, `title="Rename task"`, visible on row focus/hover) and the span is also double-click editable.
   - Editing renders an `<input>` seeded with `task.title` (reuse `.project-dropdown`-adjacent styling; add `.task-rename-input` class in styles.css using existing input tokens). **Enter** saves (`renameTask`), **Escape** cancels, blur cancels (safer than saving). No layout jank; transitions ≤ 150 ms.
7. **styles.css:** minimal additions for the rename input (border-radius 6 px, background `#2b2d2a` family, 11 px body size).

**Verify:** `npm run build`; manual: create a task with only a prompt → sidebar shows fallback title immediately, LLM title arrives within seconds; rename inline in the sidebar (also while `ready`/`awaiting_review` after later phases); rerun does not clobber a renamed title; create → immediately Run does not produce a mismatched branch (confirm branch on the worktree with `git -C <worktree> branch --show-current`).

---

## Phase B — Open worktree in external editor

**1. Worktree recovery — `worktree.rs`:** add
```rust
pub fn ensure_worktree(&self, repo_root: &Path, worktree_path: &Path, branch_name: &str) -> Result<(), AppError>
```
- `worktree_path` exists → Ok. Else: `git worktree prune` (ignore errors), then `git worktree add <worktree_path> <branch_name>` (checks out the existing branch). This is what makes a pending-review task reliably openable even if the directory was removed.
- Test (mirror worktree.rs:535 pattern): create worktree, commit, delete dir externally, `ensure_worktree` recreates it containing the committed file.

**2. New file `src-tauri/src/editors.rs`** — static registry + PATH-only detection (never launches processes to detect):

| id | label | commands | Windows well-known paths |
|---|---|---|---|
| vscode | VS Code | `code` | `%LOCALAPPDATA%\Programs\Microsoft VS Code\bin\code.cmd`, `C:\Program Files\Microsoft VS Code\bin\code.cmd` |
| vscode-insiders | VS Code Insiders | `code-insiders` | equivalent Insiders paths |
| cursor | Cursor | `cursor` | `%LOCALAPPDATA%\Programs\cursor\resources\app\bin\cursor.cmd` |
| windsurf | Windsurf | `windsurf` | `%LOCALAPPDATA%\Programs\Windsurf\resources\app\bin\windsurf.cmd` |
| zed | Zed | `zed` | `%LOCALAPPDATA%\Programs\Zed\Zed.exe` |
| jetbrains | JetBrains IDE | `idea`, `idea64` | `%LOCALAPPDATA%\JetBrains\Toolbox\scripts\idea.cmd` |
| nvim | Neovim | `nvim` | — |
| vim | Vim | `vim` | — |
| subl | Sublime Text | `subl` | — |

- `#[derive(Serialize)] pub struct EditorInfo { id, label, command }` with `rename_all = "camelCase"`.
- `fn find_in_path(cmd: &str) -> Option<String>` — Windows: `where.exe <cmd>` first line (`CREATE_NO_WINDOW`); Unix: `which <cmd>`. Then check the well-known absolute paths with `Path::exists`.
- `pub fn detect_editors() -> Vec<EditorInfo>` — registry entries whose command resolved.
- `pub fn launch_editor(editor_id: &str, worktree_path: &Path) -> Result<(), AppError>`:
  - Re-resolve from the registry; missing → clear error.
  - Terminal editors (`nvim`, `vim`): Windows → spawn `cmd.exe /K <command> "<path>"` (keeps the console open); Unix → spawn `x-terminal-emulator -e <command> <path>`, error if that binary is missing. GUI editors → spawn `<command> <worktree_path>` directly.
  - `.spawn()` then drop the child (detached).

**3. Commands (commands.rs):**
- `#[tauri::command] pub fn list_editors() -> Result<Vec<EditorInfo>, AppError>`.
- `#[tauri::command] pub fn open_worktree_in_editor(state: State<DbState>, app: AppHandle, task_id: String, editor_id: String) -> Result<Task, AppError>`:
  - Load task + project; require `worktree_path` and `branch_name` (draft tasks → actionable error: "This task has no worktree yet — run it first").
  - `wm.ensure_worktree(repo_root, worktree_path, branch_name)?`, then `editors::launch_editor(...)`; return the re-selected task.
- Register both in `lib.rs`.

**4. Frontend (data-flow rules):**
- types.ts: `export interface EditorInfo { id: string; label: string; command: string; }`
- api.ts: contracts `list_editors` / `open_worktree_in_editor` + `api.listEditors()`, `api.openWorktreeInEditor(taskId, editorId)`.
- store.ts: `editors: EditorInfo[]` loaded in `initApp` (fire-and-forget, non-fatal); action `openInEditor(taskId, editorId)`.
- **TaskFollowUpPanel.tsx** (actions row, line 322): "Open in editor" `quiet-button`, enabled when `task.worktreePath != null`. One editor → direct launch; several → toggles a dropdown reusing the `.project-dropdown` pattern (see App.tsx:166) with ArrowUp/Down + Enter navigation and Escape-to-close; zero → disabled with title "No supported editor found".
- The same button is added to the review screen header in Phase C.

**Verify:** `cargo test`, `npm run build`; manual: task in pending review → button opens VS Code/Zed at the worktree; delete the worktree dir externally → button recreates it and still opens.

---

## Phase C — Review as a regular screen (UI restructure, no behavior change yet)

1. **store.ts:** replace `reviewModal: ReviewModalState | null` with `reviewTaskId: string | null` (keep `ReviewStep` for the inline verdict step).
   - `openReview(taskId)` → sets `reviewTaskId`, loads comments + catalogs (same body as store.ts:261). `closeReview()` → null.
   - `requestChanges`/`confirmTask` success handlers clear `reviewTaskId` (instead of `reviewModal`, store.ts:351/366). Rename all `reviewModal` reads (DiffReview.tsx:215, 275).
2. **New file `src/components/ReviewScreen.tsx`** — move the internals of `DiffReview.tsx` here (delete `DiffReview.tsx`, update import at App.tsx:7):
   - Keep `ReviewContent`, `CommentThread`, `DiffLineRow`, `CommentComposer`, `VerdictStep` unchanged.
   - Replace the `DiffReviewDialog` wrapper (DiffReview.tsx:273-287) with `export function ReviewScreen({ task }: { task: Task })` rendering `<section className="review-screen">` with the existing `review-header` / error banner / `review-body` structure — **no** `.review-overlay`, no `role="dialog"`, no `aria-modal`.
   - Header: "← Back to task" button (`closeReview()`), the Phase B "Open in editor" button, and a monospace chip showing `task.branchName`.
3. **App.tsx:**
   - Remove the modal render (line 258). In `main.editor-panel` (lines 212-220): when `reviewTaskId === selectedTask?.id`, render `<ReviewScreen task={selectedTask}/>` instead of `<TaskFollowUpPanel/>`.
   - Global Escape handler (lines 98-103) keeps calling `closeReview()` — it now returns to the task pane.
4. **styles.css:** add `.review-screen` (full-height column inside the editor panel; `overflow-y: auto` on the body area) reusing existing `review-*` classes and the `#272825` background family; remove `.review-overlay`/`.review-dialog` chrome only if unused elsewhere.

**Verify:** `npm run build`; manual: open review from the "Changes" activity button and from the task panel; line comments, per-comment agent/model assignment, request-changes and confirm all still work; Escape/back navigation works.

---

## Phase D — Submit review → `ready` → manual start

**Backend:**

1. **New task status `"ready"`**, new turn status `"pending"` (both TEXT columns — no status migration). Update the status comment (models.rs:24) and the `TaskStatus` union (types.ts:12, add `| "ready"`).
2. **db.rs — new `submit_review`** (transaction pattern from db.rs:648):
   - Guarded `UPDATE task SET status='ready' WHERE id=? AND status IN ('awaiting_review','changes_requested')` (0 rows → `InvalidOperation "not open for review"`).
   - Insert turn row `kind='follow_up'`, `status='pending'`, with `log_path = {app_data_dir}/logs/{task_id}/{turn_id}.jsonl` computed now so the later start reuses it.
3. **db.rs — modify `begin_follow_up`** (db.rs:484): accept `status IN ('awaiting_review','changes_requested','ready')`. Add:
   ```rust
   pub fn activate_pending_turn(conn, task_id, turn_id, now) -> Result<(), AppError>
   ```
   - One transaction: `UPDATE turn SET status='running' WHERE id=? AND task_id=? AND status='pending'` + guarded `UPDATE task SET status='running' WHERE id=? AND status='ready'`; 0 rows → error.
4. **db.rs — helper** `pub fn select_pending_turn(conn, task_id) -> Result<Option<TaskTurn>, AppError>` (latest turn with `status='pending'`).
5. **execution.rs:**
   - New `pub fn submit_review(app, task_id, reviewer_note, selection) -> Result<Task, AppError>`: load task (status guard), unresolved comments, build the follow-up prompt via the existing `build_follow_up_prompt` (execution.rs:910 — same "comment or reviewer note required" error), resolve agent via `resolve_agent` (defaults from `selection`), generate `turn_id`, compute log path, call `db::submit_review`. Comments stay unresolved until the run starts (grouping reads them at start time, execution.rs:271).
   - **Routing in `start_task` (execution.rs:308):** if the loaded task's status is `ready`, delegate to a new `start_ready_task(app, runs, task_id, selection)`:
     - `load_follow_up_groups` unchanged.
     - First group: `select_pending_turn` → `activate_pending_turn`; reuse that turn's `id`/`prompt`/`log_path` in the `PreparedRun` instead of inserting a new turn; resume the thread if `select_latest_agent_turn` (db.rs:578) matches the agent, else `start_turn` with `build_handoff_prompt`; resolve that group's comments; spawn `consume_turn`. Refactor `start_follow_up_group` (execution.rs:455) to accept an optional pre-existing turn so this is one code path.
     - Remaining groups: unchanged sequential loop (execution.rs:418-451).
   - `prepare_run`'s guard (execution.rs:664) stays `draft`-only.
   - **Remove the immediate-start path:** delete `start_follow_up`'s public entry, the `request_changes` command (commands.rs:1064), its `lib.rs` registration (line 67), api.ts entry (line 123), and the store action (store.ts:345).
6. **commands.rs:** `#[tauri::command] pub fn submit_review(...)` with the old `request_changes` signature (`task_id, reviewer_note, agent_id, model, effort`) → `execution::submit_review`; register in `lib.rs`.

**Frontend:**

7. **api.ts / types.ts:** `submit_review` contract + `api.submitReview(taskId, reviewerNote?, selection?)`; remove `requestChanges`.
8. **store.ts:** `submitReview` (same shape as the old `requestChanges`; on success update the task and clear `reviewTaskId`). `runTask` is unchanged — it now also starts `ready` tasks via the backend routing.
9. **ReviewScreen.tsx `VerdictStep`:** rename the flow to "Submit review" — copy: *"Feedback is queued as a follow-up message in this task's conversation. The task stays ready-to-run until you start it."* Button calls `submitReview`; the default-agent/model selectors for unassigned comments remain.
10. **TaskFollowUpPanel.tsx:**
    - `STATUS_LABELS`: `ready: "Ready to run"`; `stepIndex`: `ready` → 2 (same step as pending review, distinct label via `stepLabel`).
    - `canRun` (line 303): `draft || ready`; button label "Run task" / "Start follow-up".
    - `OutputBlock`: treat `status === "pending"` turns as a neutral waiting block (extend the `statusClass` mapping at line 196).

**Tests:** db.rs — `submit_review` sets `ready` + creates a pending turn, rejects non-review statuses; `activate_pending_turn` flips pending→running only from `ready`; `begin_follow_up` accepts `ready`. execution.rs — existing prompt/selection tests unchanged.

**Verify:** `cargo test`, `npm run build`; manual E2E: run → review → comment + assign one comment to a different agent → Submit review → task "Ready to run" with the follow-up message visible in the conversation → Start follow-up → thread resumes, comments resolve, diff updates.

---

## Phase E — Line-range selection in review (GitHub-style)

1. **Migration 7 (db.rs `run_migrations`):** `ALTER TABLE review_comment ADD COLUMN line_end_number INTEGER;` (guarded/idempotent like migration 2, db.rs:115). Update `insert_review_comment`, both comment SELECTs (db.rs:616, 631), `resolve_review_comment`/`assign_review_comment` re-selects, and `row_to_review_comment` (db.rs:720).
2. **models.rs / types.ts:** `lineEndNumber: number | null` on `ReviewComment` and `AddReviewCommentInput`.
3. **commands.rs `validate_comment_anchor` (line 927):** accept optional `line_end`; rule: `None` or `>= line_number` on the same side. New tests for the range cases (reuse the existing test at commands.rs:1169).
4. **api.ts:** pass `lineEndNumber` through `add_review_comment`.
5. **ReviewScreen.tsx:** clicking a line number starts a selection; shift-click on another line of the same file/side extends it (`Anchor` gains `lineEndNumber`); the composer/thread is anchored at the **start** line (`commentsAt` matching unchanged); in-range lines get a `diff-line--in-range` highlight (new CSS class; low-opacity accent from the `#baa77c` family). Composer header reads "new lines 12–18".

**Verify:** `cargo test`, `npm test`, `npm run build`; manual range-comment round-trip.

---

## Phase order & dependency

A1 → A2 → B → C → D → E. C and D depend on nothing from A/B except the review-header editor button (B); if parallelized, land B before C, and D after C.

## Notes / residual risks

- The naming background thread must **never** update a task that left `draft` (the `status='draft'` guard in `apply_generated_naming` is load-bearing for `confirm_task`'s merge correctness).
- `db::reset_task_run` clears `branch_name` on turn-insert failure; `prepare_run` falls back to `naming::fallback_branch_name`, so recovery is safe.
- Terminal editors (vim/nvim) launched from a GUI app need the `cmd /K` / terminal-emulator wrapper; if detection proves flaky on some setup, they are the first candidates to drop from the registry.
