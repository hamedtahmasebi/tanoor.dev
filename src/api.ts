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
  create_task: {
    args: {
      projectId: string;
      title: string;
      prompt: string;
      fileRefs: string[];
    };
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
  check_system_health: {
    args: undefined;
    result: SystemHealthStatus;
  };
  run_task: {
    args: { taskId: string };
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
      body: string;
    };
    result: ReviewComment;
  };
  resolve_review_comment: {
    args: { commentId: string };
    result: ReviewComment;
  };
  list_review_comments: {
    args: { taskId: string };
    result: ReviewComment[];
  };
  request_changes: {
    args: {
      taskId: string;
      reviewerNote: string | null;
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

  // --- Tasks ---

  createTask: (projectId: string, input: CreateTaskInput): Promise<Task> =>
    invokeCommand("create_task", {
      projectId,
      title: input.title,
      prompt: input.prompt,
      fileRefs: input.fileRefs,
    }),

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

  checkSystemHealth: (): Promise<SystemHealthStatus> =>
    invokeCommand("check_system_health", undefined),

  runTask: (taskId: string): Promise<Task> =>
    invokeCommand("run_task", { taskId }),

  cancelTask: (taskId: string): Promise<Task> =>
    invokeCommand("cancel_task", { taskId }),

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
      body: input.body,
    }),

  resolveReviewComment: (commentId: string): Promise<ReviewComment> =>
    invokeCommand("resolve_review_comment", { commentId }),

  listReviewComments: (taskId: string): Promise<ReviewComment[]> =>
    invokeCommand("list_review_comments", { taskId }),

  requestChanges: (taskId: string, reviewerNote?: string): Promise<Task> =>
    invokeCommand("request_changes", {
      taskId,
      reviewerNote: reviewerNote?.trim() || null,
    }),

  confirmTask: (taskId: string, merge: boolean): Promise<Task> =>
    invokeCommand("confirm_task", { taskId, merge }),

  listTaskTurns: (taskId: string): Promise<TaskTurn[]> =>
    invokeCommand("list_task_turns", { taskId }),

  getTurnOutput: (turnLogPath: string): Promise<string[]> =>
    invokeCommand("get_turn_output", { turnLogPath }),
};
