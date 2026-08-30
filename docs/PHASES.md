# v1 implementation phases

This project follows the nine implementation batches in `project-proposal.md`, renamed as phases so each future conversation has a bounded scope and a durable handoff point.

| Phase | Proposal batch | Scope | Status |
|---|---:|---|---|
| [I — Scaffold](phases/phase-I-scaffold.md) | 0 | Tauri + React/Vite shell and secure plugin capabilities | **Implemented; validation pending** |
| [II — Persistence and CRUD](phases/phase-II-persistence-crud.md) | 1 | SQLite schema, migrations, projects, and draft tasks | **Implemented; validation pending** |
| [III — Git worktrees](phases/phase-III-git-worktrees.md) | 2 | Repository checks, isolated worktrees, diff, commit, and merge | **Implemented; validation pending** |
| [IV — Codex runner](phases/phase-IV-codex-runner.md) | 3 | Codex process runner, JSONL events, logs, health check, cancellation | **Implemented; validation pending** |
| [V — Execution orchestration](phases/phase-V-execution-orchestration.md) | 4 | Run lifecycle, event streaming, concurrency, and raw diff review state | **Implemented; validation passed** |
| [VI — Diff review UI](phases/phase-VI-diff-review-ui.md) | 5 | GitHub-style diff display, inline comments, confirm/request changes UI | **Implemented; validation passed** |
| [VII — Follow-up loop](phases/phase-VII-follow-up-loop.md) | 6 | Resume turns, unresolved comments, confirm/commit/merge cleanup | **Implemented; validation passed** |
| [VIII — Settings](phases/phase-VIII-settings.md) | 7 | Binary configuration, health status, concurrency, and merge preference | **Implemented; validation passed** |
| [IX — Packaging](phases/phase-IX-packaging.md) | 8 | Windows installer configuration and cross-platform smoke checklist | **Implemented; validation passed** |

## Conversation handoff

### Current phase

- Phase: IX — Packaging
- Status: Implemented; validation passed
- Next phase: v1 implementation complete
- Source of truth: the phase document linked above plus the `project-proposal.md` sections referenced there.

### Decisions and constraints

- The v1 architecture remains Tauri v2, Rust backend, React/TypeScript frontend, SQLite persistence, and Codex through a spawned terminal process.
- The shell capability allows only the two named system commands, `codex` and `git`; both accept arguments because later phases construct their command lines.
- The app is branded “Tanoor” in the shell. This is a presentation name only and does not change the proposal’s domain model.
- Submitted review comments are resolved only after a resume process starts, making launch failures safe to retry.
- Confirmation preserves the task branch by default; its per-task merge checkbox starts from the persisted global preference.

### Completed work

- Built project/task persistence, isolated git worktrees, Codex JSONL execution, cancellation, and bounded concurrency.
- Added global and task-scoped event delivery with cumulative diff persistence.
- Added a unified-diff parser, expandable review dialog, old/new line and file comments, and resolution controls.
- Added resumed turns on the stored Codex thread with structured unresolved-comment feedback and repeated event/diff processing.
- Added commit, optional merge, approval, branch-aware worktree cleanup, and conflict-abort recovery.
- Added persisted Codex/git overrides, tool and authentication health, live concurrency control, and a merge-on-confirm default while keeping `workspace-write` immutable.
- Added platform-scoped MSI/NSIS configuration, a stable MSI upgrade identity, bundle metadata, an artifact verifier, signing guidance, and cross-platform release smoke checklists.

### Verification record

- `cargo test --manifest-path src-tauri/Cargo.toml`: 56 passed.
- `npm test`: added, removed, and renamed diff fixtures passed.
- `npm run build`: TypeScript and Vite production build passed.
- `npm run bundle:windows`: produced MSI and NSIS installers.
- `scripts/verify-windows-bundle.ps1`: found both artifacts and reported their unsigned status and SHA-256 hashes.
- `git diff --check`: passed.
- Visual browser smoke testing remains pending because no browser surface was connected.

### Changes from the original plan

- The editor-first shell replaces the proposal's generic task-list presentation without changing the domain model.
- A global `forge:task-event` companion channel supplements the specified task-scoped channel so background tasks stay synchronized.
- Binary and execution preferences are persisted in SQLite and applied to new operations without an app restart.

### Release checks carried forward

- Run the clean-install, upgrade, launch, uninstall, PATH, and process-tree checks in `docs/RELEASE_CHECKLIST.md` on the release OS matrix.
- Test Windows process-tree cancellation with a real Codex child process from the installed package before release.
- Preserve the warning that uncommitted main-worktree changes are not copied into task worktrees.
