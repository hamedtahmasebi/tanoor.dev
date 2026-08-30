import { useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { readDir } from "@tauri-apps/plugin-fs";
import type { CreateTaskInput, Project } from "../types";

type PaletteType = "reference" | "command";
interface PaletteState { type: PaletteType; query: string; start: number; end: number; }

interface Props { project: Project; onClose: () => void; onCreate: (input: CreateTaskInput) => Promise<void>; }

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

export function NewTaskDialog({ project, onClose, onCreate }: Props) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [fileRefs, setFileRefs] = useState<string[]>([]);
  const [projectFiles, setProjectFiles] = useState<string[]>([]);
  const [palette, setPalette] = useState<PaletteState | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const cursorRef = useRef(0);

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

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (palette) setPalette(null);
      else onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose, palette]);

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

  return (
    <section className="composer-pane">
      <div className="composer-intro"><div className="composer-eyebrow"><span className="composer-prompt-symbol">›</span> New task <span className="composer-project">in {project.rootPath}</span></div><h1>What should Codex work on?</h1><p>Describe the change in plain language. Use <kbd>@</kbd> to reference files and <kbd>/</kbd> for commands.</p></div>
      <form className="task-composer" onSubmit={handleSubmit}>
        <input ref={titleRef} className="composer-title" value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Task title" maxLength={200} aria-label="Task title" />
        <div className="composer-editor-wrap">
          <textarea ref={promptRef} className="composer-editor" value={prompt} onChange={handlePromptChange} onKeyUp={(event) => { cursorRef.current = event.currentTarget.selectionStart; detectPalette(event.currentTarget.value, event.currentTarget.selectionStart); }} placeholder="Describe the task…" rows={9} aria-label="Task prompt" />
          {palette && <div className="inline-palette" role="listbox"><div className="inline-palette-header"><span>{palette.type === "reference" ? "Project files" : "Commands"}</span><kbd>↑↓</kbd></div>{paletteItems.length === 0 ? <div className="inline-palette-empty">No matches</div> : paletteItems.map((item) => { const name = typeof item === "string" ? item : item.name; const label = typeof item === "string" ? item : item.label; const hint = typeof item === "string" ? "File in this project" : item.hint; return <button key={name} type="button" className="inline-palette-item" onMouseDown={(event) => event.preventDefault()} onClick={() => insertPaletteItem(name, palette.type)}><span className="palette-item-icon">{palette.type === "reference" ? "·/" : "/"}</span><span><strong>{label}</strong><small>{hint}</small></span><kbd>↵</kbd></button>; })}</div>}
        </div>
        {fileRefs.length > 0 && <div className="composer-context"><span className="context-heading">Context</span>{fileRefs.map((ref) => <button key={ref} type="button" className="context-chip" onClick={() => setFileRefs((current) => current.filter((item) => item !== ref))}>@{ref}<span>×</span></button>)}</div>}
        <div className="composer-footer"><div className="composer-tools"><button type="button" className="composer-tool" onClick={handleAddFiles}>＋ Add context</button><span className="composer-hint"><kbd>@</kbd> files <kbd>/</kbd> commands</span></div><button type="submit" className="send-button" disabled={!canSubmit}>{isSubmitting ? "Creating…" : "Create task"}<span>⌘ ↵</span></button></div>
        {error && <div className="composer-error" role="alert">{error}</div>}
      </form>
      <div className="composer-footnote"><span>i</span> Tasks start from the current commit in an isolated worktree. Your uncommitted changes stay untouched.</div>
    </section>
  );
}
