# Forge

Forge is a Tauri desktop workspace for running Codex tasks in isolated git worktrees and reviewing the resulting changes.

## Status

The project is currently in **Phase I — Scaffold**. The UI shell is present, while task persistence and execution are intentionally not wired yet. See [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for the phase handoff and [project-proposal.md](project-proposal.md) for the full v1 design.

## Prerequisites

- Node.js and npm
- Rust and Cargo
- Tauri v2 system prerequisites for the target OS
- `git` and an authenticated `codex` executable on `PATH` (needed from Phase III onward)

## Development

```text
npm install
npm run tauri dev
```

The frontend can also be run by itself with `npm run dev`.
