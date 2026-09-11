import { create } from "zustand";
import { api } from "./api";
import type {
  AddReviewCommentInput,
  AgentModelCatalog,
  AppSettings,
  CreateTaskInput,
  Project,
  ReviewComment,
  Task,
  TaskEvent,
  TaskTurn,
  SystemHealthStatus,
  UpdateSettingsInput,
  UpdateTaskInput,
  AgentSelection,
  EditorInfo,
  ProjectEntry,
} from "./types";

export type ReviewStep = "review" | "confirm" | "request_changes";
export type DiffViewMode = "unified" | "split";


// ---------------------------------------------------------------------------
// Store shape
// ---------------------------------------------------------------------------

interface AppState {
  // --- Data ---
  projects: Project[];
  currentProjectId: string | null;
  tasks: Task[];
  taskEvents: Record<string, TaskEvent[]>;
  taskTurns: Record<string, TaskTurn[]>;
  taskOutput: Record<string, string[]>; // turnId -> log lines
  reviewComments: Record<string, ReviewComment[]>;
  reviewTaskId: string | null;
  reviewStep: ReviewStep;
  /** Comment the review screen should scroll to and highlight after opening. */
  reviewFocusCommentId: string | null;
  diffViewMode: DiffViewMode;
  editors: EditorInfo[];
  settings: AppSettings | null;
  systemHealth: SystemHealthStatus | null;
  isSettingsOpen: boolean;
  modelCatalog: AgentModelCatalog | null;
  agentCatalogs: AgentModelCatalog[];

  // --- Loading flags ---
  isLoadingProjects: boolean;
  isLoadingTasks: boolean;
  isLoadingReview: boolean;
  isSubmittingReview: boolean;
  isLoadingSettings: boolean;
  isSavingSettings: boolean;
  isCheckingHealth: boolean;
  isLoadingTaskOutput: boolean;

  // --- Error ---
  error: string | null;

