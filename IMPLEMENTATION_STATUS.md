# Forge v1 implementation status

This is the cross-conversation handoff file. The full phase specifications live in [docs/PHASES.md](docs/PHASES.md) and `docs/phases/`.

| Phase | Scope | Status |
|---|---|---|
| [I — Scaffold](docs/phases/phase-I-scaffold.md) | Tauri + React/Vite shell, plugins, capabilities | **Implemented; validation pending** |
| [II — Persistence and CRUD](docs/phases/phase-II-persistence-crud.md) | SQLite, projects, draft task CRUD | Pending |
| [III — Git worktrees](docs/phases/phase-III-git-worktrees.md) | Repository/worktree/diff/commit helpers | Pending |
| [IV — Codex runner](docs/phases/phase-IV-codex-runner.md) | Codex process, JSONL, logs, health, cancellation | Pending |
| [V — Execution orchestration](docs/phases/phase-V-execution-orchestration.md) | Run lifecycle, streaming, concurrency | Pending |
| [VI — Diff review UI](docs/phases/phase-VI-diff-review-ui.md) | Diff display, inline comments, review actions | Pending |
| [VII — Follow-up loop](docs/phases/phase-VII-follow-up-loop.md) | Resume turns, confirmation, cleanup | Pending |
| [VIII — Settings](docs/phases/phase-VIII-settings.md) | Binary status, concurrency, merge preferences | Pending |
| [IX — Packaging](docs/phases/phase-IX-packaging.md) | Windows installer and cross-platform smoke checks | Pending |

## Current handoff

- Current phase: **I — Scaffold** (implemented; validation pending)
- Next phase: **II — Persistence and CRUD**
- Proposal baseline: `project-proposal.md`, Batch 0 / §2 and §5
- Functional scope changes: none
- Naming change: proposal Batch 0 is called Phase I in these documents
- UI decision: the new-task controls are disabled until persistence exists in Phase II

## Phase I verification

- `rustfmt --edition 2021 --check src-tauri/src/lib.rs src-tauri/src/main.rs src-tauri/build.rs`: passed against the source files.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: blocked because Cargo 1.77.1 cannot parse the declared Rust 2024 edition; re-run with Rust 1.85+.
- `cargo metadata --manifest-path src-tauri/Cargo.toml --no-deps --format-version 1`: blocked for the same Rust 2024/Cargo 1.77.1 incompatibility; re-run with Rust 1.85+.
- JSON parsing for `package.json`, `tsconfig.json`, `src-tauri/tauri.conf.json`, and `src-tauri/capabilities/default.json`: passed.
- `npm run build`: not run; Node.js/npm are unavailable in the current environment.
- `cargo check`: not completed; the available Rust is 1.77.1, below the Rust 2024 requirement, and crates.io access is unavailable.
- `npm run tauri dev`: not run; Node.js/npm are unavailable.

## Decisions and carried-forward risks

- Tauri v2 with Rust backend and React/TypeScript frontend remains the selected architecture.
- External command capability is limited to `codex` and `git`; arguments are allowed because later phases construct their CLI invocations.
- The shell uses the system binaries through PATH, not bundled sidecars, as specified by the proposal.
- Tauri’s current plugin guidance requires Rust 1.77.2; the proposal’s Rust 2024 edition requires Rust 1.85+, so the project declares `rust-version = "1.85"`.
- Validate the generated Tauri capability schema after the first dependency install.
- Before Phase IV, pin and empirically test Codex CLI `exec`/`resume` flags and event shapes.
- Before Phase V, test Windows process-tree cancellation and preserve the proposal’s warning about uncommitted changes in the main project folder.
