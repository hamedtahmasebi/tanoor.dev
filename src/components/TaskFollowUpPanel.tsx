import { useEffect, useRef, useMemo, useState } from "react";
import { useStore } from "../store";
import type { Task, TaskStatus, TaskTurn, TaskEvent } from "../types";

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

const STATUS_STEPS: TaskStatus[] = [
  "draft",
  "running",
  "awaiting_review",
  "approved",
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
}

function StatusStepper({ status }: StatusStepperProps) {
  const current = stepIndex(status);
  const isError = current === -1;

  return (
    <div className="followup-stepper">
      {STATUS_STEPS.map((step, i) => {
        const completed = !isError && i < current;
        const active = !isError && i === current;
        const label = i === current ? stepLabel(status) : stepLabel(step);
        return (
          <div key={step} className="followup-step">
            <div className={[
              "followup-step-dot",
              completed ? "followup-step-dot--done" : "",
              active ? "followup-step-dot--active" : "",
              isError && i === 1 ? "followup-step-dot--error" : "",
            ].filter(Boolean).join(" ")}>
              {completed ? "✓" : active && status === "running"
                ? <span className="followup-step-spinner" />
                : i + 1}
            </div>
            <span className={[
              "followup-step-label",
              active ? "followup-step-label--active" : "",
              completed ? "followup-step-label--done" : "",
            ].filter(Boolean).join(" ")}>{label}</span>
            {i < STATUS_STEPS.length - 1 && (
              <div className={`followup-step-line${completed ? " followup-step-line--done" : ""}`} />
            )}
          </div>
        );
      })}
      {isError && (
        <div className="followup-step">
          <div className="followup-step-dot followup-step-dot--error">✗</div>
          <span className="followup-step-label followup-step-label--error">
            {STATUS_LABELS[status] ?? status}
          </span>
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
  onOpenReview: () => void;
}

export function TaskFollowUpPanel({ task, onRunTask, onRetryTask, onCancelTask, onOpenReview }: Props) {
  const {
    taskTurns,
    taskOutput,
    taskEvents,
    isLoadingTaskOutput,
    loadTaskOutput,
    editors,
    openInEditor,
  } = useStore();
  const [showEditorDropdown, setShowEditorDropdown] = useState(false);
  const [focusedEditor, setFocusedEditor] = useState(0);
  const editorListRef = useRef<HTMLDivElement>(null);

  const turns: TaskTurn[] = taskTurns[task.id] ?? [];
  const liveEvents = taskEvents[task.id] ?? [];

  // Load saved output whenever the task changes
  useEffect(() => {
    void loadTaskOutput(task.id);
  }, [task.id, loadTaskOutput]);

  // Re-fetch saved output when the task finishes a run (status leaves "running")
  const prevStatusRef = useRef(task.status);
  useEffect(() => {
    if (prevStatusRef.current === "running" && task.status !== "running") {
      void loadTaskOutput(task.id);
    }
    prevStatusRef.current = task.status;
  }, [task.status, task.id, loadTaskOutput]);

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
  const needsReview = task.status === "awaiting_review" || task.status === "changes_requested";

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

        <StatusStepper status={task.status} />

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
          {needsReview && (
            <button className="quiet-button" type="button" onClick={onOpenReview}>
              Review changes
            </button>
          )}
        </div>
      </div>

      <div className="followup-divider" />

      {/* ── Prompt ── */}
      <div className="followup-prompt-section">
        <span className="detail-label">Prompt</span>
        <p className="detail-prompt">{task.prompt}</p>
        {task.fileRefs.length > 0 && (
          <div className="detail-context" style={{ marginTop: 12 }}>
            <span className="detail-label">Context</span>
            {task.fileRefs.map((ref) => <code key={ref}>@{ref}</code>)}
          </div>
        )}
      </div>

      {/* ── Agent output ── */}
      {(allTurns.length > 0 || isRunning) && (
        <>
          <div className="followup-divider" />
          <div className="followup-output-section">
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
      {needsReview && (
        <>
          <div className="followup-divider" />
          <div className="detail-review-card">
            <div>
              <strong>Changes are ready for review</strong>
              <span>Inspect the cumulative diff and leave line-level feedback.</span>
            </div>
            <button className="quiet-button" type="button" onClick={onOpenReview}>
              Review changes
            </button>
          </div>
        </>
      )}
    </div>
  );
}