  // --- Actions ---
  initApp: () => Promise<void>;
  addProject: (rootPath: string) => Promise<void>;
  switchProject: (projectId: string) => Promise<void>;
  loadProjectDir: (projectId: string, path: string | null) => Promise<ProjectEntry[]>;
  searchProjectFiles: (projectId: string, query: string) => Promise<string[]>;
  createTask: (projectId: string, input: CreateTaskInput) => Promise<Task>;
  deleteTask: (taskId: string) => Promise<void>;
  updateTask: (taskId: string, input: UpdateTaskInput) => Promise<void>;
  renameTask: (taskId: string, title: string) => Promise<void>;
  openInEditor: (taskId: string, editorId: string) => Promise<void>;
  runTask: (taskId: string, selection?: AgentSelection) => Promise<void>;
  retryTask: (taskId: string, selection?: AgentSelection) => Promise<void>;
  cancelTask: (taskId: string) => Promise<void>;
  appendTaskEvent: (event: TaskEvent) => void;
  loadTaskOutput: (taskId: string) => Promise<void>;
  openReview: (taskId: string, focusCommentId?: string | null) => Promise<void>;
  setReviewStep: (step: ReviewStep) => void;
  clearReviewFocus: () => void;
  setDiffViewMode: (mode: DiffViewMode) => void;
  closeReview: () => void;
  /** `silent` skips the loading flag and error banner, for background refreshes. */
  loadReviewComments: (taskId: string, options?: { silent?: boolean }) => Promise<void>;
  addReviewComment: (taskId: string, input: AddReviewCommentInput) => Promise<void>;
  resolveReviewComment: (taskId: string, commentId: string) => Promise<void>;
  assignReviewComment: (taskId: string, commentId: string, agentId: string | null, model: string | null, effort: string | null) => Promise<void>;
  submitReview: (taskId: string, reviewerNote: string, selection?: AgentSelection) => Promise<void>;
  confirmTask: (taskId: string, merge: boolean) => Promise<void>;
  openSettings: () => void;
  closeSettings: () => void;
  saveSettings: (input: UpdateSettingsInput) => Promise<void>;
  /** Loads the model catalog from the backend. Call before opening the inline picker. */
  openModelDialog: () => Promise<void>;
  saveModelSelection: (agentId: string, modelId: string, effortId: string) => Promise<void>;
  refreshSystemHealth: () => Promise<void>;
  clearError: () => void;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export const useStore = create<AppState>((set, get) => ({
  projects: [],
  currentProjectId: null,
  tasks: [],
  taskEvents: {},
  taskTurns: {},
  taskOutput: {},
  reviewComments: {},
  reviewTaskId: null,
  reviewStep: "review",
  reviewFocusCommentId: null,
  diffViewMode: "unified",
  editors: [],
  settings: null,
  systemHealth: null,
  isSettingsOpen: false,
  modelCatalog: null,
  agentCatalogs: [],
  isLoadingProjects: false,
  isLoadingTasks: false,
  isLoadingReview: false,
  isSubmittingReview: false,
  isLoadingSettings: false,
  isSavingSettings: false,
  isCheckingHealth: false,
  isLoadingTaskOutput: false,
  error: null,

  initApp: async () => {
    set({ isLoadingProjects: true, isLoadingSettings: true, error: null });
    try {
      const [projects, settings] = await Promise.all([
        api.listProjects(),
        api.getSettings(),
      ]);
      const currentProjectId = projects.length > 0 ? projects[0].id : null;
      set({ projects, settings, currentProjectId, isLoadingProjects: false, isLoadingSettings: false });
      void get().refreshSystemHealth();
      void api.listEditors().then((editors) => set({ editors })).catch(() => undefined);
      if (currentProjectId) {
        await get().switchProject(currentProjectId);
      }
    } catch (e) {
      set({ error: String(e), isLoadingProjects: false, isLoadingSettings: false });
    }
  },

  addProject: async (rootPath: string) => {
    set({ error: null });
    try {
      const project = await api.createProject(rootPath);
      set((s) => ({
        projects: [project, ...s.projects],
        currentProjectId: project.id,
        tasks: [],
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  switchProject: async (projectId: string) => {
    set({ currentProjectId: projectId, isLoadingTasks: true, tasks: [], error: null });
    try {
      const tasks = await api.listTasks(projectId);
      set({ tasks, isLoadingTasks: false });
    } catch (e) {
      set({ error: String(e), isLoadingTasks: false });
    }
  },

  loadProjectDir: async (projectId: string, path: string | null) => {
    try {
      return await api.listProjectDir(projectId, path);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  searchProjectFiles: async (projectId: string, query: string) => {
    try {
      return await api.searchProjectFiles(projectId, query);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  createTask: async (projectId: string, input: CreateTaskInput) => {
    set({ error: null });
    try {
      const task = await api.createTask(projectId, input);
      set((s) => ({ tasks: [task, ...s.tasks] }));
      return task;
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  deleteTask: async (taskId: string) => {
    set({ error: null });
    try {
      await api.deleteTask(taskId);
      set((s) => ({ tasks: s.tasks.filter((t) => t.id !== taskId) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  updateTask: async (taskId: string, input: UpdateTaskInput) => {
    set({ error: null });
    try {
      const updated = await api.updateTaskPrompt(taskId, input);
      set((s) => ({
        tasks: s.tasks.map((t) => (t.id === taskId ? updated : t)),
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  renameTask: async (taskId: string, title: string) => {
    set({ error: null });
    try {
      const task = await api.renameTask(taskId, title);
      set((s) => ({ tasks: s.tasks.map((item) => item.id === task.id ? task : item) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  openInEditor: async (taskId: string, editorId: string) => {
    set({ error: null });
    try {
      const task = await api.openWorktreeInEditor(taskId, editorId);
      set((s) => ({ tasks: s.tasks.map((item) => item.id === task.id ? task : item) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  runTask: async (taskId: string, selection?: AgentSelection) => {
    set({ error: null });
    try {
      const task = await api.runTask(taskId, selection);
      set((s) => ({ tasks: s.tasks.map((item) => item.id === task.id ? task : item) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  retryTask: async (taskId: string, selection?: AgentSelection) => {
    set({ error: null });
    try {
      const task = await api.retryTask(taskId, selection);
      set((s) => ({ tasks: s.tasks.map((item) => item.id === task.id ? task : item) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  cancelTask: async (taskId: string) => {
    set({ error: null });
    try {
      const task = await api.cancelTask(taskId);
      // Update the task in state immediately. For a live cancel the background
      // thread will emit a final status event that overwrites this; for a
      // dangling task this is the only update that will arrive.
      set((s) => ({ tasks: s.tasks.map((t) => (t.id === task.id ? task : t)) }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  appendTaskEvent: (event: TaskEvent) => {
    set((s) => {
      if (event.eventType === "forge.task.named") {
        const title = typeof event.raw.title === "string" ? event.raw.title : null;
        const branchName = typeof event.raw.branchName === "string" ? event.raw.branchName : null;
        return {
          tasks: s.tasks.map((task) => task.id === event.taskId ? {
            ...task,
            ...(title ? { title } : {}),
            ...(branchName ? { branchName } : {}),
          } : task),
        };
      }
      const previous = s.taskEvents[event.taskId] ?? [];
      const next = [...previous, event].slice(-500);
      const tasks = event.status
        ? s.tasks.map((task) => task.id === event.taskId ? {
          ...task,
          status: event.status!,
          diff: event.diff ?? task.diff,
        } : task)
        : s.tasks;
      return { taskEvents: { ...s.taskEvents, [event.taskId]: next }, tasks };
    });
  },

  loadTaskOutput: async (taskId: string) => {
    set({ isLoadingTaskOutput: true });
    try {
      const turns = await api.listTaskTurns(taskId);
      // Fetch log lines for each turn in parallel
      const outputEntries = await Promise.all(
        turns.map(async (turn) => {
          const lines = await api.getTurnOutput(turn.logPath);
          return { turnId: turn.id, lines };
        }),
      );
      const taskOutput: Record<string, string[]> = {};
      for (const { turnId, lines } of outputEntries) {
        taskOutput[turnId] = lines;
      }
      set((s) => ({
        taskTurns: { ...s.taskTurns, [taskId]: turns },
        taskOutput: { ...s.taskOutput, ...taskOutput },
        isLoadingTaskOutput: false,
      }));
    } catch (e) {
      set({ error: String(e), isLoadingTaskOutput: false });
    }
  },

  openReview: async (taskId: string, focusCommentId?: string | null) => {
    set({ reviewTaskId: taskId, reviewStep: "review", reviewFocusCommentId: focusCommentId ?? null });
    await get().openModelDialog();
    try {
      await get().loadReviewComments(taskId);
    } catch {
      // The loader already exposes the error in store state. Keeping the
      // dialog open lets the user dismiss it or retry without an unhandled
      // promise from click handlers.
    }
  },

  setReviewStep: (step: ReviewStep) => set({ reviewStep: step }),

  clearReviewFocus: () => set({ reviewFocusCommentId: null }),

  setDiffViewMode: (mode: DiffViewMode) => set({ diffViewMode: mode }),

  closeReview: () => set({ reviewTaskId: null, reviewFocusCommentId: null }),

  loadReviewComments: async (taskId: string, options?: { silent?: boolean }) => {
    const silent = options?.silent ?? false;
    if (!silent) set({ isLoadingReview: true, error: null });
    try {
      const comments = await api.listReviewComments(taskId);
      set((state) => ({
        reviewComments: { ...state.reviewComments, [taskId]: comments },
        ...(silent ? {} : { isLoadingReview: false }),
      }));
    } catch (e) {
      // A background refresh keeps the current view instead of hijacking the
      // error banner; the review screen reports the failure when opened.
      if (silent) return;
      set({ error: String(e), isLoadingReview: false });
      throw e;
    }
  },

  addReviewComment: async (taskId: string, input: AddReviewCommentInput) => {
    set({ error: null });
    try {
      const comment = await api.addReviewComment(taskId, input);
      set((state) => ({
        reviewComments: {
          ...state.reviewComments,
          [taskId]: [...(state.reviewComments[taskId] ?? []), comment],
        },
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  resolveReviewComment: async (taskId: string, commentId: string) => {
    set({ error: null });
    try {
      const updated = await api.resolveReviewComment(commentId);
      set((state) => ({
        reviewComments: {
          ...state.reviewComments,
          [taskId]: (state.reviewComments[taskId] ?? []).map((comment) =>
            comment.id === updated.id ? updated : comment),
        },
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  assignReviewComment: async (taskId, commentId, agentId, model, effort) => {
    set({ error: null });
    try {
      const updated = await api.assignReviewComment(commentId, agentId, model, effort);
      set((state) => ({
        reviewComments: {
          ...state.reviewComments,
          [taskId]: (state.reviewComments[taskId] ?? []).map((comment) =>
            comment.id === updated.id ? updated : comment),
        },
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  submitReview: async (taskId: string, reviewerNote: string, selection?: AgentSelection) => {
    set({ isSubmittingReview: true, error: null });
    try {
      const task = await api.submitReview(taskId, reviewerNote, selection);
      set((state) => ({
        tasks: state.tasks.map((item) => item.id === task.id ? task : item),
        reviewTaskId: null,
        isSubmittingReview: false,
      }));
    } catch (e) {
      set({ error: String(e), isSubmittingReview: false });
      throw e;
    }
  },

  confirmTask: async (taskId: string, merge: boolean) => {
    set({ isSubmittingReview: true, error: null });
    try {
      const task = await api.confirmTask(taskId, merge);
      set((state) => ({
        tasks: state.tasks.map((item) => item.id === task.id ? task : item),
        reviewTaskId: null,
        isSubmittingReview: false,
      }));
    } catch (e) {
      set({ error: String(e), isSubmittingReview: false });
      throw e;
    }
  },

  openSettings: () => {
    set({ isSettingsOpen: true, error: null });
    void get().refreshSystemHealth();
  },

  closeSettings: () => set({ isSettingsOpen: false }),

  openModelDialog: async () => {
    set({ error: null });
    try {
      const agentCatalogs = await api.listAgentCatalogs();
      const selectedAgent = get().settings?.defaultAgent ?? "codex";
      set({
        agentCatalogs,
        modelCatalog: agentCatalogs.find((catalog) => catalog.agentId === selectedAgent)
          ?? agentCatalogs[0]
          ?? null,
      });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  saveModelSelection: async (agentId: string, modelId: string, effortId: string) => {
    const { settings } = get();
    if (!settings) return;
    try {
      const updated = await api.updateSettings({
        ...settings,
        ...(agentId === "codex" ? { codexModel: modelId, codexEffort: effortId } : {}),
        ...(agentId === "claude" ? { claudeModel: modelId } : {}),
        ...(agentId === "opencode" ? { opencodeModel: modelId } : {}),
        defaultAgent: agentId,
      });
      set((state) => ({
        settings: updated,
        modelCatalog: state.agentCatalogs.find((catalog) => catalog.agentId === agentId) ?? state.modelCatalog,
      }));
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  saveSettings: async (input: UpdateSettingsInput) => {
    set({ isSavingSettings: true, error: null });
    try {
      const settings = await api.updateSettings(input);
      set({ settings, isSavingSettings: false });
      await get().refreshSystemHealth();
    } catch (e) {
      set({ error: String(e), isSavingSettings: false });
      throw e;
    }
  },

  refreshSystemHealth: async () => {
    set({ isCheckingHealth: true });
    try {
      const systemHealth = await api.checkSystemHealth();
      set({ systemHealth, isCheckingHealth: false });
    } catch (e) {
      set({ error: String(e), isCheckingHealth: false });
    }
  },

  clearError: () => set({ error: null }),
}));
