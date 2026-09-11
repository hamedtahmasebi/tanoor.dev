import { useEffect, useRef, useMemo, useState } from "react";
import { parseUnifiedDiff } from "../diff";
import { useStore } from "../store";
import { commentLocation, isTaskReviewable } from "./ReviewScreen";
import type { ReviewComment, Task, TaskStatus, TaskTurn, TaskEvent } from "../types";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const STATUS_LABELS: Record<TaskStatus, string> = {
  draft: "Draft",
  running: "Running",
  awaiting_review: "Pending review",
  changes_requested: "Changes requested",
  ready: "Ready to run",
  approved: "Finished",
  failed: "Failed",
  cancelled: "Cancelled",
};

/**
 * The four pipeline stages. Each one maps to a section of the panel that stays
 * reachable for the whole life of the task — including after it finished.
 */
export type StageId = "request" | "run" | "review" | "result";

const STAGE_STEPS: { id: StageId; status: TaskStatus }[] = [
  { id: "request", status: "draft" },
  { id: "run", status: "running" },
  { id: "review", status: "awaiting_review" },
  { id: "result", status: "approved" },
];

/** Returns a 0-4 progress index for the stepper. */
function stepIndex(status: TaskStatus): number {
  switch (status) {
    case "draft": return 0;
    case "running": return 1;
    case "awaiting_review":
    case "changes_requested":
    case "ready": return 2;
    case "approved": return 3;
    case "failed":
    case "cancelled": return -1; // error / terminal
    default: return 0;
  }
}

function stepLabel(s: TaskStatus): string {
  if (s === "awaiting_review" || s === "changes_requested") return "Pending review";
  if (s === "ready") return "Ready to run";
  return STATUS_LABELS[s] ?? s;
}

const firstString = (...values: unknown[]): string | null => {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value;
  }
  return null;
};

/** A short, single-line preview extracted from well-known text-bearing fields. */
function extractHeadline(record: Record<string, unknown>): string | null {
  const direct = firstString(
    record["message"], record["msg"], record["content"], record["text"],
    record["reasoning"], record["result"], record["error"], record["delta"],
  );
  if (direct) return direct;

  const part = record["part"];
  if (part && typeof part === "object") {
    const p = part as Record<string, unknown>;
    const tool = firstString(p["tool"], p["name"]);
    const text = firstString(p["text"], p["output"], p["reason"]);
    const combined = [tool, text].filter(Boolean).join(" → ");
    if (combined) return combined;
  }

  const item = record["item"];
  if (item && typeof item === "object") {
    const it = item as Record<string, unknown>;
    const detail = firstString(it["content"], it["text"], it["output"], it["name"]);
    if (detail) return detail;
  }

  return null;
}

/**
 * Format a JSONL log line for display. Nothing is truncated or dropped: a
 * readable headline is shown, and the full payload is appended as pretty JSON
 * so every field is available for debugging.
 */
function summariseLine(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    // Plain text line (e.g. stderr) — show it verbatim.
    return trimmed;
  }
  if (!parsed || typeof parsed !== "object") return trimmed;

  const record = parsed as Record<string, unknown>;
  const type = typeof record["type"] === "string" ? (record["type"] as string) : "event";
  const headlineText = extractHeadline(record)?.replace(/\s+/g, " ").trim();
  const headline = `[${type}]${headlineText ? ` ${headlineText}` : ""}`;

  const otherKeys = Object.keys(record).filter((key) => key !== "type");
  if (otherKeys.length === 0) return headline;

  let detail: string;
  try {
    detail = JSON.stringify(record, null, 2);
  } catch {
    detail = trimmed;
  }
  return `${headline}\n${detail}`;
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

interface StatusStepperProps {
  status: TaskStatus;
  /** Stages that have recorded content and can therefore be revealed. */
  available: Record<StageId, boolean>;
  onSelect: (stage: StageId) => void;
}

