import { useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { readDir } from "@tauri-apps/plugin-fs";
import { useStore } from "../store";
import type { CreateTaskInput, EffortLevel, ModelOption, Project } from "../types";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type PaletteType = "reference" | "command";
interface PaletteState { type: PaletteType; query: string; start: number; end: number; }

// Two-step inline model picker: first pick a model, then pick effort (if any).
type PickerStep = "model" | "effort";
interface PickerState { step: PickerStep; }

interface Props {
  project: Project;
  onClose: () => void;
  onCreate: (input: CreateTaskInput) => Promise<void>;
  /** Called by App when an external trigger (command palette) wants to open the model picker. */
  onOpenModelPicker?: (open: () => void) => void;
}

const COMMANDS = [
  { name: "compact", label: "Compact conversation", hint: "Summarize the current context" },
  { name: "model", label: "Change model", hint: "Choose the model for this task" },
  { name: "context", label: "Add context", hint: "Attach another file or folder" },
];

function makeRelative(rootPath: string, absPath: string): string {
  const root = rootPath.replace(/\\/g, "/").replace(/\/+$/, "");
  const abs = absPath.replace(/\\/g, "/");
  if (abs.startsWith(`${root}/`)) return abs.slice(root.length + 1);
  if (abs === root) return ".";
  return abs;
}

// ---------------------------------------------------------------------------
// NewTaskDialog
// ---------------------------------------------------------------------------

export function NewTaskDialog({ project, onClose, onCreate, onOpenModelPicker }: Props) {
  const { settings, modelCatalog, openModelDialog } = useStore();

  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [fileRefs, setFileRefs] = useState<string[]>([]);
  const [projectFiles, setProjectFiles] = useState<string[]>([]);
  const [palette, setPalette] = useState<PaletteState | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Inline model picker state
  const [picker, setPicker] = useState<PickerState | null>(null);
  // Local selections while picker is open; committed on accept
  const [pickerModel, setPickerModel] = useState(settings?.codexModel ?? "");
  const [pickerEffort, setPickerEffort] = useState(settings?.codexEffort ?? "");
  // Which item has keyboard focus inside the picker
  const [focusedModel, setFocusedModel] = useState(0);
  const [focusedEffort, setFocusedEffort] = useState(0);
  // Set to true when picker was requested before the catalog loaded
  const pendingPickerOpen = useRef(false);

  const titleRef = useRef<HTMLInputElement>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const cursorRef = useRef(0);
  const modelChipRef = useRef<HTMLButtonElement>(null);
  const pickerRef = useRef<HTMLDivElement>(null);

  // -------------------------------------------------------------------------
  // Expose openPicker to parent (for command palette wiring)
  // -------------------------------------------------------------------------
  useEffect(() => {
    onOpenModelPicker?.(() => openPicker());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onOpenModelPicker]);

  // -------------------------------------------------------------------------
  // Project file collection
  // -------------------------------------------------------------------------
  useEffect(() => {
    titleRef.current?.focus();
    let cancelled = false;
    const collectFiles = async (directory: string, prefix = ""): Promise<string[]> => {
      const entries = await readDir(directory);
      const files: string[] = [];
      for (const entry of entries) {
        if (!entry.name) continue;
        const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
        if (relative === ".git" || relative.startsWith(".git/")) continue;
        const childPath = `${directory.replace(/[\\\\/]+$/, "")}${project.rootPath.includes("\\") ? "\\" : "/"}${entry.name}`;
        if (entry.isDirectory) files.push(...await collectFiles(childPath, relative));
        else if (entry.isFile) files.push(relative);
        if (files.length >= 200) break;
      }
      return files;
    };
    void collectFiles(project.rootPath).then((files) => {
      if (!cancelled) setProjectFiles(files.slice(0, 200).sort());
    }).catch(() => setProjectFiles([]));
    return () => { cancelled = true; };
  }, [project.rootPath]);

  // -------------------------------------------------------------------------
  // Escape handler
  // -------------------------------------------------------------------------
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (picker) { closePicker(); return; }
      if (palette) { setPalette(null); return; }
      onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose, palette, picker]);

  // -------------------------------------------------------------------------
  // Close picker when clicking outside
  // -------------------------------------------------------------------------
  useEffect(() => {
    if (!picker) return;
    const onPointerDown = (e: PointerEvent) => {
      if (
        pickerRef.current && !pickerRef.current.contains(e.target as Node) &&
        modelChipRef.current && !modelChipRef.current.contains(e.target as Node)
      ) {
        closePicker();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [picker]);

  // When the catalog loads and a picker open was pending, open it now
  useEffect(() => {
    if (modelCatalog && pendingPickerOpen.current) {
      pendingPickerOpen.current = false;
      openPicker();
    }
    // openPicker reads modelCatalog from closure; it will be fresh here
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modelCatalog]);

  // -------------------------------------------------------------------------
  // Inline model picker helpers
  // -------------------------------------------------------------------------

  const models = modelCatalog?.models ?? [];

  const openPicker = () => {
    // Ensure catalog is loaded; remember to open once it arrives
    if (!modelCatalog) { pendingPickerOpen.current = true; void openModelDialog(); return; }
    const currentModel = settings?.codexModel ?? "";
    const currentEffort = settings?.codexEffort ?? "";
    const modelIdx = Math.max(0, models.findIndex((m) => m.id === currentModel));
    setPickerModel(currentModel);
    setPickerEffort(currentEffort);
    setFocusedModel(modelIdx);
    setPicker({ step: "model" });
    // Focus first item after render
    requestAnimationFrame(() => {
      pickerRef.current?.querySelectorAll<HTMLElement>("[data-picker-item]")[modelIdx]?.focus();
    });
  };

  const closePicker = () => {
    setPicker(null);
    modelChipRef.current?.focus();
  };

  // Called when the user confirms a model selection (Enter/Tab on model step).
  const commitModel = (modelId: string) => {
    const model = models.find((m) => m.id === modelId);
    if (!model) return;

    // Resolve effort: keep current if valid, else use model default
    const validEffortIds = model.effortLevels.map((e) => e.id);
    let effort = pickerEffort;
    if (validEffortIds.length === 0) {
      effort = "";
    } else if (!validEffortIds.includes(effort)) {
      effort = model.defaultEffort || validEffortIds[0];
    }
    setPickerModel(modelId);
    setPickerEffort(effort);

    if (model.effortLevels.length > 0) {
      // Advance to effort step
      const effortIdx = Math.max(0, model.effortLevels.findIndex((e) => e.id === effort));
      setFocusedEffort(effortIdx);
      setPicker({ step: "effort" });
      requestAnimationFrame(() => {
        pickerRef.current?.querySelectorAll<HTMLElement>("[data-picker-item]")[effortIdx]?.focus();
      });
    } else {
      // No effort — save immediately
      void saveSelection(modelId, "");
    }
  };

  const commitEffort = (effortId: string) => {
    void saveSelection(pickerModel, effortId);
  };

  const saveSelection = async (modelId: string, effortId: string) => {
    setPicker(null);
    await useStore.getState().saveModelSelection(modelId, effortId);
    modelChipRef.current?.focus();
  };

  // -------------------------------------------------------------------------
  // Inline palette (@ / / commands in textarea)
  // -------------------------------------------------------------------------

  const filteredFiles = useMemo(() => {
    const query = palette?.query.toLowerCase() ?? "";
    return projectFiles.filter((file) => file.toLowerCase().includes(query)).slice(0, 8);
  }, [palette, projectFiles]);

  const detectPalette = (value: string, cursor: number) => {
    const before = value.slice(0, cursor);
    const match = before.match(/(?:^|\s)([@/])([^\s]*)$/);
    if (!match) { setPalette(null); return; }
    const tokenStart = cursor - match[0].length + (match[0].startsWith(" ") ? 1 : 0);
    setPalette({ type: match[1] === "@" ? "reference" : "command", query: match[2], start: tokenStart, end: cursor });
  };

  const handlePromptChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.target.value;
    cursorRef.current = event.target.selectionStart;
    setPrompt(value);
    detectPalette(value, event.target.selectionStart);
  };

  const insertPaletteItem = (value: string, type: PaletteType) => {
    const active = palette;
    if (!active) return;

    if (type === "command" && value === "model") {
      setPrompt((prev) => prev.slice(0, active.start).trimEnd());
      setPalette(null);
      openPicker();
      return;
    }

    const replacement = `${type === "reference" ? "@" : "/"}${value} `;
    const nextPrompt = `${prompt.slice(0, active.start)}${replacement}${prompt.slice(active.end)}`;
    setPrompt(nextPrompt);
    setPalette(null);
    if (type === "reference") setFileRefs((current) => current.includes(value) ? current : [...current, value]);
    requestAnimationFrame(() => {
      const nextCursor = active.start + replacement.length;
      cursorRef.current = nextCursor;
      promptRef.current?.focus();
      promptRef.current?.setSelectionRange(nextCursor, nextCursor);
    });
  };

  const handleAddFiles = async () => {
    const result = await openDialog({ directory: false, multiple: true, defaultPath: project.rootPath, title: "Add file references" });
    if (!result) return;
    const paths = Array.isArray(result) ? result : [result];
    const refs = paths.map((path) => makeRelative(project.rootPath, path)).filter((ref) => ref && !fileRefs.includes(ref));
    setFileRefs((current) => [...current, ...refs]);
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!title.trim() || !prompt.trim() || isSubmitting) return;
    setIsSubmitting(true);
    setError(null);
    try { await onCreate({ title: title.trim(), prompt: prompt.trim(), fileRefs }); }
    catch (submissionError) { setError(String(submissionError)); setIsSubmitting(false); }
  };

  const canSubmit = Boolean(title.trim() && prompt.trim()) && !isSubmitting;
  const paletteItems = palette?.type === "reference" ? filteredFiles : COMMANDS.filter((command) => command.name.includes(palette?.query.toLowerCase() ?? ""));

  // -------------------------------------------------------------------------
  // Model chip label
  // -------------------------------------------------------------------------
  const activeModel = models.find((m) => m.id === settings?.codexModel);
  const chipLabel = activeModel
    ? `${activeModel.label}${settings?.codexEffort ? ` · ${settings.codexEffort}` : ""}`
    : (settings?.codexModel ?? "");

  // -------------------------------------------------------------------------
  // Active effort levels for the current picker model
  // -------------------------------------------------------------------------
  const pickerActiveModel = models.find((m) => m.id === pickerModel);
  const effortLevels: EffortLevel[] = pickerActiveModel?.effortLevels ?? [];

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------
  return (
    <section className="composer-pane">
      <div className="composer-intro">
        <div className="composer-eyebrow"><span className="composer-prompt-symbol">›</span> New task <span className="composer-project">in {project.rootPath}</span></div>
        <h1>What should Codex work on?</h1>
        <p>Describe the change in plain language. Use <kbd>@</kbd> to reference files and <kbd>/</kbd> for commands.</p>
      </div>

      <form className="task-composer" onSubmit={handleSubmit}>
        <input ref={titleRef} className="composer-title" value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Task title" maxLength={200} aria-label="Task title" />
        <div className="composer-editor-wrap">
          <textarea
            ref={promptRef}
            className="composer-editor"
            value={prompt}
            onChange={handlePromptChange}
            onKeyUp={(event) => { cursorRef.current = event.currentTarget.selectionStart; detectPalette(event.currentTarget.value, event.currentTarget.selectionStart); }}
            placeholder="Describe the task…"
            rows={9}
            aria-label="Task prompt"
          />
          {palette && (
            <div className="inline-palette" role="listbox">
              <div className="inline-palette-header"><span>{palette.type === "reference" ? "Project files" : "Commands"}</span><kbd>↑↓</kbd></div>
              {paletteItems.length === 0
                ? <div className="inline-palette-empty">No matches</div>
                : paletteItems.map((item) => {
                  const name = typeof item === "string" ? item : item.name;
                  const label = typeof item === "string" ? item : item.label;
                  const hint = typeof item === "string" ? "File in this project" : item.hint;
                  return (
                    <button key={name} type="button" className="inline-palette-item" onMouseDown={(event) => event.preventDefault()} onClick={() => insertPaletteItem(name, palette.type)}>
                      <span className="palette-item-icon">{palette.type === "reference" ? "·/" : "/"}</span>
                      <span><strong>{label}</strong><small>{hint}</small></span>
                      <kbd>↵</kbd>
                    </button>
                  );
                })}
            </div>
          )}
        </div>

        {fileRefs.length > 0 && (
          <div className="composer-context">
            <span className="context-heading">Context</span>
            {fileRefs.map((ref) => (
              <button key={ref} type="button" className="context-chip" onClick={() => setFileRefs((current) => current.filter((item) => item !== ref))}>
                @{ref}<span>×</span>
              </button>
            ))}
          </div>
        )}

        <div className="composer-footer">
          <div className="composer-tools">
            <button type="button" className="composer-tool" onClick={handleAddFiles}>＋ Add context</button>
            <span className="composer-hint"><kbd>@</kbd> files <kbd>/</kbd> commands</span>
          </div>

          {/* Model chip + inline picker anchor */}
          <div className="composer-model-wrap">
            <button
              ref={modelChipRef}
              type="button"
              className={`composer-model-chip${picker ? " open" : ""}`}
              onClick={openPicker}
              aria-haspopup="listbox"
              aria-expanded={!!picker}
              title="Change model (/model)"
            >
              <span className="composer-model-chip-icon">◈</span>
              {settings ? chipLabel : "Model"}
            </button>

            {picker && (
              <div ref={pickerRef} className="model-picker" role="listbox" aria-label={picker.step === "model" ? "Select model" : "Select effort"}>
                <div className="model-picker-header">
                  <span>{picker.step === "model" ? "Model" : "Reasoning effort"}</span>
                  <kbd>↑↓ navigate · ↵ select · esc cancel</kbd>
                </div>

                {picker.step === "model" && (
                  <ModelPickerList
                    models={models}
                    focusedIdx={focusedModel}
                    selectedId={pickerModel}
                    onFocus={setFocusedModel}
                    onCommit={commitModel}
                    onEscape={closePicker}
                  />
                )}

                {picker.step === "effort" && (
                  <EffortPickerList
                    levels={effortLevels}
                    focusedIdx={focusedEffort}
                    selectedId={pickerEffort}
                    onFocus={setFocusedEffort}
                    onCommit={commitEffort}
                    onBack={() => {
                      // Go back to model step
                      const modelIdx = Math.max(0, models.findIndex((m) => m.id === pickerModel));
                      setFocusedModel(modelIdx);
                      setPicker({ step: "model" });
                      requestAnimationFrame(() => {
                        pickerRef.current?.querySelectorAll<HTMLElement>("[data-picker-item]")[modelIdx]?.focus();
                      });
                    }}
                    onEscape={closePicker}
                  />
                )}
              </div>
            )}
          </div>

          <button type="submit" className="send-button" disabled={!canSubmit}>
            {isSubmitting ? "Creating…" : "Create task"}<span>⌘ ↵</span>
          </button>
        </div>

        {error && <div className="composer-error" role="alert">{error}</div>}
      </form>

      <div className="composer-footnote">
        <span>i</span> Tasks start from the current commit in an isolated worktree. Your uncommitted changes stay untouched.
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// ModelPickerList — keyboard-navigable model list inside the inline picker
// ---------------------------------------------------------------------------

interface ModelPickerListProps {
  models: ModelOption[];
  focusedIdx: number;
  selectedId: string;
  onFocus: (idx: number) => void;
  onCommit: (id: string) => void;
  onEscape: () => void;
}

function ModelPickerList({ models, focusedIdx, selectedId, onFocus, onCommit, onEscape }: ModelPickerListProps) {
  const listRef = useRef<HTMLDivElement>(null);

  const move = (dir: 1 | -1) => {
    const next = (focusedIdx + dir + models.length) % models.length;
    onFocus(next);
    listRef.current?.querySelectorAll<HTMLElement>("[data-picker-item]")[next]?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
    else if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      if (models[focusedIdx]) onCommit(models[focusedIdx].id);
    } else if (e.key === "Escape") { e.preventDefault(); onEscape(); }
  };

  return (
    <div ref={listRef} onKeyDown={handleKeyDown}>
      {models.map((model, idx) => {
        const isFocused = idx === focusedIdx;
        const isSelected = model.id === selectedId;
        return (
          <button
            key={model.id}
            type="button"
            role="option"
            aria-selected={isSelected}
            data-picker-item={model.id}
            className={`model-picker-item${isSelected ? " selected" : ""}${isFocused ? " focused" : ""}`}
            tabIndex={isFocused ? 0 : -1}
            onFocus={() => onFocus(idx)}
            onClick={() => onCommit(model.id)}
          >
            <span className="model-picker-item-body">
              <span className="model-picker-item-label">{model.label}</span>
              <span className="model-picker-item-desc">{model.description}</span>
            </span>
            <span className="model-picker-item-end">
              {model.effortLevels.length > 0 && (
                <span className="model-picker-badge">reasoning</span>
              )}
              <span className="model-picker-item-check">{isSelected ? "●" : ""}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// EffortPickerList — keyboard-navigable effort list inside the inline picker
// ---------------------------------------------------------------------------

interface EffortPickerListProps {
  levels: EffortLevel[];
  focusedIdx: number;
  selectedId: string;
  onFocus: (idx: number) => void;
  onCommit: (id: string) => void;
  onBack: () => void;
  onEscape: () => void;
}

function EffortPickerList({ levels, focusedIdx, selectedId, onFocus, onCommit, onBack, onEscape }: EffortPickerListProps) {
  const listRef = useRef<HTMLDivElement>(null);

  const move = (dir: 1 | -1) => {
    const next = (focusedIdx + dir + levels.length) % levels.length;
    onFocus(next);
    listRef.current?.querySelectorAll<HTMLElement>("[data-picker-item]")[next]?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
    else if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      if (levels[focusedIdx]) onCommit(levels[focusedIdx].id);
    } else if (e.key === "Escape") { e.preventDefault(); onEscape(); }
    else if (e.key === "Backspace") { e.preventDefault(); onBack(); }
  };

  return (
    <div ref={listRef} onKeyDown={handleKeyDown}>
      <button type="button" className="model-picker-back" onClick={onBack} tabIndex={-1}>
        ‹ Back to model
      </button>
      {levels.map((level, idx) => {
        const isFocused = idx === focusedIdx;
        const isSelected = level.id === selectedId;
        return (
          <button
            key={level.id}
            type="button"
            role="option"
            aria-selected={isSelected}
            data-picker-item={level.id}
            className={`model-picker-item${isSelected ? " selected" : ""}${isFocused ? " focused" : ""}`}
            tabIndex={isFocused ? 0 : -1}
            onFocus={() => onFocus(idx)}
            onClick={() => onCommit(level.id)}
          >
            <span className="model-picker-item-body">
              <span className="model-picker-item-label">{level.id}</span>
              <span className="model-picker-item-desc">{level.description}</span>
            </span>
            <span className="model-picker-item-end">
              <span className="model-picker-item-check">{isSelected ? "●" : ""}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}
