import { invoke } from "@tauri-apps/api/core";
import type { Project, Task, CreateTaskInput, UpdateTaskInput } from "./types";

// ---------------------------------------------------------------------------
// Typed wrappers around Tauri `invoke`.
//
// Tauri commands receive Rust parameter names. Return values use
// camelCase because the Rust structs are annotated with
// `#[serde(rename_all = "camelCase")]`.
// ---------------------------------------------------------------------------

export const api = {
  // --- Projects ---

  createProject: (rootPath: string): Promise<Project> =>
    invoke("create_project", { root_path: rootPath }),

  listProjects: (): Promise<Project[]> => invoke("list_projects"),

  // --- Tasks ---

  createTask: (projectId: string, input: CreateTaskInput): Promise<Task> =>
    invoke("create_task", {
      project_id: projectId,
      title: input.title,
      prompt: input.prompt,
      file_refs: input.fileRefs,
    }),

  listTasks: (projectId: string): Promise<Task[]> =>
    invoke("list_tasks", { project_id: projectId }),

  getTask: (taskId: string): Promise<Task> =>
    invoke("get_task", { task_id: taskId }),

  deleteTask: (taskId: string): Promise<void> =>
    invoke("delete_task", { task_id: taskId }),

  updateTaskPrompt: (taskId: string, input: UpdateTaskInput): Promise<Task> =>
    invoke("update_task_prompt", {
      task_id: taskId,
      title: input.title ?? null,
      prompt: input.prompt ?? null,
      file_refs: input.fileRefs ?? null,
    }),

  runTask: (taskId: string, codexBin?: string): Promise<Task> =>
    invoke("run_task", {
      task_id: taskId,
      codex_bin: codexBin ?? null,
    }),

  cancelTask: (taskId: string): Promise<void> =>
    invoke("cancel_task", { task_id: taskId }),
};
