import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import { useStore } from "../store";
import type { ProjectEntry } from "../types";

// ---------------------------------------------------------------------------
// Imperative handle
//
// The composer textarea keeps DOM focus so the user can keep typing the `@`
// query, so tree navigation is driven from the outside through this handle
// rather than the tree's own key events.
// ---------------------------------------------------------------------------

export interface FileTreeHandle {
  moveDown: () => void;
  moveUp: () => void;
  expand: () => void;
  collapse: () => void;
  select: () => void;
}

interface Props {
  projectId: string;
  /** The text typed after `@`. Non-empty switches the tree to flat search. */
  query: string;
  onSelect: (path: string) => void;
}

interface FlatNode {
  entry: ProjectEntry;
  depth: number;
}

const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");

export const FileTree = forwardRef<FileTreeHandle, Props>(function FileTree(
  { projectId, query, onSelect },
  ref,
) {
  const { loadProjectDir, searchProjectFiles } = useStore();
  const [childrenByDir, setChildrenByDir] = useState<Record<string, ProjectEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState<Set<string>>(new Set());
  const [focused, setFocused] = useState<string | null>(null);
  const [results, setResults] = useState<string[] | null>(null);
  const [searching, setSearching] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);

  const searchMode = query.trim().length > 0;

  const loadDir = useCallback(
    async (path: string) => {
      setLoading((prev) => new Set(prev).add(path));
      try {
        const entries = await loadProjectDir(projectId, path || null);
        setChildrenByDir((prev) => ({ ...prev, [path]: entries }));
      } catch {
        // The store surfaces the error; leave the node unexpanded.
      } finally {
        setLoading((prev) => {
          const next = new Set(prev);
          next.delete(path);
          return next;
        });
      }
    },
    [projectId, loadProjectDir],
  );

  // Load the root level whenever the project changes.
  useEffect(() => {
    setChildrenByDir({});
    setExpanded(new Set());
    setFocused(null);
    void loadDir("");
  }, [projectId, loadDir]);

  // Debounced search whenever a query is present.
  useEffect(() => {
    if (!searchMode) {
      setResults(null);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const handle = setTimeout(() => {
      void searchProjectFiles(projectId, query.trim())
        .then((found) => {
          if (cancelled) return;
          setResults(found);
          setFocused(found[0] ?? null);
        })
        .catch(() => {
          if (!cancelled) setResults([]);
        })
        .finally(() => {
          if (!cancelled) setSearching(false);
        });
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [projectId, query, searchMode, searchProjectFiles]);

  // Flatten the loaded/expanded tree into visible rows.
  const flat = useMemo(() => {
    const rows: FlatNode[] = [];
    const walk = (dir: string, depth: number) => {
      for (const entry of childrenByDir[dir] ?? []) {
        rows.push({ entry, depth });
        if (entry.isDir && expanded.has(entry.path)) walk(entry.path, depth + 1);
      }
    };
    walk("", 0);
    return rows;
  }, [childrenByDir, expanded]);

  const visiblePaths = useMemo(
    () => (searchMode ? results ?? [] : flat.map((node) => node.entry.path)),
    [searchMode, results, flat],
  );

  // Keep a valid focus target as the visible rows change.
  useEffect(() => {
    if (searchMode) return;
    if (focused === null || !visiblePaths.includes(focused)) {
      setFocused(visiblePaths[0] ?? null);
    }
  }, [visiblePaths, searchMode, focused]);

  // Scroll the focused row into view.
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(".file-tree-row.active")?.scrollIntoView({ block: "nearest" });
  }, [focused]);

  const expandAt = (path: string) => {
    setExpanded((prev) => new Set(prev).add(path));
    if (!childrenByDir[path]) void loadDir(path);
  };
  const collapseAt = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      next.delete(path);
      return next;
    });

  const moveBy = (delta: number) => {
    if (!visiblePaths.length) return;
    const index = focused ? visiblePaths.indexOf(focused) : -1;
    const base = index < 0 ? 0 : index;
    const next = (base + delta + visiblePaths.length) % visiblePaths.length;
    setFocused(visiblePaths[next]);
  };

  const expand = () => {
    if (searchMode || !focused) return;
    const node = flat.find((item) => item.entry.path === focused);
    if (!node?.entry.isDir) return;
    if (!expanded.has(focused)) {
      expandAt(focused);
    } else {
      const kids = childrenByDir[focused];
      if (kids?.length) setFocused(kids[0].path);
    }
  };

  const collapse = () => {
    if (searchMode || !focused) return;
    const node = flat.find((item) => item.entry.path === focused);
    if (node?.entry.isDir && expanded.has(focused)) {
      collapseAt(focused);
      return;
    }
    const parent = parentOf(focused);
    if (parent) setFocused(parent);
  };

  const activate = (entry: ProjectEntry) => {
    setFocused(entry.path);
    if (!entry.isDir) {
      onSelect(entry.path);
      return;
    }
    if (expanded.has(entry.path)) collapseAt(entry.path);
    else expandAt(entry.path);
  };

  const select = () => {
    if (searchMode) {
      if (focused) onSelect(focused);
      return;
    }
    const node = flat.find((item) => item.entry.path === focused);
    if (node) activate(node.entry);
  };

  useImperativeHandle(ref, () => ({
    moveDown: () => moveBy(1),
    moveUp: () => moveBy(-1),
    expand,
    collapse,
    select,
  }));

  const rootLoading = loading.has("") && flat.length === 0;

  return (
    <div className="file-tree" ref={listRef} role="tree" aria-label="Project files">
      {searchMode ? (
        searching && !results ? (
          <div className="inline-palette-empty">Searching…</div>
        ) : results && results.length ? (
          results.map((path) => (
            <button
              key={path}
              type="button"
              role="option"
              aria-selected={path === focused}
              className={`file-tree-row${path === focused ? " active" : ""}`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onSelect(path)}
            >
              <span className="file-tree-twisty" />
              <span className="file-tree-icon">·</span>
              <span className="file-tree-label file-tree-label--path">{path}</span>
            </button>
          ))
        ) : (
          <div className="inline-palette-empty">No matching files</div>
        )
      ) : rootLoading ? (
        <div className="inline-palette-empty">Loading…</div>
      ) : flat.length ? (
        flat.map(({ entry, depth }) => (
          <button
            key={entry.path}
            type="button"
            role="treeitem"
            aria-expanded={entry.isDir ? expanded.has(entry.path) : undefined}
            aria-selected={entry.path === focused}
            className={`file-tree-row${entry.path === focused ? " active" : ""}`}
            style={{ paddingLeft: 8 + depth * 14 }}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => activate(entry)}
          >
            <span className="file-tree-twisty">{entry.isDir ? (expanded.has(entry.path) ? "▾" : "▸") : ""}</span>
            <span className="file-tree-icon">{entry.isDir ? "◇" : "·"}</span>
            <span className="file-tree-label">{entry.name}</span>
            {loading.has(entry.path) && <span className="loading-spinner file-tree-spinner" />}
          </button>
        ))
      ) : (
        <div className="inline-palette-empty">No files</div>
      )}
    </div>
  );
});
