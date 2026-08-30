import { Fragment, useMemo, useState } from "react";
import { parseUnifiedDiff, type DiffFile, type DiffLine } from "../diff";
import { useStore } from "../store";
import type { AddReviewCommentInput, DiffSide, ReviewComment, Task } from "../types";

interface Anchor {
  filePath: string;
  lineNumber: number | null;
  side: DiffSide | null;
}

function commentsAt(comments: ReviewComment[], anchor: Anchor) {
  return comments.filter((comment) =>
    comment.filePath === anchor.filePath
    && comment.lineNumber === anchor.lineNumber
    && comment.side === anchor.side);
}

function filePath(file: DiffFile) {
  return file.newPath ?? file.oldPath ?? file.displayPath;
}

function anchorForLine(file: DiffFile, line: DiffLine, side: DiffSide): Anchor | null {
  const lineNumber = side === "old" ? line.oldLineNumber : line.newLineNumber;
  return lineNumber === null ? null : { filePath: filePath(file), lineNumber, side };
}

function CommentThread({
  comments,
  onResolve,
}: {
  comments: ReviewComment[];
  onResolve: (id: string) => Promise<void>;
}) {
  if (comments.length === 0) return null;
  return (
    <div className="review-comment-thread">
      {comments.map((comment) => (
        <article className={`review-comment${comment.resolved ? " resolved" : ""}`} key={comment.id}>
          <div><span>{comment.resolved ? "Resolved" : "Review comment"}</span><time>{new Date(comment.createdAt).toLocaleString()}</time></div>
          <p>{comment.body}</p>
          {!comment.resolved && <button type="button" onClick={() => { void onResolve(comment.id).catch(() => undefined); }}>Resolve</button>}
        </article>
      ))}
    </div>
  );
}

function DiffLineRow({
  file,
  line,
  comments,
  onComment,
  onResolve,
}: {
  file: DiffFile;
  line: DiffLine;
  comments: ReviewComment[];
  onComment: (anchor: Anchor) => void;
  onResolve: (id: string) => Promise<void>;
}) {
  const oldAnchor = anchorForLine(file, line, "old");
  const newAnchor = anchorForLine(file, line, "new");
  const anchored = [
    ...(oldAnchor ? commentsAt(comments, oldAnchor) : []),
    ...(newAnchor ? commentsAt(comments, newAnchor) : []),
  ];
  return (
    <Fragment>
      <div className={`diff-line diff-line--${line.kind}`}>
        <button type="button" className="diff-line-number" disabled={!oldAnchor} title={oldAnchor ? "Comment on old line" : undefined} onClick={() => oldAnchor && onComment(oldAnchor)}>{line.oldLineNumber ?? ""}</button>
        <button type="button" className="diff-line-number" disabled={!newAnchor} title={newAnchor ? "Comment on new line" : undefined} onClick={() => newAnchor && onComment(newAnchor)}>{line.newLineNumber ?? ""}</button>
        <span className="diff-line-marker">{line.kind === "addition" ? "+" : line.kind === "deletion" ? "−" : " "}</span>
        <code>{line.content || " "}</code>
      </div>
      {anchored.length > 0 && <CommentThread comments={anchored} onResolve={onResolve} />}
    </Fragment>
  );
}

