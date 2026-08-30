import { useEffect, useRef, useMemo } from "react";
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
    case "changes_requested": return 2;
    case "approved": return 3;
    case "failed":
    case "cancelled": return -1; // error / terminal
    default: return 0;
  }
}

function stepLabel(s: TaskStatus): string {
  if (s === "awaiting_review" || s === "changes_requested") return "Pending review";
  return STATUS_LABELS[s] ?? s;
}

/** Parse a JSONL log line and pull out a readable text fragment. */
function summariseLine(raw: string): string | null {
  try {
    const obj = JSON.parse(raw) as Record<string, unknown>;
    const type = String(obj["type"] ?? "");

    // agent_message / assistant messages carry the actual text
    const msg = obj["message"] ?? obj["msg"] ?? obj["content"];
    if (typeof msg === "string" && msg.trim()) {
      return `[${type}] ${msg.trim()}`;
    }

    // item events may have nested content
    if (obj["item"]) {
      const item = obj["item"] as Record<string, unknown>;
      const content = item["content"] ?? item["text"] ?? item["output"];
      if (typeof content === "string" && content.trim()) {
        return `[${type}] ${content.trim().slice(0, 200)}`;
      }
    }

    // Turn lifecycle events
    if (type === "turn.completed") return "✓ Turn completed";
    if (type === "turn.failed") {
      const err = obj["error"];
      return `✗ Turn failed${typeof err === "string" ? `: ${err}` : ""}`;
    }
    if (type === "thread.started") return "↻ Agent thread started";
    if (type === "agent.reasoning" || type === "agent.thinking") {
      const txt = obj["text"] ?? obj["reasoning"];
      if (typeof txt === "string") return `… ${txt.trim().slice(0, 120)}`;
    }

    // Fallback: show the type
    return type ? `[${type}]` : null;
  } catch {
    // Plain text line (e.g. stderr)
    const trimmed = raw.trim();
    return trimmed.length > 0 ? trimmed : null;
  }
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
        const label = stepLabel(step);
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

  const kindLabel = turn.kind === "initial" ? "Initial run" : "Follow-up";
  const statusClass = turn.status === "completed" ? "turn-status--done"
    : turn.status === "failed" || turn.status === "cancelled" ? "turn-status--error"
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
  onCancelTask: () => void;
  onOpenReview: () => void;
}

export function TaskFollowUpPanel({ task, onRunTask, onCancelTask, onOpenReview }: Props) {
  const {
    taskTurns,
    taskOutput,
    taskEvents,
    isLoadingTaskOutput,
    loadTaskOutput,
  } = useStore();

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
      };
      return [...turns, fake];
    }
    return turns;
  }, [turns, liveTurnId, task.id]);

  const isRunning = task.status === "running";
  const canRun = task.status === "draft";
  const needsReview = task.status === "awaiting_review" || task.status === "changes_requested";

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
              Run task
            </button>
          )}
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
