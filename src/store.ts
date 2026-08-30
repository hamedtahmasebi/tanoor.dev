import { create } from "zustand";
import { api } from "./api";
import type {
  AddReviewCommentInput,
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
} from "./types";

export type ReviewStep = "review" | "confirm" | "request_changes";

export interface ReviewModalState {
  taskId: string;
  step: ReviewStep;
}

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
  reviewModal: ReviewModalState | null;
  settings: AppSettings | null;
  systemHealth: SystemHealthStatus | null;
  isSettingsOpen: boolean;

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
  createTask: (projectId: string, input: CreateTaskInput) => Promise<Task>;
  deleteTask: (taskId: string) => Promise<void>;
  updateTask: (taskId: string, input: UpdateTaskInput) => Promise<void>;
  runTask: (taskId: string) => Promise<void>;
  cancelTask: (taskId: string) => Promise<void>;
  appendTaskEvent: (event: TaskEvent) => void;
  loadTaskOutput: (taskId: string) => Promise<void>;
  openReview: (taskId: string) => Promise<void>;
  setReviewStep: (step: ReviewStep) => void;
  closeReview: () => void;
  loadReviewComments: (taskId: string) => Promise<void>;
  addReviewComment: (taskId: string, input: AddReviewCommentInput) => Promise<void>;
  resolveReviewComment: (taskId: string, commentId: string) => Promise<void>;
  requestChanges: (taskId: string, reviewerNote: string) => Promise<void>;
  confirmTask: (taskId: string, merge: boolean) => Promise<void>;
  openSettings: () => void;
  closeSettings: () => void;
  saveSettings: (input: UpdateSettingsInput) => Promise<void>;
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
  reviewModal: null,
  settings: null,
  systemHealth: null,
  isSettingsOpen: false,
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

  runTask: async (taskId: string) => {
    set({ error: null });
    try {
      const task = await api.runTask(taskId);
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

  openReview: async (taskId: string) => {
    set({ reviewModal: { taskId, step: "review" } });
    try {
      await get().loadReviewComments(taskId);
    } catch {
      // The loader already exposes the error in store state. Keeping the
      // dialog open lets the user dismiss it or retry without an unhandled
      // promise from click handlers.
    }
  },

  setReviewStep: (step: ReviewStep) => {
    set((state) => state.reviewModal
      ? { reviewModal: { ...state.reviewModal, step } }
      : state);
  },

  closeReview: () => set({ reviewModal: null }),

  loadReviewComments: async (taskId: string) => {
    set({ isLoadingReview: true, error: null });
    try {
      const comments = await api.listReviewComments(taskId);
      set((state) => ({
        reviewComments: { ...state.reviewComments, [taskId]: comments },
        isLoadingReview: false,
      }));
    } catch (e) {
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

  requestChanges: async (taskId: string, reviewerNote: string) => {
    set({ isSubmittingReview: true, error: null });
    try {
      const task = await api.requestChanges(taskId, reviewerNote);
      set((state) => ({
        tasks: state.tasks.map((item) => item.id === task.id ? task : item),
        reviewComments: {
          ...state.reviewComments,
          [taskId]: (state.reviewComments[taskId] ?? []).map((comment) =>
            comment.resolved ? comment : { ...comment, resolved: true }),
        },
        reviewModal: null,
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
        reviewModal: null,
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
