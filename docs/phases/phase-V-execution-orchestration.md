# Phase V — Execution orchestration

Status: **Pending**

Maps to `project-proposal.md` Batch 4.

## Objective

Connect persistence, worktrees, and the Codex runner into the first end-to-end run lifecycle.

## Deliverables

- Run command creating a worktree and initial turn.
- Task-scoped event forwarding using `task:{id}:event`.
- Turn logs and state transitions through completion/failure/cancellation.
- Cumulative diff computation and `awaiting_review` transition.
- Bounded concurrency with default maximum of two tasks.
- Cancel command and partial-diff preservation.

## Handoff requirements

Record the state transition table, event payload shape, concurrency configuration, and failure recovery behavior.
