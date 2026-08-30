# Tanoor

Tanoor is a Tauri desktop workspace for running Codex tasks in isolated git worktrees and reviewing the resulting changes.

## Status

All nine v1 phases are implemented. The frontend is an editor-first desktop workflow with isolated task execution, cumulative GitHub-style diff review, inline feedback, resumed Codex turns, commit/merge confirmation, and persisted operational settings. Windows MSI/NSIS packaging and the cross-platform release checklist are documented in [docs/RELEASE_CHECKLIST.md](docs/RELEASE_CHECKLIST.md). See [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for the handoff and [project-proposal.md](project-proposal.md) for the full v1 design.

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

## Packaging

On Windows, build both configured installer formats with:

```text
npm run bundle:windows
powershell -ExecutionPolicy Bypass -File scripts/verify-windows-bundle.ps1
```

The Windows installers are unsigned until a release owner configures the documented signing integration. Use `npm run bundle` for the native targets configured by Tauri on Linux or macOS.
