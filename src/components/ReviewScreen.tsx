import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { parseUnifiedDiff, toSplitRows, type DiffFile, type DiffHunk, type DiffLine, type SplitRow } from "../diff";
import { useStore, type DiffViewMode } from "../store";
import type { AddReviewCommentInput, AgentModelCatalog, DiffSide, ReviewComment, Task, TaskStatus } from "../types";

/** Statuses where the review is still open for new feedback and a verdict. */
const REVIEWABLE_STATUSES: TaskStatus[] = ["awaiting_review", "changes_requested"];
export const isTaskReviewable = (task: Task) => REVIEWABLE_STATUSES.includes(task.status);

const DIFF_VIEW_MODES: { id: DiffViewMode; label: string }[] = [
  { id: "unified", label: "Unified" },
  { id: "split", label: "Split" },
];

interface Anchor { filePath: string; lineNumber: number | null; lineEndNumber: number | null; side: DiffSide | null; }
const commentsAt = (comments: ReviewComment[], anchor: Anchor) => comments.filter((comment) => comment.filePath === anchor.filePath && comment.lineNumber === anchor.lineNumber && comment.side === anchor.side);
const filePath = (file: DiffFile) => file.newPath ?? file.oldPath ?? file.displayPath;
const anchorKey = (path: string, side: DiffSide | null, lineNumber: number | null) => `${path}|${side ?? "file"}|${lineNumber ?? ""}`;
function anchorForLine(file: DiffFile, line: DiffLine, side: DiffSide): Anchor | null { const lineNumber = side === "old" ? line.oldLineNumber : line.newLineNumber; return lineNumber === null ? null : { filePath: filePath(file), lineNumber, lineEndNumber: null, side }; }
/** True when `candidate` falls inside the (possibly multi-line) pending anchor. */
const isInRange = (anchor: Anchor | null, candidate: Anchor | null) => Boolean(candidate && anchor && candidate.filePath === anchor.filePath && candidate.side === anchor.side && anchor.lineNumber !== null && candidate.lineNumber !== null && candidate.lineNumber >= anchor.lineNumber && candidate.lineNumber <= (anchor.lineEndNumber ?? anchor.lineNumber));
/** True when the composer should open right below this line pair. */
const anchorStartsAt = (anchor: Anchor | null, path: string, oldNumber: number | null, newNumber: number | null) => Boolean(anchor && anchor.filePath === path && anchor.lineNumber !== null && ((anchor.side === "old" && anchor.lineNumber === oldNumber) || (anchor.side === "new" && anchor.lineNumber === newNumber)));
/** `path:line–line (side)` label used wherever a comment is listed out of context. */
export const commentLocation = (comment: ReviewComment) => comment.lineNumber === null ? `${comment.filePath} · file` : `${comment.filePath}:${comment.lineNumber}${comment.lineEndNumber && comment.lineEndNumber !== comment.lineNumber ? `–${comment.lineEndNumber}` : ""}${comment.side ? ` (${comment.side})` : ""}`;

/** Everything the diff rows need to render comments and accept new ones. */
interface DiffInteraction {
  comments: ReviewComment[];
  anchor: Anchor | null;
  readOnly: boolean;
  focusId: string | null;
  catalogs: AgentModelCatalog[];
  onComment: (anchor: Anchor, extend: boolean) => void;
  onResolve: (id: string) => Promise<void>;
  onAssign: (comment: ReviewComment, agentId: string | null, model: string | null) => Promise<void>;
  onCancelAnchor: () => void;
  onSubmitComment: (input: AddReviewCommentInput) => Promise<void>;
}

