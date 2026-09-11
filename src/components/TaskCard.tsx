import { useEffect, useRef, useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import type { Task, TaskStatus } from "../types";

const STATUS_LABELS: Record<TaskStatus, string> = {
  draft: "Draft",
  running: "Running",
  awaiting_review: "Needs review",
  changes_requested: "Changes requested",
  ready: "Ready to run",
  approved: "Approved",
  failed: "Failed",
  cancelled: "Cancelled",
};

interface Props {
  task: Task;
  active?: boolean;
  onSelect: () => void;
  onDelete: (id: string) => Promise<void>;
}

export function TaskCard({ task, active = false, onSelect, onDelete }: Props) {
  const renameTask = useStore((state) => state.renameTask);
  const [isEditing, setIsEditing] = useState(false);
  const [title, setTitle] = useState(task.title);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { setTitle(task.title); }, [task.title]);
  useEffect(() => { if (isEditing) inputRef.current?.focus(); }, [isEditing]);

  const cancelRename = () => {
    setTitle(task.title);
    setIsEditing(false);
  };
  const saveRename = () => {
    const nextTitle = title.trim();
    if (!nextTitle || nextTitle === task.title) { cancelRename(); return; }
    setIsEditing(false);
    void renameTask(task.id, nextTitle).catch(() => setTitle(task.title));
  };
  const handleDelete = async (event: React.SyntheticEvent) => {
    event.stopPropagation();
    const ok = await confirm(`Delete "${task.title}"? This cannot be undone.`, { title: "Delete task", kind: "warning" });
    if (ok) await onDelete(task.id);
  };

  return (
    <div className={`task-row${active ? " active" : ""}`} role="button" tabIndex={0} onClick={() => { if (!isEditing) onSelect(); }} onKeyDown={(event) => { if (!isEditing && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); onSelect(); } }}>
      <span className={`task-row-status task-row-status--${task.status}`} />
      <span className="task-row-copy">
        {isEditing ? (
          <input ref={inputRef} className="task-rename-input" value={title} maxLength={200} aria-label="Task title" onClick={(event) => event.stopPropagation()} onBlur={cancelRename} onChange={(event) => setTitle(event.target.value)} onKeyDown={(event) => { event.stopPropagation(); if (event.key === "Enter") { event.preventDefault(); saveRename(); } else if (event.key === "Escape") { event.preventDefault(); cancelRename(); } }} />
        ) : (
          <span className="task-row-title" onDoubleClick={(event) => { event.stopPropagation(); setIsEditing(true); }}>{task.title}<button type="button" className="task-row-rename" title="Rename task" aria-label={`Rename ${task.title}`} onClick={(event) => { event.stopPropagation(); setIsEditing(true); }}>✎</button></span>
        )}
        <span className="task-row-meta">{STATUS_LABELS[task.status] ?? task.status}</span>
      </span>
      <span className="task-row-actions"><span className="task-row-chevron">›</span><button type="button" className="task-row-delete" title="Delete task" onClick={handleDelete}>×</button></span>
    </div>
  );
}
