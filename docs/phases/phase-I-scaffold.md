# Phase I — Scaffold

Status: **Implemented; validation pending**

Maps to `project-proposal.md` Batch 0.

## Objective

Create a launchable Tauri v2 desktop shell with a React/TypeScript/Vite frontend, the four proposal plugins, and a basic sidebar-plus-main-panel layout. There is no persistence or backend task logic in this phase.

## Acceptance criteria

- [x] Tauri v2 Rust entry point exists.
- [x] React + TypeScript + Vite frontend exists.
- [x] Shell, dialog, filesystem, and window-state plugins are registered.
- [x] The capability file exposes only `codex` and `git` as external commands.
- [x] The initial screen includes project selection context, task navigation, and an editor-first task panel.
- [x] The project has setup instructions and a documented handoff for the next conversation.
- [ ] Local build and desktop launch verified — blocked by missing Node/npm in the current environment.

## Files added

- `package.json`, `tsconfig.json`, `vite.config.ts`, `index.html`
- `src/main.tsx`, `src/App.tsx`, `src/styles.css`, `src/vite-env.d.ts`
- `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`
- `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`
- `.gitignore`, `README.md`

## Implementation notes

- The frontend uses a compact desktop-tool layout: activity rail, project/task tree, editor chrome, status bar, and neutral low-contrast colors inspired by modern code editors.
- Task creation is an inline editor workflow rather than a web-style modal. The composer supports `@` project-file references, `/` command suggestions, keyboard shortcuts, and the existing Phase II persistence API.
- Execution controls remain visibly staged until Phase V connects the runner, worktrees, and lifecycle state transitions.
- The shell plugin is initialized now, but no command is invoked until later orchestration work.
- `fs:default` and `dialog:default` are registered for the scoped project/file picking work planned in Phase II; their UI use is not implemented yet.
- The frontend package uses major-version ranges for Tauri v2 packages. The first dependency install should generate and commit a lockfile once the supported Node/npm toolchain is available.

## Handoff to Phase II

Start by installing dependencies and validating the Tauri capability schema. Then add the SQLite-backed project/task commands described in proposal §3 and Batch 1. Keep the existing shell layout, but replace the placeholder project switcher and disabled task controls with real state from the backend.

## Verification

The current environment has Rust/Cargo but no Node.js, npm, or pnpm. The available Rust is 1.77.1, while this project declares Rust 1.85+ to honor the proposal’s 2024 edition. The frontend build and Tauri launch require the Node toolchain and a compatible Rust toolchain.