function StatusStepper({ status, available, onSelect }: StatusStepperProps) {
  const current = stepIndex(status);
  const isError = current === -1;
  const stepperRef = useRef<HTMLDivElement>(null);

  // Horizontal control: arrow keys walk the selectable stages with wrap-around.
  const moveFocus = (direction: 1 | -1) => {
    const steps = Array.from(stepperRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    if (steps.length === 0) return;
    const index = steps.findIndex((step) => step === document.activeElement);
    steps[(index + direction + steps.length) % steps.length].focus();
  };

  const stage = (id: StageId, label: string, dot: React.ReactNode, dotClass: string, labelClass: string) => (
    <button
      type="button"
      className="followup-step-nav"
      disabled={!available[id]}
      aria-current={!isError && STAGE_STEPS[current]?.id === id ? "step" : undefined}
      title={!available[id] ? `${label} — nothing recorded yet` : id === "review" ? "Open the changes & review screen" : `Show ${label.toLowerCase()}`}
      onClick={() => onSelect(id)}
    >
      <span className={dotClass}>{dot}</span>
      <span className={labelClass}>{label}</span>
    </button>
  );

  return (
    <div
      ref={stepperRef}
      className="followup-stepper"
      role="group"
      aria-label="Task pipeline"
      onKeyDown={(event) => {
        if (event.key === "ArrowRight") { event.preventDefault(); moveFocus(1); }
        else if (event.key === "ArrowLeft") { event.preventDefault(); moveFocus(-1); }
      }}
    >
      {STAGE_STEPS.map(({ id, status: step }, i) => {
        const completed = !isError && i < current;
        const active = !isError && i === current;
        const label = i === current ? stepLabel(status) : stepLabel(step);
        return (
          <div key={id} className="followup-step">
            {stage(id, label,
              completed ? "✓" : active && status === "running" ? <span className="followup-step-spinner" /> : i + 1,
              [
                "followup-step-dot",
                completed ? "followup-step-dot--done" : "",
                active ? "followup-step-dot--active" : "",
                isError && i === 1 ? "followup-step-dot--error" : "",
              ].filter(Boolean).join(" "),
              [
                "followup-step-label",
                active ? "followup-step-label--active" : "",
                completed ? "followup-step-label--done" : "",
              ].filter(Boolean).join(" "),
            )}
            {i < STAGE_STEPS.length - 1 && (
              <div className={`followup-step-line${completed ? " followup-step-line--done" : ""}`} />
            )}
          </div>
        );
      })}
      {isError && (
        <div className="followup-step">
          {stage("result", STATUS_LABELS[status] ?? status, "✗",
            "followup-step-dot followup-step-dot--error",
            "followup-step-label followup-step-label--error")}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Output display
// ---------------------------------------------------------------------------

interface OutputBlockProps {
  turn: TaskTurn;
  savedLines: string[];
  liveEvents: TaskEvent[];
  isActive: boolean;
}

function OutputBlock({ turn, savedLines, liveEvents, isActive }: OutputBlockProps) {
  const endRef = useRef<HTMLDivElement>(null);

  // Merge saved lines with live events for this turn
  const combinedLines = useMemo(() => {
    const result: Array<{ key: string; text: string; isLive?: boolean }> = [];

    // Saved JSONL log lines
    savedLines.forEach((line, i) => {
      const text = summariseLine(line);
      if (text) result.push({ key: `saved-${i}`, text });
    });

    // Live events arriving via Tauri event channel (not yet in the log file)
    liveEvents.forEach((evt, i) => {
      const raw = JSON.stringify({ type: evt.eventType, ...evt.raw });
      const text = summariseLine(raw);
      if (text) result.push({ key: `live-${i}`, text, isLive: true });
    });

    return result;
  }, [savedLines, liveEvents]);

  // Auto-scroll to bottom for active turns
  useEffect(() => {
    if (isActive) {
      endRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [combinedLines.length, isActive]);

  const kindLabel = `${turn.kind === "initial" ? "Initial run" : "Follow-up"}${turn.agentId ? ` · ${turn.agentId}` : ""}`;
  const statusClass = turn.status === "completed" ? "turn-status--done"
    : turn.status === "failed" || turn.status === "cancelled" ? "turn-status--error"
    : turn.status === "pending" ? "turn-status--pending"
    : "turn-status--running";

  return (
    <div className="followup-turn-block">
      <div className="followup-turn-header">
        <span className="followup-turn-kind">{kindLabel}</span>
        <span className={`followup-turn-status ${statusClass}`}>{turn.status}</span>
        <span className="followup-turn-time">{new Date(turn.startedAt).toLocaleTimeString()}</span>
      </div>
      {turn.prompt.trim() && (
        <details className="followup-turn-prompt">
          <summary>{turn.kind === "initial" ? "Prompt sent to the agent" : "Review feedback sent to the agent"}</summary>
          <pre>{turn.prompt}</pre>
        </details>
      )}
      {combinedLines.length === 0 ? (
        isActive ? (
          <div className="followup-output-waiting">
            <span className="loading-spinner" /> Waiting for agent output…
          </div>
        ) : (
          <div className="followup-output-empty">No output recorded</div>
        )
      ) : (
        <div className="followup-output-scroll">
          {combinedLines.map(({ key, text, isLive }) => (
            <div key={key} className={`followup-output-line${isLive ? " followup-output-line--live" : ""}`}>
              {text}
            </div>
          ))}
          <div ref={endRef} />
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main panel
// ---------------------------------------------------------------------------

interface Props {
  task: Task;
  onRunTask: () => void;
  onRetryTask: () => void;
  onCancelTask: () => void;
  /** Opens the review screen, optionally deep-linked to one comment. */
  onOpenReview: (focusCommentId?: string) => void;
}

export function TaskFollowUpPanel({ task, onRunTask, onRetryTask, onCancelTask, onOpenReview }: Props) {
  const {
    taskTurns,
    taskOutput,
    taskEvents,
    isLoadingTaskOutput,
    loadTaskOutput,
    reviewComments,
    loadReviewComments,
    editors,
    openInEditor,
  } = useStore();
  const [showEditorDropdown, setShowEditorDropdown] = useState(false);
  const [focusedEditor, setFocusedEditor] = useState(0);
  const editorListRef = useRef<HTMLDivElement>(null);
  const stageRefs = useRef<Partial<Record<StageId, HTMLDivElement | null>>>({});

  const turns: TaskTurn[] = taskTurns[task.id] ?? [];
  const liveEvents = taskEvents[task.id] ?? [];
  const comments: ReviewComment[] = reviewComments[task.id] ?? [];

  // Load saved output and the review history whenever the task changes
  useEffect(() => {
    void loadTaskOutput(task.id);
    void loadReviewComments(task.id, { silent: true });
  }, [task.id, loadTaskOutput, loadReviewComments]);

  // Re-fetch saved output when the task finishes a run (status leaves "running")
  const prevStatusRef = useRef(task.status);
  useEffect(() => {
    if (prevStatusRef.current === "running" && task.status !== "running") {
      void loadTaskOutput(task.id);
      void loadReviewComments(task.id, { silent: true });
    }
    prevStatusRef.current = task.status;
  }, [task.status, task.id, loadTaskOutput, loadReviewComments]);

  // Group live events by turn
  const liveByTurn = useMemo(() => {
    const map: Record<string, TaskEvent[]> = {};
    for (const evt of liveEvents) {
      if (!map[evt.turnId]) map[evt.turnId] = [];
      map[evt.turnId].push(evt);
    }
    return map;
  }, [liveEvents]);

  // Latest turn id from live events (may be mid-stream before DB is written)
  const liveTurnId = liveEvents.length > 0 ? liveEvents[liveEvents.length - 1].turnId : null;

  // Synthesise a live turn entry when we're streaming but DB hasn't written it yet
  const allTurns = useMemo(() => {
    if (liveTurnId && !turns.some((t) => t.id === liveTurnId)) {
      // Fabricate a placeholder turn
      const fake: TaskTurn = {
        id: liveTurnId,
        taskId: task.id,
        kind: turns.length === 0 ? "initial" : "follow_up",
        prompt: "",
        status: "running",
        logPath: "",
        startedAt: new Date().toISOString(),
        endedAt: null,
        agentId: task.agentId,
        agentModel: task.agentModel,
        agentEffort: task.agentEffort,
        agentThreadId: task.agentThreadId,
      };
      return [...turns, fake];
    }
    return turns;
  }, [turns, liveTurnId, task.id]);

  const isRunning = task.status === "running";
  const canRun = task.status === "draft" || task.status === "ready";
  const canRetry = task.status === "failed" || task.status === "cancelled";
  const needsReview = isTaskReviewable(task);
  const isFinished = task.status === "approved" || task.status === "failed" || task.status === "cancelled";

  // Diff totals for the review stage summary. The diff is kept on the task row
  // after approval, so these stay meaningful for finished tasks.
  const diffStats = useMemo(() => {
    const files = parseUnifiedDiff(task.diff ?? "");
    return {
      files: files.length,
      additions: files.reduce((total, file) => total + file.additions, 0),
      deletions: files.reduce((total, file) => total + file.deletions, 0),
    };
  }, [task.diff]);
  const hasChanges = Boolean(task.diff?.trim());
  const canOpenReview = hasChanges || comments.length > 0;

  // Comments grouped by the turn they were left on, i.e. one group per review
  // round, in chronological order.
  const commentRounds = useMemo(() => {
    const order = new Map(turns.map((turn, index) => [turn.id, index]));
    const groups = new Map<string, ReviewComment[]>();
    for (const comment of comments) {
      const group = groups.get(comment.turnId);
      if (group) group.push(comment);
      else groups.set(comment.turnId, [comment]);
    }
    return [...groups.entries()]
      .sort(([a], [b]) => (order.get(a) ?? Number.MAX_SAFE_INTEGER) - (order.get(b) ?? Number.MAX_SAFE_INTEGER))
      .map(([turnId, items], index) => ({ turnId, items, round: index + 1, turn: turns.find((turn) => turn.id === turnId) ?? null }));
  }, [comments, turns]);

  const stageAvailable: Record<StageId, boolean> = {
    request: true,
    run: allTurns.length > 0 || isRunning,
    review: canOpenReview || needsReview,
    result: isFinished,
  };
  // The review stage lives on its own screen, so its step deep-links there
  // instead of scrolling to the summary card.
  const revealStage = (stage: StageId) => {
    if (stage === "review") { onOpenReview(); return; }
    stageRefs.current[stage]?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const selectEditor = (editorId: string) => {
    setShowEditorDropdown(false);
    void openInEditor(task.id, editorId).catch(() => undefined);
  };
  const moveEditorFocus = (direction: 1 | -1) => {
    const next = (focusedEditor + direction + editors.length) % editors.length;
    setFocusedEditor(next);
    requestAnimationFrame(() => editorListRef.current?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus());
  };

  return (
    <div className="followup-panel">
      {/* ── Status header ── */}
      <div className="followup-header">
        <div className="followup-title-row">
          <div>
            <p className="detail-kicker">Task</p>
            <h1 className="followup-title">{task.title}</h1>
          </div>
          <span className={`detail-status detail-status--${task.status}`}>
            {STATUS_LABELS[task.status] ?? task.status}
          </span>
        </div>

        <StatusStepper status={task.status} available={stageAvailable} onSelect={revealStage} />

        <div className="followup-actions">
          {isRunning && (
            <button className="quiet-button" type="button" onClick={onCancelTask}>
              Cancel run
            </button>
          )}
          {canRun && (
            <button className="quiet-button" type="button" onClick={onRunTask}>
              {task.status === "ready" ? "Start follow-up" : "Run task"}
            </button>
          )}
          {canRetry && (
            <button className="quiet-button" type="button" onClick={onRetryTask} title="Reset and run this task again">
              Retry task
            </button>
          )}
          <div className="editor-launcher">
            <button className="quiet-button" type="button" disabled={!task.worktreePath || editors.length === 0} title={!task.worktreePath ? "This task has no worktree yet — run it first" : editors.length === 0 ? "No supported editor found" : "Open worktree in editor"} aria-haspopup={editors.length > 1 ? "listbox" : undefined} aria-expanded={showEditorDropdown} onClick={() => { if (editors.length === 1) selectEditor(editors[0].id); else if (editors.length > 1) { setFocusedEditor(0); setShowEditorDropdown((value) => !value); requestAnimationFrame(() => editorListRef.current?.querySelector<HTMLButtonElement>("button")?.focus()); } }}>
              Open in editor
            </button>
            {showEditorDropdown && editors.length > 1 && <div ref={editorListRef} className="project-dropdown editor-dropdown" role="listbox" aria-label="Choose editor" onKeyDown={(event) => { if (event.key === "ArrowDown") { event.preventDefault(); moveEditorFocus(1); } else if (event.key === "ArrowUp") { event.preventDefault(); moveEditorFocus(-1); } else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); selectEditor(editors[focusedEditor].id); } else if (event.key === "Escape") { event.preventDefault(); setShowEditorDropdown(false); } }}>
              {editors.map((editor, index) => <button key={editor.id} className="project-dropdown-item" type="button" role="option" aria-selected={index === focusedEditor} tabIndex={index === focusedEditor ? 0 : -1} onFocus={() => setFocusedEditor(index)} onClick={() => selectEditor(editor.id)}><span>{editor.label}</span><small>{editor.command}</small></button>)}
            </div>}
          </div>
          {canOpenReview && (
            <button className="quiet-button" type="button" onClick={() => onOpenReview()}>
              {needsReview ? "Review changes" : "View changes"}
            </button>
          )}
        </div>
      </div>

      <div className="followup-divider" />

      {/* ── Stage 1: request ── */}
      <div className="followup-prompt-section" ref={(node) => { stageRefs.current.request = node; }}>
        <span className="detail-label">Prompt</span>
        <p className="detail-prompt">{task.prompt}</p>
        {task.fileRefs.length > 0 && (
          <div className="detail-context" style={{ marginTop: 12 }}>
            <span className="detail-label">Context</span>
            {task.fileRefs.map((ref) => <code key={ref}>@{ref}</code>)}
          </div>
        )}
        {task.agentId && (
          <div className="detail-context" style={{ marginTop: 12 }}>
            <span className="detail-label">Agent</span>
            <code>{[task.agentId, task.agentModel, task.agentEffort].filter(Boolean).join(" · ")}</code>
          </div>
        )}
      </div>

      {/* ── Stage 2: run ── */}
      {(allTurns.length > 0 || isRunning) && (
        <>
          <div className="followup-divider" />
          <div className="followup-output-section" ref={(node) => { stageRefs.current.run = node; }}>
            <span className="detail-label">Agent output</span>
            {isLoadingTaskOutput && allTurns.length === 0 ? (
              <div className="followup-output-waiting">
                <span className="loading-spinner" /> Loading output…
              </div>
            ) : (
              <div className="followup-turns">
                {allTurns.map((turn) => (
                  <OutputBlock
                    key={turn.id}
                    turn={turn}
                    savedLines={taskOutput[turn.id] ?? []}
                    liveEvents={liveByTurn[turn.id] ?? []}
                    isActive={turn.id === liveTurnId || (isRunning && turn.id === allTurns[allTurns.length - 1]?.id)}
                  />
                ))}
              </div>
            )}
          </div>
        </>
      )}

      {/* ── Review card ── */}
      {/* ── Stage 3: review ── */}
      {stageAvailable.review && (
        <>
          <div className="followup-divider" />
          <div className="followup-review-section" ref={(node) => { stageRefs.current.review = node; }}>
            <span className="detail-label">Changes &amp; review</span>
            <div className="detail-review-card">
              <div>
                <strong>{needsReview ? "Changes are ready for review" : hasChanges ? "Recorded changes" : "No recorded changes"}</strong>
                <span>
                  {needsReview
                    ? "Inspect the cumulative diff and leave line-level feedback."
                    : hasChanges
                      ? `${diffStats.files} ${diffStats.files === 1 ? "file" : "files"} · +${diffStats.additions} −${diffStats.deletions} · read-only`
                      : "This task never produced a cumulative diff."}
                </span>
              </div>
              <button className="quiet-button" type="button" disabled={!canOpenReview} title={canOpenReview ? undefined : "Nothing was recorded for this task yet"} onClick={() => onOpenReview()}>
                {needsReview ? "Review changes" : "View changes"}
              </button>
            </div>

            {commentRounds.length > 0 && (
              <div className="followup-comment-rounds">
                {commentRounds.map(({ turnId, items, round, turn }) => (
                  <div className="followup-comment-round" key={turnId}>
                    <div className="followup-comment-round-header">
                      <span>Review round {round}{turn ? ` · ${turn.kind === "initial" ? "initial run" : "follow-up"}` : ""}</span>
                      <span>{items.filter((comment) => !comment.resolved).length} open · {items.filter((comment) => comment.resolved).length} resolved</span>
                    </div>
                    {items.map((comment) => (
                      <article className={`review-comment${comment.resolved ? " resolved" : ""}`} key={comment.id}>
                        <div>
                          <span>{comment.resolved ? "Resolved" : "Open"}</span>
                          <time>{new Date(comment.createdAt).toLocaleString()}</time>
                        </div>
                        <code className="followup-comment-anchor">{commentLocation(comment)}</code>
                        <p>{comment.body}</p>
                        {comment.assignedAgentId && (
                          <div className="review-comment-assignment">
                            <label>Sent to <code>{[comment.assignedAgentId, comment.assignedModel, comment.assignedEffort].filter(Boolean).join(" · ")}</code></label>
                          </div>
                        )}
                        <button type="button" onClick={() => onOpenReview(comment.id)}>View in diff →</button>
                      </article>
                    ))}
                  </div>
                ))}
              </div>
            )}
          </div>
        </>
      )}

      {/* ── Stage 4: result ── */}
      {isFinished && (
        <>
          <div className="followup-divider" />
          <div className="followup-result-section" ref={(node) => { stageRefs.current.result = node; }}>
            <span className="detail-label">Result</span>
            <div className="detail-review-card">
              <div>
                <strong>
                  {task.status === "approved" ? "Task approved" : task.status === "failed" ? "Run failed" : "Run cancelled"}
                </strong>
                <span>
                  {task.status === "approved"
                    ? task.branchName
                      ? `Committed on ${task.branchName}; the worktree was removed.`
                      : "Committed and merged into the project branch."
                    : "The full agent output above records what happened before the run stopped."}
                </span>
                <span>Last updated {new Date(task.updatedAt).toLocaleString()}</span>
              </div>
              <button className="quiet-button" type="button" disabled={!canOpenReview} title={canOpenReview ? undefined : "This task has no recorded diff"} onClick={() => onOpenReview()}>
                View changes
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
