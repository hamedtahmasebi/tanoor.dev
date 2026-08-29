# Phase IV — Codex runner

Status: **Pending**

Maps to `project-proposal.md` Batch 3.

## Objective

Isolate Codex CLI process and protocol details behind the `AgentRunner` trait.

## Deliverables

- `AgentRunner`, `EventStream`, and `RunHandle` abstractions.
- `CodexExecRunner` invoking `codex exec --json` with the proposal’s sandbox flags.
- Typed JSONL `CodexEvent` parsing, raw stdout/stderr transcript persistence, and `thread.started` capture.
- Health/auth check for Settings.
- Cancellation that reaches the process tree on Windows and process groups on Unix where applicable.
- CLI-version smoke tests, especially `resume` flag support.

## Handoff requirements

Record the pinned CLI version and observed event/flag behavior before Phase V depends on it.