function CommentThread({ comments, io }: { comments: ReviewComment[]; io: DiffInteraction }) {
  if (!comments.length) return null;
  return <div className="review-comment-thread">{comments.map((comment) => <article id={`review-comment-${comment.id}`} className={`review-comment${comment.resolved ? " resolved" : ""}${comment.id === io.focusId ? " review-comment--focus" : ""}`} key={comment.id}><div><span>{comment.resolved ? "Resolved" : "Review comment"}</span><time>{new Date(comment.createdAt).toLocaleString()}</time></div><p>{comment.body}</p>{io.readOnly ? (comment.assignedAgentId && <div className="review-comment-assignment"><label>Sent to <code>{comment.assignedAgentId}{comment.assignedModel ? ` · ${comment.assignedModel}` : ""}</code></label></div>) : <>{!comment.resolved && <div className="review-comment-assignment"><label>Agent <select value={comment.assignedAgentId ?? ""} onChange={(event) => { const agentId = event.target.value || null; void io.onAssign(comment, agentId, io.catalogs.find((item) => item.agentId === agentId)?.selectedModel ?? null); }}><option value="">Same as task</option>{io.catalogs.map((catalog) => <option key={catalog.agentId} value={catalog.agentId}>{catalog.agentLabel}</option>)}</select></label>{comment.assignedAgentId && <label>Model <select value={comment.assignedModel ?? ""} onChange={(event) => void io.onAssign(comment, comment.assignedAgentId, event.target.value || null)}>{(io.catalogs.find((item) => item.agentId === comment.assignedAgentId)?.models ?? []).map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}</select></label>}</div>}{!comment.resolved && <button type="button" onClick={() => void io.onResolve(comment.id).catch(() => undefined)}>Resolve</button>}</>}</article>)}</div>;
}

function LineNumberButton({ io, anchor, value, side, extraClass }: { io: DiffInteraction; anchor: Anchor | null; value: number | null; side: DiffSide; extraClass?: string }) {
  return <button type="button" className={`diff-line-number${extraClass ? ` ${extraClass}` : ""}`} disabled={io.readOnly || !anchor} title={!io.readOnly && anchor ? `Comment on ${side} line ${value} — hold Shift to extend` : undefined} onClick={(event) => anchor && io.onComment(anchor, event.shiftKey)}>{value ?? ""}</button>;
}

function CommentComposer({ anchor, onCancel, onSubmit }: { anchor: Anchor; onCancel: () => void; onSubmit: (input: AddReviewCommentInput) => Promise<void> }) {
  const [body, setBody] = useState(""); const [saving, setSaving] = useState(false);
  const submit = async () => { if (!body.trim()) return; setSaving(true); try { await onSubmit({ ...anchor, body }); onCancel(); } finally { setSaving(false); } };
  const rangeLabel = anchor.lineNumber === null ? "File comment" : anchor.lineEndNumber && anchor.lineEndNumber !== anchor.lineNumber ? `${anchor.side} lines ${anchor.lineNumber}–${anchor.lineEndNumber}` : `${anchor.side} line ${anchor.lineNumber}`;
  return <div className="review-comment-composer"><div>{rangeLabel}</div><textarea autoFocus value={body} onChange={(event) => setBody(event.target.value)} placeholder="Leave a clear, actionable comment…" /><div><button type="button" onClick={onCancel}>Cancel</button><button type="button" className="review-primary" disabled={saving || !body.trim()} onClick={() => void submit()}>{saving ? "Saving…" : "Add comment"}</button></div></div>;
}

// ---------------------------------------------------------------------------
// Diff rows — unified (one column) and split (old | new side by side)
// ---------------------------------------------------------------------------

function DiffLineRow({ file, line, io }: { file: DiffFile; line: DiffLine; io: DiffInteraction }) {
  const oldAnchor = anchorForLine(file, line, "old"); const newAnchor = anchorForLine(file, line, "new");
  const anchored = [...(oldAnchor ? commentsAt(io.comments, oldAnchor) : []), ...(newAnchor ? commentsAt(io.comments, newAnchor) : [])];
  const highlighted = isInRange(io.anchor, oldAnchor) || isInRange(io.anchor, newAnchor);
  return <Fragment><div className={`diff-line diff-line--${line.kind}${highlighted ? " diff-line--in-range" : ""}`}><LineNumberButton io={io} anchor={oldAnchor} value={line.oldLineNumber} side="old" /><LineNumberButton io={io} anchor={newAnchor} value={line.newLineNumber} side="new" /><span className="diff-line-marker">{line.kind === "addition" ? "+" : line.kind === "deletion" ? "−" : " "}</span><code>{line.content || " "}</code></div>{anchored.length > 0 && <CommentThread comments={anchored} io={io} />}</Fragment>;
}

