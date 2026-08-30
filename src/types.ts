// ---------------------------------------------------------------------------
// Domain types — mirror the Rust models (snake_case fields are renamed to
// camelCase by serde's `rename_all = "camelCase"` on the Rust structs).
// ---------------------------------------------------------------------------

export interface Project {
  id: string;
  rootPath: string;
  createdAt: string;
}

export type TaskStatus =
  | "draft"
  | "running"
  | "awaiting_review"
  | "changes_requested"
  | "approved"
  | "failed"
  | "cancelled";

export interface Task {
  id: string;
  projectId: string;
  title: string;
  prompt: string;
  fileRefs: string[];
  status: TaskStatus;
  baseRef: string | null;
  worktreePath: string | null;
  branchName: string | null;
  agentThreadId: string | null;
  diff: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface TaskEvent {
  taskId: string;
  turnId: string;
  eventType: string;
  raw: Record<string, unknown>;
  status: TaskStatus | null;
  diff: string | null;
  error: string | null;
}

// ---------------------------------------------------------------------------
// Command input shapes
// ---------------------------------------------------------------------------

export interface CreateTaskInput {
  title: string;
  prompt: string;
  fileRefs: string[];
}

export interface UpdateTaskInput {
  title?: string;
  prompt?: string;
  fileRefs?: string[];
}
