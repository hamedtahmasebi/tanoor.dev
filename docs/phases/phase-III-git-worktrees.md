# Phase III — Git worktrees

Status: **Pending**

Maps to `project-proposal.md` Batch 2.

## Objective

Build a unit-testable Rust `WorktreeManager` around the system `git` CLI.

## Deliverables

- Verify a project is a git repository and read its current HEAD.
- Add and remove one linked worktree per task using the task branch convention.
- Compute the cumulative unified diff against the task base commit.
- Commit task changes and provide the optional merge helper needed by confirmation.
- Test against throwaway repositories, including dirty main working trees and cleanup failures.

## Handoff requirements

Document exact git arguments, path quoting behavior, branch naming, and conflict/error mapping.