function DiffSplitRow({ file, row, io }: { file: DiffFile; row: SplitRow; io: DiffInteraction }) {
  if (row.meta) return <div className="diff-split-row diff-split-row--meta"><code>{row.meta.content}</code></div>;
  const leftAnchor = row.left ? anchorForLine(file, row.left, "old") : null; const rightAnchor = row.right ? anchorForLine(file, row.right, "new") : null;
  const anchored = [...(leftAnchor ? commentsAt(io.comments, leftAnchor) : []), ...(rightAnchor ? commentsAt(io.comments, rightAnchor) : [])];
  const cellClass = (line: DiffLine | null, anchor: Anchor | null) => `diff-split-cell${line ? ` diff-split-cell--${line.kind}` : " diff-split-cell--empty"}${isInRange(io.anchor, anchor) ? " diff-split-cell--in-range" : ""}`;
  return <Fragment><div className="diff-split-row"><LineNumberButton io={io} anchor={leftAnchor} value={row.left?.oldLineNumber ?? null} side="old" /><code className={cellClass(row.left, leftAnchor)}>{row.left ? row.left.content || " " : ""}</code><LineNumberButton io={io} anchor={rightAnchor} value={row.right?.newLineNumber ?? null} side="new" extraClass="diff-split-divider" /><code className={cellClass(row.right, rightAnchor)}>{row.right ? row.right.content || " " : ""}</code></div>{anchored.length > 0 && <CommentThread comments={anchored} io={io} />}</Fragment>;
}

function HunkView({ file, hunk, mode, io }: { file: DiffFile; hunk: DiffHunk; mode: DiffViewMode; io: DiffInteraction }) {
  const path = filePath(file);
  const rows = useMemo(() => toSplitRows(hunk.lines), [hunk.lines]);
  const composer = (oldNumber: number | null, newNumber: number | null) => anchorStartsAt(io.anchor, path, oldNumber, newNumber) && io.anchor
    ? <CommentComposer anchor={io.anchor} onCancel={io.onCancelAnchor} onSubmit={io.onSubmitComment} />
    : null;
  return (
    <section className={`diff-hunk${mode === "split" ? " diff-hunk--split" : ""}`}>
      <header>{hunk.header}</header>
      {mode === "split"
        ? rows.map((row) => <Fragment key={row.id}><DiffSplitRow file={file} row={row} io={io} />{composer(row.left?.oldLineNumber ?? null, row.right?.newLineNumber ?? null)}</Fragment>)
        : hunk.lines.map((line) => <Fragment key={line.id}><DiffLineRow file={file} line={line} io={io} />{composer(line.oldLineNumber, line.newLineNumber)}</Fragment>)}
    </section>
  );
}

