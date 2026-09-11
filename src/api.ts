import { invoke } from "@tauri-apps/api/core";
import type { InvokeArgs } from "@tauri-apps/api/core";
import type {
  CodexHealthStatus,
  AppSettings,
  SystemHealthStatus,
  UpdateSettingsInput,
  CreateTaskInput,
  Project,
  ReviewComment,
  AddReviewCommentInput,
  Task,
  TaskTurn,
  UpdateTaskInput,
  AgentModelCatalog,
  AgentSelection,
  EditorInfo,
  ProjectEntry,
} from "./types";

// ---------------------------------------------------------------------------
// Typed wrappers around Tauri `invoke`.
//
// Tauri's command macro converts Rust parameter names to camelCase by default.
// Return values also use camelCase because the Rust structs are annotated with
// `#[serde(rename_all = "camelCase")]`.
// ---------------------------------------------------------------------------

type CommandContract = {
  create_project: {
    args: { rootPath: string };
    result: Project;
  };
  list_projects: {
    args: undefined;
    result: Project[];
  };
  list_project_dir: {
    args: { projectId: string; path: string | null };
    result: ProjectEntry[];
  };
  search_project_files: {
    args: { projectId: string; query: string };
    result: string[];
  };
  create_task: {
    args: {
      projectId: string;
      title: string | null;
      prompt: string;
      fileRefs: string[];
      agentId: string | null;
      agentModel: string | null;
      agentEffort: string | null;
    };
    result: Task;
  };
  rename_task: {
    args: { taskId: string; title: string };
    result: Task;
  };
  list_tasks: {
    args: { projectId: string };
    result: Task[];
  };
  get_task: {
    args: { taskId: string };
    result: Task;
  };
  delete_task: {
    args: { taskId: string };
    result: void;
  };
  update_task_prompt: {
    args: {
      taskId: string;
      title: string | null;
      prompt: string | null;
      fileRefs: string[] | null;
    };
    result: Task;
  };
  check_codex_health: {
    args: undefined;
    result: CodexHealthStatus;
  };
  get_settings: {
    args: undefined;
    result: AppSettings;
  };
  update_settings: {
    args: UpdateSettingsInput;
    result: AppSettings;
  };
  get_agent_models: {
    args: undefined;
    result: AgentModelCatalog;
  };
  list_agent_catalogs: {
    args: undefined;
    result: AgentModelCatalog[];
  };
  check_system_health: {
    args: undefined;
    result: SystemHealthStatus;
  };
  run_task: {
    args: { taskId: string; agentId: string | null; model: string | null; effort: string | null };
    result: Task;
  };
  retry_task: {
    args: { taskId: string; agentId: string | null; model: string | null; effort: string | null };
    result: Task;
  };
  cancel_task: {
    args: { taskId: string };
    result: Task;
  };
  add_review_comment: {
    args: {
      taskId: string;
      filePath: string;
      lineNumber: number | null;
      side: "old" | "new" | null;
      lineEndNumber: number | null;
      body: string;
    };
    result: ReviewComment;
  };
  resolve_review_comment: {
    args: { commentId: string };
    result: ReviewComment;
  };
  assign_review_comment: {
    args: { commentId: string; agentId: string | null; model: string | null; effort: string | null };
    result: ReviewComment;
  };
  list_review_comments: {
    args: { taskId: string };
    result: ReviewComment[];
  };
  list_editors: {
    args: undefined;
    result: EditorInfo[];
  };
  open_worktree_in_editor: {
    args: { taskId: string; editorId: string };
    result: Task;
  };
  submit_review: {
    args: {
      taskId: string;
      reviewerNote: string | null;
      agentId: string | null;
      model: string | null;
      effort: string | null;
    };
    result: Task;
  };
  confirm_task: {
    args: { taskId: string; merge: boolean };
    result: Task;
  };
  list_task_turns: {
    args: { taskId: string };
    result: TaskTurn[];
  };
  get_turn_output: {
    args: { turnLogPath: string };
    result: string[];
  };
};

const invokeCommand = <Name extends keyof CommandContract>(
  name: Name,
  args: CommandContract[Name]["args"],
): Promise<CommandContract[Name]["result"]> =>
  invoke(name, args as InvokeArgs | undefined);

