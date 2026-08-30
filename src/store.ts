import { create } from "zustand";
import { api } from "./api";
import type { Project, Task, CreateTaskInput, UpdateTaskInput, TaskEvent } from "./types";

// ---------------------------------------------------------------------------
// Store shape
// ---------------------------------------------------------------------------

interface AppState {
  // --- Data ---
  projects: Project[];
  currentProjectId: string | null;
  tasks: Task[];
  taskEvents: Record<string, TaskEvent[]>;

  // --- Loading flags ---
  isLoadingProjects: boolean;
  isLoadingTasks: boolean;

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
  isLoadingProjects: false,
  isLoadingTasks: false,
  error: null,

  initApp: async () => {
    set({ isLoadingProjects: true, error: null });
    try {
      const projects = await api.listProjects();
      const currentProjectId = projects.length > 0 ? projects[0].id : null;
      set({ projects, currentProjectId, isLoadingProjects: false });
      if (currentProjectId) {
        await get().switchProject(currentProjectId);
      }
    } catch (e) {
      set({ error: String(e), isLoadingProjects: false });
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
      await api.cancelTask(taskId);
    } catch (e) {
      set({ error: String(e) });
      throw e;
    }
  },

  appendTaskEvent: (event: TaskEvent) => {
    set((s) => {
      const previous = s.taskEvents[event.taskId] ?? [];
      const next = [...previous, event].slice(-200);
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

  clearError: () => set({ error: null }),
}));