function CommentComposer({
  anchor,
  onCancel,
  onSubmit,
}: {
  anchor: Anchor;
  onCancel: () => void;
  onSubmit: (input: AddReviewCommentInput) => Promise<void>;
}) {
  const [body, setBody] = useState("");
  const [saving, setSaving] = useState(false);
  const submit = async () => {
    if (!body.trim()) return;
    setSaving(true);
    try {
      await onSubmit({ ...anchor, body });
      onCancel();
    } catch {
      // Store actions retain the actionable backend error for the app banner.
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="review-comment-composer">
      <div>{anchor.lineNumber ? `${anchor.side} line ${anchor.lineNumber}` : "File comment"}</div>
      <textarea autoFocus value={body} onChange={(event) => setBody(event.target.value)} placeholder="Leave a clear, actionable comment…" />
      <div><button type="button" onClick={onCancel}>Cancel</button><button type="button" className="review-primary" disabled={saving || !body.trim()} onClick={() => void submit()}>{saving ? "Saving…" : "Add comment"}</button></div>
    </div>
  );
}

function ReviewContent({ task }: { task: Task }) {
  const { reviewComments, addReviewComment, resolveReviewComment, setReviewStep } = useStore();
  const files = useMemo(() => parseUnifiedDiff(task.diff ?? ""), [task.diff]);
  const comments = reviewComments[task.id] ?? [];
  const [anchor, setAnchor] = useState<Anchor | null>(null);
  const additions = files.reduce((total, file) => total + file.additions, 0);
  const deletions = files.reduce((total, file) => total + file.deletions, 0);
  const unresolved = comments.filter((comment) => !comment.resolved).length;

  return (
    <>
      <div className="review-summary">
        <span>{files.length} {files.length === 1 ? "file" : "files"}</span>
        <span className="review-additions">+{additions}</span>
        <span className="review-deletions">−{deletions}</span>
        <span>{unresolved} unresolved</span>
      </div>
      {files.length === 0 ? (
        <div className="review-empty"><strong>No changes to display</strong><span>The task completed without a cumulative diff.</span></div>
      ) : files.map((file) => {
        const path = filePath(file);
        const fileComments = comments.filter((comment) => comment.filePath === path && comment.lineNumber === null);
        return (
          <details className="diff-file" open key={file.id}>
            <summary>
              <span className={`diff-file-status diff-file-status--${file.status}`}>{file.status}</span>
              <code>{file.displayPath}</code>
              {file.status === "renamed" && <small>{file.oldPath} → {file.newPath}</small>}
              <span className="diff-file-totals"><b>+{file.additions}</b><i>−{file.deletions}</i></span>
            </summary>
            <div className="diff-file-toolbar"><button type="button" onClick={() => setAnchor({ filePath: path, lineNumber: null, side: null })}>＋ File comment</button></div>
            {fileComments.length > 0 && <CommentThread comments={fileComments} onResolve={(id) => resolveReviewComment(task.id, id)} />}
            {anchor?.filePath === path && anchor.lineNumber === null && <CommentComposer anchor={anchor} onCancel={() => setAnchor(null)} onSubmit={(input) => addReviewComment(task.id, input)} />}
            {file.binary ? <div className="diff-binary">Binary file changed</div> : file.hunks.map((hunk) => (
              <section className="diff-hunk" key={hunk.id}>
                <header>{hunk.header}</header>
                {hunk.lines.map((line) => {
                  const isActive = anchor?.filePath === path && (
                    (anchor.side === "old" && anchor.lineNumber === line.oldLineNumber)
                    || (anchor.side === "new" && anchor.lineNumber === line.newLineNumber)
                  );
                  return (
                    <Fragment key={line.id}>
                      <DiffLineRow file={file} line={line} comments={comments} onComment={setAnchor} onResolve={(id) => resolveReviewComment(task.id, id)} />
                      {isActive && anchor && <CommentComposer anchor={anchor} onCancel={() => setAnchor(null)} onSubmit={(input) => addReviewComment(task.id, input)} />}
                    </Fragment>
                  );
                })}
              </section>
            ))}
          </details>
        );
      })}
      <footer className="review-actions">
        <div><strong>{unresolved ? `${unresolved} unresolved comments` : "Review ready"}</strong><span>Comments stay anchored to this task turn.</span></div>
        <button type="button" onClick={() => setReviewStep("request_changes")}>Request changes</button>
        <button type="button" className="review-primary" disabled={unresolved > 0} title={unresolved > 0 ? "Resolve or submit comments before confirming" : undefined} onClick={() => setReviewStep("confirm")}>Confirm</button>
      </footer>
    </>
  );
}

function VerdictStep({ kind, task }: { kind: "confirm" | "request_changes"; task: Task }) {
  const {
    setReviewStep,
    reviewModal,
    reviewComments,
    requestChanges,
    confirmTask,
    isSubmittingReview,
    settings,
  } = useStore();
  const [reviewerNote, setReviewerNote] = useState("");
  const [merge, setMerge] = useState(settings?.mergeOnConfirm ?? false);
  const unresolved = reviewModal
    ? (reviewComments[reviewModal.taskId] ?? []).filter((comment) => !comment.resolved)
    : [];
  const requesting = kind === "request_changes";
  const canSubmit = requesting
    ? unresolved.length > 0 || reviewerNote.trim().length > 0
    : unresolved.length === 0;
  const submit = async () => {
    try {
      if (requesting) await requestChanges(task.id, reviewerNote);
      else await confirmTask(task.id, merge);
    } catch {
      // Store actions expose the backend's recovery guidance in the app banner.
    }
  };
  return (
    <div className="review-deferred-step">
      <span className="review-step-icon">{requesting ? "↻" : "✓"}</span>
      <h2>{requesting ? "Request changes" : "Confirm task"}</h2>
      <p>{requesting
        ? `${unresolved.length} unresolved comment${unresolved.length === 1 ? "" : "s"} will be sent in one follow-up turn on the existing Codex thread.`
        : "Tanoor will commit the reviewed work, remove its worktree, and mark the task approved."}</p>
      {requesting ? (
        <label className="review-verdict-field">
          <span>Additional reviewer note <small>optional when comments are unresolved</small></span>
          <textarea value={reviewerNote} onChange={(event) => setReviewerNote(event.target.value)} placeholder="Add context that is not tied to a specific diff line…" />
        </label>
      ) : (
        <label className="review-merge-option">
          <input type="checkbox" checked={merge} onChange={(event) => setMerge(event.target.checked)} />
          <span><strong>Merge into the current project branch</strong><small>Off by default. When off, the approved commit remains on <code>{task.branchName}</code>.</small></span>
        </label>
      )}
      <div className="review-boundary-note">{requesting
        ? "Submitted comments are marked resolved only after Codex starts. A launch failure keeps them available for retry."
        : merge
          ? "A merge conflict is aborted automatically; the task branch and worktree remain available for retry."
          : "The task branch is preserved after worktree cleanup so you can merge or inspect it later."}</div>
      <div className="review-deferred-actions">
        <button type="button" disabled={isSubmittingReview} onClick={() => setReviewStep("review")}>Back to review</button>
        <button type="button" className="review-primary" disabled={isSubmittingReview || !canSubmit} onClick={() => void submit()}>{isSubmittingReview ? (requesting ? "Starting…" : "Confirming…") : (requesting ? "Send feedback" : "Confirm task")}</button>
      </div>
    </div>
  );
}

export function DiffReviewDialog({ task }: { task: Task }) {
  const { reviewModal, closeReview, isLoadingReview, error, clearError } = useStore();
  if (!reviewModal || reviewModal.taskId !== task.id) return null;
  return (
    <div className="review-overlay" role="presentation" onClick={(event) => { if (event.target === event.currentTarget) closeReview(); }}>
      <section className="review-dialog" role="dialog" aria-modal="true" aria-label={`Review ${task.title}`}>
        <header className="review-header"><div><span>Task review</span><h1>{task.title}</h1></div><button type="button" aria-label="Close review" onClick={closeReview}>×</button></header>
        {error && <div className="review-dialog-error" role="alert"><span>{error}</span><button type="button" onClick={clearError}>Dismiss</button></div>}
        <div className="review-body">
          {isLoadingReview ? <div className="review-loading"><span className="loading-spinner" /> Loading review</div> : reviewModal.step === "review" ? <ReviewContent task={task} /> : <VerdictStep kind={reviewModal.step} task={task} />}
        </div>
      </section>
    </div>
  );
}
