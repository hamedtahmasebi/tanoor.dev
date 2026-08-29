# v1 implementation phases

This project follows the nine implementation batches in `project-proposal.md`, renamed as phases so each future conversation has a bounded scope and a durable handoff point.

| Phase | Proposal batch | Scope | Status |
|---|---:|---|---|
| [I — Scaffold](phases/phase-I-scaffold.md) | 0 | Tauri + React/Vite shell and secure plugin capabilities | **Implemented; validation pending** |
| [II — Persistence and CRUD](phases/phase-II-persistence-crud.md) | 1 | SQLite schema, migrations, projects, and draft tasks | Pending |
| [III — Git worktrees](phases/phase-III-git-worktrees.md) | 2 | Repository checks, isolated worktrees, diff, commit, and merge | Pending |
| [IV — Codex runner](phases/phase-IV-codex-runner.md) | 3 | Codex process runner, JSONL events, logs, health check, cancellation | Pending |
| [V — Execution orchestration](phases/phase-V-execution-orchestration.md) | 4 | Run lifecycle, event streaming, concurrency, and raw diff review state | Pending |
| [VI — Diff review UI](phases/phase-VI-diff-review-ui.md) | 5 | GitHub-style diff display, inline comments, confirm/request changes UI | Pending |
| [VII — Follow-up loop](phases/phase-VII-follow-up-loop.md) | 6 | Resume turns, unresolved comments, confirm/commit/merge cleanup | Pending |
| [VIII — Settings](phases/phase-VIII-settings.md) | 7 | Binary configuration, health status, concurrency, and merge preference | Pending |
| [IX — Packaging](phases/phase-IX-packaging.md) | 8 | Windows installer configuration and cross-platform smoke checklist | Pending |

## Conversation handoff

### Current phase

- Phase: I — Scaffold
- Status: Implemented; validation pending
- Next phase: II — Persistence and CRUD
- Source of truth: the phase document linked above plus the `project-proposal.md` sections referenced there.

### Decisions and constraints

- The v1 architecture remains Tauri v2, Rust backend, React/TypeScript frontend, SQLite persistence, and Codex through a spawned terminal process.
- Phase I maps exactly to proposal Batch 0. No backend behavior was invented ahead of the proposal.
- The shell capability allows only the two named system commands, `codex` and `git`; both accept arguments because later phases construct their command lines.
- The app is branded “Forge” in the shell. This is a presentation name only and does not change the proposal’s domain model.
- Task creation controls are visibly disabled until Phase II so the scaffold does not imply that persistence already works.

### Completed work

- Created the Vite + React + TypeScript frontend shell.
- Created the Tauri v2 Rust application and registered shell, dialog, filesystem, and window-state plugins.
- Added the default capability file with scoped `codex` and `git` execute entries.
- Added the initial task-list sidebar and empty main panel.
- Added setup documentation and phase-specific handoff documents.

### Verification record

- `rustfmt --edition 2021 --check src-tauri/src/lib.rs src-tauri/src/main.rs src-tauri/build.rs`: passed against the source files.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: blocked because Cargo 1.77.1 cannot parse the declared Rust 2024 edition; re-run with Rust 1.85+.
- `cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1`: blocked for the same Rust 2024/Cargo 1.77.1 incompatibility; re-run with Rust 1.85+.
- JSON parsing for the generated package, TypeScript, Tauri, and capability files: passed.
- `cargo check --manifest-path src-tauri/Cargo.toml`: not completed; the available Rust is 1.77.1 and crates.io access is unavailable.
- `npm run build`: not runnable in the current environment because Node/npm are unavailable; run after installing Node.js.
- `npm run tauri dev`: not runnable in the current environment because Node/npm are unavailable; run after installing Node.js and Tauri prerequisites.

### Changes from the original plan

- No functional scope changes.
- The proposal’s Batch 0 is called Phase I in the execution documents.
- The initial UI uses a small CSS-built illustration and text glyphs, avoiding a new icon dependency during the scaffold phase.

### Risks carried forward

- Validate the exact Tauri v2 plugin permission schema and generated capability schema during the first build on a machine with Node/npm installed.
- Pin and smoke-test the installed Codex CLI flags before Phase IV relies on them.
- Preserve the proposal’s warning about uncommitted changes in the main project folder when Phase II adds the new-task flow.
