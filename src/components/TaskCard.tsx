import { confirm } from "@tauri-apps/plugin-dialog";
import type { Task, TaskStatus } from "../types";

const STATUS_LABELS: Record<TaskStatus, string> = {
  draft: "Draft",
  running: "Running",
  awaiting_review: "Needs review",
  changes_requested: "Changes requested",
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
  const handleDelete = async (event: React.SyntheticEvent) => {
    event.stopPropagation();
    const ok = await confirm(`Delete "${task.title}"? This cannot be undone.`, { title: "Delete task", kind: "warning" });
    if (ok) await onDelete(task.id);
  };

  return (
    <div className={`task-row${active ? " active" : ""}`} role="button" tabIndex={0} onClick={onSelect} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(); } }}>
      <span className={`task-row-status task-row-status--${task.status}`} />
      <span className="task-row-copy"><span className="task-row-title">{task.title}</span><span className="task-row-meta">{STATUS_LABELS[task.status] ?? task.status}</span></span>
      <span className="task-row-actions"><span className="task-row-chevron">›</span><button type="button" className="task-row-delete" title="Delete task" onClick={handleDelete}>×</button></span>
    </div>
  );
}
