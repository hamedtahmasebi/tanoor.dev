import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "./store";
import { TaskCard } from "./components/TaskCard";
import { NewTaskDialog } from "./components/NewTaskDialog";
import { DiffReviewDialog } from "./components/DiffReview";
import { SettingsDialog } from "./components/SettingsDialog";
import { TaskFollowUpPanel } from "./components/TaskFollowUpPanel";
import type { CreateTaskInput, TaskEvent } from "./types";

type NavFilter = "all" | "needs_review" | "in_progress" | "approved";

const NAV_ITEMS: { label: string; icon: string; filter: NavFilter }[] = [
  { label: "All tasks", icon: "▦", filter: "all" },
  { label: "Needs review", icon: "◌", filter: "needs_review" },
  { label: "In progress", icon: "↻", filter: "in_progress" },
  { label: "Approved", icon: "✓", filter: "approved" },
];

const NOOP = () => undefined;

function projectDisplayName(rootPath: string): string {
  const parts = rootPath.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts[parts.length - 1] ?? rootPath;
}


function App() {
  const {
    projects,
    currentProjectId,
    tasks,
    isLoadingProjects,
    isLoadingTasks,
    error,
    addProject,
    switchProject,
    createTask,
    deleteTask,
    runTask,
    cancelTask,
    appendTaskEvent,
    taskEvents,
    reviewModal,
    openReview,
    closeReview,
    clearError,
    systemHealth,
    isSettingsOpen,
    openSettings,
    closeSettings,
  } = useStore();

  const [showProjectDropdown, setShowProjectDropdown] = useState(false);
  const [navFilter, setNavFilter] = useState<NavFilter>("all");
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [showCommandPalette, setShowCommandPalette] = useState(false);

  const currentProject = projects.find((project) => project.id === currentProjectId) ?? null;
  const selectedTask = tasks.find((task) => task.id === selectedTaskId) ?? null;

  useEffect(() => {
    void useStore.getState().initApp();
  }, []);

  useEffect(() => {
    if (selectedTaskId && !tasks.some((task) => task.id === selectedTaskId)) setSelectedTaskId(null);
  }, [selectedTaskId, tasks]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void listen<TaskEvent>("forge:task-event", (event) => {
      appendTaskEvent(event.payload);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [appendTaskEvent]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey;
      if (modifier && event.key.toLowerCase() === "p") {
        event.preventDefault();
        setShowCommandPalette(true);
      }
      if (modifier && event.key.toLowerCase() === "n") {
        event.preventDefault();
        setSelectedTaskId(null);
      }
      if (event.key === "Escape") {
        setShowCommandPalette(false);
        setShowProjectDropdown(false);
        closeReview();
        closeSettings();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [closeReview, closeSettings]);

  const counts: Record<NavFilter, number> = {
    all: tasks.length,
    needs_review: tasks.filter((task) => ["awaiting_review", "changes_requested"].includes(task.status)).length,
    in_progress: tasks.filter((task) => task.status === "running").length,
    approved: tasks.filter((task) => task.status === "approved").length,
  };

  const filteredTasks = useMemo(() => tasks.filter((task) => {
    if (navFilter === "needs_review") return ["awaiting_review", "changes_requested"].includes(task.status);
    if (navFilter === "in_progress") return task.status === "running";
    if (navFilter === "approved") return task.status === "approved";
    return true;
  }), [navFilter, tasks]);

  const handleAddProject = async () => {
    setShowProjectDropdown(false);
    const result = await openDialog({ directory: true, multiple: false, title: "Open git project" });
    if (typeof result === "string") await addProject(result);
  };

  const handleCreateTask = async (input: CreateTaskInput) => {
    if (!currentProjectId) return;
    const task = await createTask(currentProjectId, input);
    setSelectedTaskId(task.id);
  };

  const handleRunTask = async () => {
    if (selectedTask) await runTask(selectedTask.id);
  };

  const handleCancelTask = async () => {
    if (selectedTask) await cancelTask(selectedTask.id);
  };

  return (
    <div className="app-shell">
      <aside className="activity-bar" aria-label="Tanoor navigation">
        <div className="tanoor-mark" aria-label="Tanoor"><span>T</span></div>
        <div className="activity-actions">
          <button className="activity-button active" type="button" title="Tasks" aria-label="Tasks">⌁</button>
          <button className="activity-button" type="button" title="Changes" aria-label="Changes" onClick={() => { const reviewTask = tasks.find((task) => ["awaiting_review", "changes_requested"].includes(task.status)); if (reviewTask) { setSelectedTaskId(reviewTask.id); void openReview(reviewTask.id); } }}>⌘</button>
          <button className="activity-button" type="button" title="Search" aria-label="Search" onClick={() => setShowCommandPalette(true)}>⌕</button>
        </div>
        <button className={`activity-button activity-settings${isSettingsOpen ? " active" : ""}`} type="button" title="Settings" aria-label="Settings" onClick={() => { closeReview(); openSettings(); }}>⚙</button>
      </aside>

      <aside className="workspace-sidebar">
        <div className="workspace-header">
          <button className="project-menu" type="button" aria-haspopup="listbox" aria-expanded={showProjectDropdown} onClick={() => setShowProjectDropdown((value) => !value)}>
            <span className="project-glyph">⌂</span>
            <span className="project-menu-copy">
              <span className="project-menu-name">{currentProject ? projectDisplayName(currentProject.rootPath) : "No project"}</span>
              <span className="project-menu-path">{currentProject?.rootPath ?? "Choose a git repository"}</span>
            </span>
            <span className="project-menu-chevron">⌄</span>
          </button>
          {showProjectDropdown && (
            <div className="project-dropdown" role="listbox">
              {projects.map((project) => (
                <button key={project.id} className={`project-dropdown-item${project.id === currentProjectId ? " active" : ""}`} type="button" role="option" aria-selected={project.id === currentProjectId} onClick={() => { void switchProject(project.id); setSelectedTaskId(null); setShowProjectDropdown(false); }}>
                  <span>{projectDisplayName(project.rootPath)}</span><small>{project.rootPath}</small>
                </button>
              ))}
              <button className="project-dropdown-add" type="button" onClick={handleAddProject}>＋ Open project…</button>
            </div>
          )}
        </div>

        <nav className="workspace-nav" aria-label="Task filters">
          {NAV_ITEMS.map((item) => (
            <button key={item.filter} className={`workspace-nav-item${navFilter === item.filter ? " active" : ""}`} type="button" onClick={() => setNavFilter(item.filter)}>
              <span className="workspace-nav-icon" aria-hidden="true">{item.icon}</span><span>{item.label}</span><span className="workspace-nav-count">{counts[item.filter]}</span>
            </button>
          ))}
        </nav>

        <div className="task-tree-header"><span>Tasks</span><button type="button" title="New task" aria-label="New task" onClick={() => setSelectedTaskId(null)}>＋</button></div>
        <div className="task-tree" aria-label="Tasks">
          {isLoadingProjects || isLoadingTasks ? (
            <div className="sidebar-loading"><span className="loading-spinner" /> Loading</div>
          ) : filteredTasks.length === 0 ? (
            <button className="sidebar-empty" type="button" onClick={() => setSelectedTaskId(null)}><span className="sidebar-empty-plus">＋</span><span>{currentProject ? "Create your first task" : "Open a project to begin"}</span></button>
          ) : (
            filteredTasks.map((task) => <TaskCard key={task.id} task={task} active={task.id === selectedTaskId} onSelect={() => setSelectedTaskId(task.id)} onDelete={deleteTask} />)
          )}
        </div>

        <div className="workspace-sidebar-footer">
          <button className="connection-row" type="button" onClick={openSettings}><span className={`connection-dot${systemHealth?.codex.binaryFound ? " online" : ""}`} /><span>Codex</span><span className="connection-state">{systemHealth?.codex.authStatus === "authenticated" ? "ready" : systemHealth?.codex.binaryFound ? "auth needed" : "offline"}</span></button>
          <div className="shortcut-row"><span>Command palette</span><kbd>⌘ P</kbd></div>
        </div>
      </aside>

      <main className="editor-panel">
        <header className="editor-header">
          <div className="editor-tab"><span className="tab-dot" /><span>{selectedTask ? selectedTask.title : "New task"}</span><span className="tab-close">×</span></div>
          <div className="editor-header-actions"><button type="button" className="editor-action" onClick={() => setShowCommandPalette(true)}><span>⌘ P</span> Command palette</button><button type="button" className="editor-icon-button" title="More actions" aria-label="More actions">•••</button></div>
        </header>

        {error && <div className="error-banner" role="alert"><span>{error}</span><button type="button" onClick={clearError}>Dismiss</button></div>}

        {!currentProject ? (
          <section className="welcome-pane"><div className="welcome-symbol">⌘</div><h1>Open a project to start</h1><p>Tanoor keeps your tasks close to the code. Pick a git repository, then describe the next change in the editor.</p><button className="quiet-button" type="button" onClick={handleAddProject}>Open git project <span>⌘ O</span></button></section>
        ) : selectedTask ? (
          <section className="task-detail-pane">
            <TaskFollowUpPanel
              task={selectedTask}
              onRunTask={() => void handleRunTask()}
              onCancelTask={() => void handleCancelTask()}
              onOpenReview={() => void openReview(selectedTask.id)}
            />
          </section>
        ) : (
          <NewTaskDialog project={currentProject} onClose={NOOP} onCreate={handleCreateTask} />
        )}

        <footer className="status-bar"><span className="status-branch">⑂ main</span><span>workspace-write</span><span className="status-spacer" /><span>{currentProject ? projectDisplayName(currentProject.rootPath) : "No workspace"}</span><span>UTF-8</span></footer>
      </main>

      {/* Floating execution dock shown only when no task is open in the main panel */}
      {selectedTask === null && tasks.some((t) => t.status === "running") && (() => {
        const runningTask = tasks.find((t) => t.status === "running");
        if (!runningTask) return null;
        const runEvents = taskEvents[runningTask.id] ?? [];
        return (
          <aside className="execution-dock" aria-label="Task execution">
            <div className="execution-dock-heading">
              <span>{runningTask.title}</span>
              <span className="execution-state execution-state--running">running</span>
            </div>
            {runEvents.length > 0 && (
              <div className="run-output" aria-label="Run output">
                {runEvents.slice(-5).map((event, index) => (
                  <div className="run-output-line" key={event.turnId + "-" + index}>
                    <span>{event.eventType}</span>
                    {event.error && <small>{event.error}</small>}
                  </div>
                ))}
              </div>
            )}
          </aside>
        );
      })()}

      {reviewModal && (() => { const task = tasks.find((item) => item.id === reviewModal.taskId); return task ? <DiffReviewDialog task={task} /> : null; })()}

      {isSettingsOpen && <SettingsDialog />}

      {showCommandPalette && <div className="command-overlay" role="presentation" onClick={(event) => { if (event.target === event.currentTarget) setShowCommandPalette(false); }}><div className="command-palette" role="dialog" aria-modal="true" aria-label="Command palette"><div className="command-input-row"><span>⌕</span><input autoFocus placeholder="Search commands…" onKeyDown={(event) => { if (event.key === "Escape") setShowCommandPalette(false); }} /></div><div className="command-group-label">Suggestions</div><button type="button" className="command-item" onClick={() => { setSelectedTaskId(null); setShowCommandPalette(false); }}><span className="command-item-icon">＋</span><span>New task</span><kbd>⌘ N</kbd></button><button type="button" className="command-item" onClick={() => setShowCommandPalette(false)}><span className="command-item-icon">⌁</span><span>Compact conversation</span><kbd>/ compact</kbd></button><button type="button" className="command-item" onClick={() => setShowCommandPalette(false)}><span className="command-item-icon">◈</span><span>Change model</span><kbd>/ model</kbd></button><button type="button" className="command-item" onClick={handleAddProject}><span className="command-item-icon">⌂</span><span>Open project</span><kbd>⌘ O</kbd></button></div></div>}
    </div>
  );
}

export default App;
