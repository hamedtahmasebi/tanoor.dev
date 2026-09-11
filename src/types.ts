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
  | "ready"
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
  agentId: string | null;
  agentModel: string | null;
  agentEffort: string | null;
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
  agentId: string | null;
  agentModel: string | null;
  agentEffort: string | null;
  agentThreadId: string | null;
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
  agents: CodexHealthStatus[];
}

export interface AppSettings {
  codexBin: string;
  gitBin: string;
  maxConcurrentTasks: number;
  mergeOnConfirm: boolean;
  sandboxMode: "workspace-write";
  codexModel: string;
  codexEffort: string;
  defaultAgent: string;
  claudeBin: string;
  claudeModel: string;
  opencodeBin: string;
  opencodeModel: string;
}

export type UpdateSettingsInput = Pick<
  AppSettings,
  "codexBin" | "gitBin" | "maxConcurrentTasks" | "mergeOnConfirm" | "codexModel" | "codexEffort"
  | "defaultAgent" | "claudeBin" | "claudeModel" | "opencodeBin" | "opencodeModel"
>;

// ---------------------------------------------------------------------------
// Agent model catalog — populated from the backend's static registry
// ---------------------------------------------------------------------------

export interface EffortLevel {
  id: string;
  description: string;
}

export interface ModelOption {
  id: string;
  label: string;
  description: string;
  /** Ordered list of effort levels this model supports. Empty = no effort control. */
  effortLevels: EffortLevel[];
  /** The effort id to pre-select when the user first picks this model. */
  defaultEffort: string;
}

export interface AgentModelCatalog {
  agentId: string;
  agentLabel: string;
  models: ModelOption[];
  selectedModel: string;
  selectedEffort: string;
  available: boolean;
  error: string | null;
  refreshedAt: string;
}

export type DiffSide = "old" | "new";

export interface ReviewComment {
  id: string;
  taskId: string;
  turnId: string;
  filePath: string;
  lineNumber: number | null;
  lineEndNumber: number | null;
  side: DiffSide | null;
  body: string;
  resolved: boolean;
  assignedAgentId: string | null;
  assignedModel: string | null;
  assignedEffort: string | null;
  createdAt: string;
}

export interface AgentSelection {
  agentId?: string | null;
  model?: string | null;
  effort?: string | null;
}

export interface AddReviewCommentInput {
  filePath: string;
  lineNumber: number | null;
  lineEndNumber: number | null;
  side: DiffSide | null;
  body: string;
}

// ---------------------------------------------------------------------------
// Command input shapes
// ---------------------------------------------------------------------------

export interface CreateTaskInput {
  title?: string;
  prompt: string;
  fileRefs: string[];
  agentId?: string;
  agentModel?: string;
  agentEffort?: string;
}

export interface EditorInfo {
  id: string;
  label: string;
  command: string;
}

export interface ProjectEntry {
  name: string;
  /** Path relative to the project root, using `/` separators. */
  path: string;
  isDir: boolean;
}

export interface UpdateTaskInput {
  title?: string;
  prompt?: string;
  fileRefs?: string[];
}
