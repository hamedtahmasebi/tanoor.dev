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

export interface TaskTurn {
  id: string;
  taskId: string;
  kind: string;
  prompt: string;
  status: string;
  logPath: string;
  startedAt: string;
  endedAt: string | null;
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

export interface CodexHealthStatus {
  binaryFound: boolean;
  version: string | null;
  authEnvPresent: boolean;
  authStatus: "authenticated" | "not_authenticated" | "unknown";
  authDetail: string | null;
  detail: string | null;
}

export interface BinaryHealthStatus {
  binaryFound: boolean;
  version: string | null;
  detail: string | null;
}

export interface SystemHealthStatus {
  codex: CodexHealthStatus;
  git: BinaryHealthStatus;
}

export interface AppSettings {
  codexBin: string;
  gitBin: string;
  maxConcurrentTasks: number;
  mergeOnConfirm: boolean;
  sandboxMode: "workspace-write";
}

export type UpdateSettingsInput = Pick<
  AppSettings,
  "codexBin" | "gitBin" | "maxConcurrentTasks" | "mergeOnConfirm"
>;

export type DiffSide = "old" | "new";

export interface ReviewComment {
  id: string;
  taskId: string;
  turnId: string;
  filePath: string;
  lineNumber: number | null;
  side: DiffSide | null;
  body: string;
  resolved: boolean;
  createdAt: string;
}

export interface AddReviewCommentInput {
  filePath: string;
  lineNumber: number | null;
  side: DiffSide | null;
  body: string;
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