function DiffViewToggle({ mode, onChange }: { mode: DiffViewMode; onChange: (mode: DiffViewMode) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const move = (direction: 1 | -1) => {
    const index = DIFF_VIEW_MODES.findIndex((item) => item.id === mode);
    const next = DIFF_VIEW_MODES[(index + direction + DIFF_VIEW_MODES.length) % DIFF_VIEW_MODES.length];
    onChange(next.id);
    requestAnimationFrame(() => ref.current?.querySelector<HTMLButtonElement>(`[data-mode="${next.id}"]`)?.focus());
  };
  return (
    <div ref={ref} className="segmented-control segmented-control--inline" role="tablist" aria-label="Diff layout" onKeyDown={(event) => {
      if (event.key === "ArrowRight") { event.preventDefault(); move(1); }
      else if (event.key === "ArrowLeft") { event.preventDefault(); move(-1); }
    }}>
      {DIFF_VIEW_MODES.map((item) => (
        <button key={item.id} type="button" role="tab" data-mode={item.id} aria-selected={item.id === mode} tabIndex={item.id === mode ? 0 : -1} className={`segmented-control-item${item.id === mode ? " active" : ""}`} onClick={() => onChange(item.id)}>
          {item.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Review body
// ---------------------------------------------------------------------------

function ReviewContent({ task, readOnly }: { task: Task; readOnly: boolean }) {
  const { reviewComments, addReviewComment, resolveReviewComment, assignReviewComment, agentCatalogs, setReviewStep, closeReview, diffViewMode, setDiffViewMode, reviewFocusCommentId, clearReviewFocus } = useStore();
  const files = useMemo(() => parseUnifiedDiff(task.diff ?? ""), [task.diff]); const comments = reviewComments[task.id] ?? []; const [anchor, setAnchor] = useState<Anchor | null>(null);
  const additions = files.reduce((total, file) => total + file.additions, 0); const deletions = files.reduce((total, file) => total + file.deletions, 0); const unresolved = comments.filter((comment) => !comment.resolved).length;

  // Deep link: scroll the requested comment into view once the diff is painted,
  // then drop the highlight so it does not linger on the next visit.
  useEffect(() => {
    if (!reviewFocusCommentId) return;
    document.getElementById(`review-comment-${reviewFocusCommentId}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
    const timer = window.setTimeout(clearReviewFocus, 2600);
    return () => window.clearTimeout(timer);
  }, [reviewFocusCommentId, clearReviewFocus, diffViewMode, files]);

  // Comments can outlive the diff line they were anchored to (a follow-up turn
  // rewrote or removed it). Collect them so no feedback becomes unreachable.
  const renderedAnchors = useMemo(() => {
    const keys = new Set<string>();
    for (const file of files) {
      const path = filePath(file);
      keys.add(anchorKey(path, null, null));
      for (const hunk of file.hunks) for (const line of hunk.lines) {
        if (line.oldLineNumber !== null) keys.add(anchorKey(path, "old", line.oldLineNumber));
        if (line.newLineNumber !== null) keys.add(anchorKey(path, "new", line.newLineNumber));
      }
    }
    return keys;
  }, [files]);
  const detached = comments.filter((comment) => !renderedAnchors.has(anchorKey(comment.filePath, comment.side, comment.lineNumber)));

  const chooseAnchor = (next: Anchor, extend: boolean) => setAnchor((current) => {
    if (!extend || !current || current.filePath !== next.filePath || current.side !== next.side || current.lineNumber === null || next.lineNumber === null) return next;
    const lineNumber = Math.min(current.lineNumber, next.lineNumber);
    return { ...current, lineNumber, lineEndNumber: Math.max(current.lineNumber, next.lineNumber) };
  });

  const io: DiffInteraction = {
    comments,
    anchor,
    readOnly,
    focusId: reviewFocusCommentId,
    catalogs: agentCatalogs,
    onComment: chooseAnchor,
    onResolve: (id) => resolveReviewComment(task.id, id),
    onAssign: (comment, agentId, model) => assignReviewComment(task.id, comment.id, agentId, model, null),
    onCancelAnchor: () => setAnchor(null),
    onSubmitComment: (input) => addReviewComment(task.id, input),
  };

  return (
    <>
      <div className="review-summary">
        <span>{files.length} {files.length === 1 ? "file" : "files"}</span>
        <span className="review-additions">+{additions}</span>
        <span className="review-deletions">−{deletions}</span>
        <span>{readOnly ? `${comments.length} ${comments.length === 1 ? "comment" : "comments"}` : `${unresolved} unresolved`}</span>
        {readOnly && <span className="review-readonly-chip">read-only</span>}
        <DiffViewToggle mode={diffViewMode} onChange={setDiffViewMode} />
      </div>

      {files.length === 0 ? (
        <div className="review-empty">
          <strong>No changes to display</strong>
          <span>{task.diff?.trim() ? "The recorded diff could not be parsed." : "The task completed without a cumulative diff."}</span>
        </div>
      ) : files.map((file) => {
        const path = filePath(file);
        const fileComments = comments.filter((comment) => comment.filePath === path && comment.lineNumber === null);
        return (
          <details className={`diff-file${diffViewMode === "split" ? " diff-file--split" : ""}`} open key={file.id}>
            <summary>
              <span className={`diff-file-status diff-file-status--${file.status}`}>{file.status}</span>
              <code>{file.displayPath}</code>
              {file.status === "renamed" && <small>{file.oldPath} → {file.newPath}</small>}
              <span className="diff-file-totals"><b>+{file.additions}</b><i>−{file.deletions}</i></span>
            </summary>
            {!readOnly && (
              <div className="diff-file-toolbar">
                <button type="button" onClick={() => setAnchor({ filePath: path, lineNumber: null, lineEndNumber: null, side: null })}>＋ File comment</button>
              </div>
            )}
            {fileComments.length > 0 && <CommentThread comments={fileComments} io={io} />}
            {anchor?.filePath === path && anchor.lineNumber === null && <CommentComposer anchor={anchor} onCancel={() => setAnchor(null)} onSubmit={(input) => addReviewComment(task.id, input)} />}
            {file.binary
              ? <div className="diff-binary">Binary file changed</div>
              : file.hunks.map((hunk) => <HunkView key={hunk.id} file={file} hunk={hunk} mode={diffViewMode} io={io} />)}
          </details>
        );
      })}

      {detached.length > 0 && (
        <section className="review-detached">
          <header>Comments from earlier review rounds</header>
          <span>These lines are no longer part of the cumulative diff.</span>
          <CommentThread comments={detached} io={io} />
        </section>
      )}

      <footer className="review-actions">
        <div>
          <strong>{readOnly ? `${comments.length} ${comments.length === 1 ? "comment" : "comments"} · ${comments.length - unresolved} resolved` : unresolved ? `${unresolved} unresolved comments` : "Review ready"}</strong>
          <span>{readOnly ? "This task is no longer open for review — the diff and feedback are read-only." : "Comments stay anchored to this task turn."}</span>
        </div>
        {readOnly ? <button type="button" onClick={closeReview}>Back to task</button> : (
          <>
            <button type="button" onClick={() => setReviewStep("request_changes")}>Submit review</button>
            <button type="button" className="review-primary" disabled={unresolved > 0} title={unresolved > 0 ? "Resolve or submit comments before confirming" : undefined} onClick={() => setReviewStep("confirm")}>Confirm</button>
          </>
        )}
      </footer>
    </>
  );
}

function VerdictStep({ kind, task }: { kind: "confirm" | "request_changes"; task: Task }) {
  const { setReviewStep, reviewTaskId, reviewComments, submitReview, confirmTask, isSubmittingReview, settings, agentCatalogs } = useStore(); const [reviewerNote, setReviewerNote] = useState(""); const [merge, setMerge] = useState(settings?.mergeOnConfirm ?? false); const [requestAgent, setRequestAgent] = useState(task.agentId ?? settings?.defaultAgent ?? "codex"); const catalog = agentCatalogs.find((item) => item.agentId === requestAgent); const [requestModel, setRequestModel] = useState(catalog?.selectedModel ?? "");
  useEffect(() => { if (!requestModel && catalog) setRequestModel(catalog.selectedModel); }, [catalog, requestModel]);
  const unresolved = reviewTaskId ? (reviewComments[reviewTaskId] ?? []).filter((comment) => !comment.resolved) : []; const submittingReview = kind === "request_changes"; const canSubmit = submittingReview ? unresolved.length > 0 || Boolean(reviewerNote.trim()) : unresolved.length === 0;
  const submit = async () => { try { if (submittingReview) await submitReview(task.id, reviewerNote, { agentId: requestAgent, model: requestModel || null, effort: null }); else await confirmTask(task.id, merge); } catch { /* Store displays the actionable error. */ } };
  return <div className="review-deferred-step"><span className="review-step-icon">{submittingReview ? "↻" : "✓"}</span><h2>{submittingReview ? "Submit review" : "Confirm task"}</h2><p>{submittingReview ? "Feedback is queued as a follow-up message in this task's conversation. The task stays ready-to-run until you start it." : "Tanoor will commit the reviewed work, remove its worktree, and mark the task approved."}</p>{submittingReview ? <><div className="review-agent-selection"><label>Default agent for unassigned feedback <select value={requestAgent} onChange={(event) => { const next = event.target.value; setRequestAgent(next); setRequestModel(agentCatalogs.find((item) => item.agentId === next)?.selectedModel ?? ""); }}>{agentCatalogs.map((item) => <option key={item.agentId} value={item.agentId}>{item.agentLabel}</option>)}</select></label>{catalog && <label>Model <select value={requestModel} onChange={(event) => setRequestModel(event.target.value)}>{catalog.models.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}</select></label>}</div><label className="review-verdict-field"><span>Additional reviewer note <small>optional when comments are unresolved</small></span><textarea value={reviewerNote} onChange={(event) => setReviewerNote(event.target.value)} placeholder="Add context that is not tied to a specific diff line…" /></label></> : <label className="review-merge-option"><input type="checkbox" checked={merge} onChange={(event) => setMerge(event.target.checked)} /><span><strong>Merge into the current project branch</strong><small>Off by default. When off, the approved commit remains on <code>{task.branchName}</code>.</small></span></label>}<div className="review-boundary-note">{submittingReview ? "Comments use their per-comment assignment; unassigned feedback uses the selected agent. Each agent group runs in its own turn." : merge ? "A merge conflict is aborted automatically; the task branch and worktree remain available for retry." : "The task branch is preserved after worktree cleanup so you can merge or inspect it later."}</div><div className="review-deferred-actions"><button type="button" disabled={isSubmittingReview} onClick={() => setReviewStep("review")}>Back to review</button><button type="button" className="review-primary" disabled={isSubmittingReview || !canSubmit} onClick={() => void submit()}>{isSubmittingReview ? (submittingReview ? "Submitting…" : "Confirming…") : (submittingReview ? "Submit review" : "Confirm task")}</button></div></div>;
}

function EditorButton({ task }: { task: Task }) { const { editors, openInEditor } = useStore(); const [open, setOpen] = useState(false); const [focused, setFocused] = useState(0); const listRef = useRef<HTMLDivElement>(null); const select = (id: string) => { setOpen(false); void openInEditor(task.id, id).catch(() => undefined); }; const move = (direction: 1 | -1) => { const next = (focused + direction + editors.length) % editors.length; setFocused(next); requestAnimationFrame(() => listRef.current?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus()); }; return <div className="editor-launcher"><button className="quiet-button" type="button" disabled={!task.worktreePath || !editors.length} title={!task.worktreePath ? "This task has no worktree yet — run it first" : !editors.length ? "No supported editor found" : "Open worktree in editor"} onClick={() => { if (editors.length === 1) select(editors[0].id); else { setFocused(0); setOpen((value) => !value); requestAnimationFrame(() => listRef.current?.querySelector<HTMLButtonElement>("button")?.focus()); } }}>Open in editor</button>{open && editors.length > 1 && <div ref={listRef} className="project-dropdown editor-dropdown" role="listbox" onKeyDown={(event) => { if (event.key === "ArrowDown") { event.preventDefault(); move(1); } else if (event.key === "ArrowUp") { event.preventDefault(); move(-1); } else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); select(editors[focused].id); } else if (event.key === "Escape") { event.preventDefault(); setOpen(false); } }}>{editors.map((editor, index) => <button key={editor.id} className="project-dropdown-item" type="button" role="option" aria-selected={index === focused} tabIndex={index === focused ? 0 : -1} onFocus={() => setFocused(index)} onClick={() => select(editor.id)}><span>{editor.label}</span><small>{editor.command}</small></button>)}</div>}</div>; }

export function ReviewScreen({ task }: { task: Task }) {
  const { reviewTaskId, reviewStep, closeReview, isLoadingReview, error, clearError } = useStore();
  if (reviewTaskId !== task.id) return null;
  // A finished task keeps its diff and comments, but can no longer receive
  // feedback or a verdict, so the verdict steps are skipped entirely.
  const readOnly = !isTaskReviewable(task);
  const step = readOnly ? "review" : reviewStep;
  return <section className="review-screen" aria-label={`Review ${task.title}`}><header className="review-header"><div><span>{readOnly ? "Task review · read-only" : "Task review"}</span><h1>{task.title}</h1></div><div className="review-header-actions"><button type="button" className="quiet-button" onClick={closeReview}>← Back to task</button><EditorButton task={task} />{task.branchName && <code className="review-branch-chip">{task.branchName}</code>}</div></header>{error && <div className="review-dialog-error" role="alert"><span>{error}</span><button type="button" onClick={clearError}>Dismiss</button></div>}<div className="review-body">{isLoadingReview ? <div className="review-loading"><span className="loading-spinner" /> Loading review</div> : step === "review" ? <ReviewContent task={task} readOnly={readOnly} /> : <VerdictStep kind={step} task={task} />}</div></section>;
}
