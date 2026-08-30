# Phase VII — Follow-up loop

Status: **Implemented; validation passed**

Maps to `project-proposal.md` Batch 6.

## Objective

Complete the review loop: unresolved comments become one Codex follow-up turn, and confirmation commits and optionally merges the task.

## Deliverables

- Follow-up prompt formatting from unresolved comments and optional reviewer note.
- `codex exec resume` integration using the stored thread ID.
- New `turn` row and repeated event/diff pipeline.
- Commit, optional merge, approved state, and worktree cleanup.
- Clear plain errors for merge conflicts and cleanup failures.

## Handoff requirements

Record prompt formatting, confirmation ordering, and recovery semantics for partial failures.

## Implemented contract

- `request_changes` serializes unresolved anchors and the optional reviewer note into one JSON-backed prompt and resumes the stored thread in the existing worktree.
- A new `follow_up` turn enters `running` and uses the Phase V event/diff consumer unchanged.
- Comments are resolved only after Codex starts; launch failures produce `changes_requested` and retain feedback for retry.
- `confirm_task` commits, optionally merges, cleans the worktree, and only then persists `approved`.
- Merge initially defaults off. Phase VIII persists a global default; unmerged approval retains the task branch and merged approval deletes it during cleanup.
- Conflicted merges are aborted automatically. Git failures retain an awaiting-review task branch/worktree for retry and return plain recovery guidance.

See `IMPLEMENTATION_STATUS.md` for exact prompt fields, ordering, recovery semantics, and validation results.
