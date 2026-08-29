# Phase VI — Diff review UI

Status: **Pending**

Maps to `project-proposal.md` Batch 5.

## Objective

Turn the raw review state into a GitHub-style cumulative diff experience.

## Deliverables

- Unified-diff parser and file/hunk display.
- Inline comment anchors by file, line, and side.
- `add_review_comment`, `resolve_review_comment`, and `list_review_comments` commands.
- Confirm and request-changes controls.
- Zustand store actions for the run/review/confirm flow and modal coordination.
- Fixture-driven UI tests for added, removed, and renamed files.

## Handoff requirements

Record the parsed diff model, comment anchoring rules, and store action contracts.