export const api = {
  // --- Projects ---

  createProject: (rootPath: string): Promise<Project> =>
    invokeCommand("create_project", { rootPath }),

  listProjects: (): Promise<Project[]> =>
    invokeCommand("list_projects", undefined),

  listProjectDir: (projectId: string, path: string | null): Promise<ProjectEntry[]> =>
    invokeCommand("list_project_dir", { projectId, path }),

  searchProjectFiles: (projectId: string, query: string): Promise<string[]> =>
    invokeCommand("search_project_files", { projectId, query }),

  // --- Tasks ---

  createTask: (projectId: string, input: CreateTaskInput): Promise<Task> =>
    invokeCommand("create_task", {
      projectId,
      title: input.title ?? null,
      prompt: input.prompt,
      fileRefs: input.fileRefs,
      agentId: input.agentId ?? null,
      agentModel: input.agentModel ?? null,
      agentEffort: input.agentEffort ?? null,
    }),

  renameTask: (taskId: string, title: string): Promise<Task> =>
    invokeCommand("rename_task", { taskId, title }),

  listTasks: (projectId: string): Promise<Task[]> =>
    invokeCommand("list_tasks", { projectId }),

  getTask: (taskId: string): Promise<Task> =>
    invokeCommand("get_task", { taskId }),

  deleteTask: (taskId: string): Promise<void> =>
    invokeCommand("delete_task", { taskId }),

  updateTaskPrompt: (taskId: string, input: UpdateTaskInput): Promise<Task> =>
    invokeCommand("update_task_prompt", {
      taskId,
      title: input.title ?? null,
      prompt: input.prompt ?? null,
      fileRefs: input.fileRefs ?? null,
    }),

  // --- Agent runner ---

  checkCodexHealth: (): Promise<CodexHealthStatus> =>
    invokeCommand("check_codex_health", undefined),

  getSettings: (): Promise<AppSettings> =>
    invokeCommand("get_settings", undefined),

  updateSettings: (input: UpdateSettingsInput): Promise<AppSettings> =>
    invokeCommand("update_settings", input),

  getAgentModels: (): Promise<AgentModelCatalog> =>
    invokeCommand("get_agent_models", undefined),

  listAgentCatalogs: (): Promise<AgentModelCatalog[]> =>
    invokeCommand("list_agent_catalogs", undefined),

  checkSystemHealth: (): Promise<SystemHealthStatus> =>
    invokeCommand("check_system_health", undefined),

  runTask: (taskId: string, selection?: AgentSelection): Promise<Task> =>
    invokeCommand("run_task", {
      taskId,
      agentId: selection?.agentId ?? null,
      model: selection?.model ?? null,
      effort: selection?.effort ?? null,
    }),

  retryTask: (taskId: string, selection?: AgentSelection): Promise<Task> =>
    invokeCommand("retry_task", {
      taskId,
      agentId: selection?.agentId ?? null,
      model: selection?.model ?? null,
      effort: selection?.effort ?? null,
    }),

  cancelTask: (taskId: string): Promise<Task> =>
    invokeCommand("cancel_task", { taskId }),

  listEditors: (): Promise<EditorInfo[]> =>
    invokeCommand("list_editors", undefined),

  openWorktreeInEditor: (taskId: string, editorId: string): Promise<Task> =>
    invokeCommand("open_worktree_in_editor", { taskId, editorId }),

  // --- Review ---

  addReviewComment: (
    taskId: string,
    input: AddReviewCommentInput,
  ): Promise<ReviewComment> =>
    invokeCommand("add_review_comment", {
      taskId,
      filePath: input.filePath,
      lineNumber: input.lineNumber,
      side: input.side,
      lineEndNumber: input.lineEndNumber,
      body: input.body,
    }),

  resolveReviewComment: (commentId: string): Promise<ReviewComment> =>
    invokeCommand("resolve_review_comment", { commentId }),

  assignReviewComment: (commentId: string, agentId: string | null, model: string | null, effort: string | null): Promise<ReviewComment> =>
    invokeCommand("assign_review_comment", { commentId, agentId, model, effort }),

  listReviewComments: (taskId: string): Promise<ReviewComment[]> =>
    invokeCommand("list_review_comments", { taskId }),

  submitReview: (taskId: string, reviewerNote?: string, selection?: AgentSelection): Promise<Task> =>
    invokeCommand("submit_review", {
      taskId,
      reviewerNote: reviewerNote?.trim() || null,
      agentId: selection?.agentId ?? null,
      model: selection?.model ?? null,
      effort: selection?.effort ?? null,
    }),

  confirmTask: (taskId: string, merge: boolean): Promise<Task> =>
    invokeCommand("confirm_task", { taskId, merge }),

  listTaskTurns: (taskId: string): Promise<TaskTurn[]> =>
    invokeCommand("list_task_turns", { taskId }),

  getTurnOutput: (turnLogPath: string): Promise<string[]> =>
    invokeCommand("get_turn_output", { turnLogPath }),
};
