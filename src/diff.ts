export type DiffFileStatus = "added" | "deleted" | "modified" | "renamed";
export type DiffLineKind = "context" | "addition" | "deletion" | "meta";

export interface DiffLine {
  id: string;
  kind: DiffLineKind;
  content: string;
  oldLineNumber: number | null;
  newLineNumber: number | null;
}

export interface DiffHunk {
  id: string;
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface DiffFile {
  id: string;
  oldPath: string | null;
  newPath: string | null;
  displayPath: string;
  status: DiffFileStatus;
  additions: number;
  deletions: number;
  hunks: DiffHunk[];
  binary: boolean;
}

interface MutableFile extends DiffFile {
  renameFrom?: string;
  renameTo?: string;
}

function decodeGitPath(value: string): string {
  const unquoted = value.startsWith('"') && value.endsWith('"')
    ? value.slice(1, -1)
    : value;
  return unquoted.replace(/\\([\\"tnr])/g, (_, escaped: string) => {
    if (escaped === "t") return "\t";
    if (escaped === "n") return "\n";
    if (escaped === "r") return "\r";
    return escaped;
  });
}

function normalizeMarkerPath(value: string): string | null {
  const decoded = decodeGitPath(value.trim());
  if (decoded === "/dev/null") return null;
  return decoded.replace(/^[ab]\//, "");
}

function pathsFromGitHeader(line: string): [string | null, string | null] {
  const match = line.match(
    /^diff --git (?:"a\/((?:\\.|[^"])*)"|a\/([^ ]+)) (?:"b\/((?:\\.|[^"])*)"|b\/([^ ]+))$/,
  );
  if (!match) return [null, null];
  return [decodeGitPath(match[1] ?? match[2]), decodeGitPath(match[3] ?? match[4])];
}

function finishFile(file: MutableFile): DiffFile {
  if (file.renameFrom) file.oldPath = file.renameFrom;
  if (file.renameTo) file.newPath = file.renameTo;
  if (file.oldPath === null) file.status = "added";
  else if (file.newPath === null) file.status = "deleted";
  else if (file.renameFrom || file.renameTo || file.oldPath !== file.newPath) file.status = "renamed";
  else file.status = "modified";
  file.displayPath = file.newPath ?? file.oldPath ?? "unknown file";
  file.id = `${file.oldPath ?? "/dev/null"}->${file.newPath ?? "/dev/null"}`;
  const { renameFrom: _renameFrom, renameTo: _renameTo, ...result } = file;
  return result;
}

/** Parse the cumulative `git diff` text used by Tanoor's review screen. */
export function parseUnifiedDiff(input: string): DiffFile[] {
  if (!input.trim()) return [];
  const files: DiffFile[] = [];
  let file: MutableFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;

  const pushFile = () => {
    if (file) files.push(finishFile(file));
    file = null;
    hunk = null;
  };

  for (const line of input.replace(/\r\n/g, "\n").split("\n")) {
    if (line.startsWith("diff --git ")) {
      pushFile();
      const [oldPath, newPath] = pathsFromGitHeader(line);
      file = {
        id: "",
        oldPath,
        newPath,
        displayPath: newPath ?? oldPath ?? "unknown file",
        status: "modified",
        additions: 0,
        deletions: 0,
        hunks: [],
        binary: false,
      };
      continue;
    }
    if (!file) continue;
    if (line.startsWith("rename from ")) {
      file.renameFrom = decodeGitPath(line.slice("rename from ".length));
      continue;
    }
    if (line.startsWith("rename to ")) {
      file.renameTo = decodeGitPath(line.slice("rename to ".length));
      continue;
    }
    if (line.startsWith("--- ")) {
      file.oldPath = normalizeMarkerPath(line.slice(4));
      continue;
    }
    if (line.startsWith("+++ ")) {
      file.newPath = normalizeMarkerPath(line.slice(4));
      continue;
    }
    if (line.startsWith("Binary files ") || line.startsWith("GIT binary patch")) {
      file.binary = true;
      continue;
    }

    const hunkMatch = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/);
    if (hunkMatch) {
      oldLine = Number(hunkMatch[1]);
      newLine = Number(hunkMatch[3]);
      hunk = {
        id: `${file.hunks.length}:${oldLine}:${newLine}`,
        header: line,
        oldStart: oldLine,
        oldLines: Number(hunkMatch[2] ?? 1),
        newStart: newLine,
        newLines: Number(hunkMatch[4] ?? 1),
        lines: [],
      };
      file.hunks.push(hunk);
      continue;
    }
    if (!hunk) continue;
    // `split("\n")` produces an empty item for the diff's final newline.
    // Real empty content lines still carry their unified-diff prefix (` `,
    // `+`, or `-`), so an unprefixed empty string is never a source line.
    if (line === "") continue;

    let parsed: DiffLine;
    if (line.startsWith("+")) {
      parsed = {
        id: `${hunk.id}:new:${newLine}`,
        kind: "addition",
        content: line.slice(1),
        oldLineNumber: null,
        newLineNumber: newLine++,
      };
      file.additions += 1;
    } else if (line.startsWith("-")) {
      parsed = {
        id: `${hunk.id}:old:${oldLine}`,
        kind: "deletion",
        content: line.slice(1),
        oldLineNumber: oldLine++,
        newLineNumber: null,
      };
      file.deletions += 1;
    } else if (line.startsWith(" ")) {
      parsed = {
        id: `${hunk.id}:both:${oldLine}:${newLine}`,
        kind: "context",
        content: line.slice(1),
        oldLineNumber: oldLine++,
        newLineNumber: newLine++,
      };
    } else {
      parsed = {
        id: `${hunk.id}:meta:${hunk.lines.length}`,
        kind: "meta",
        content: line,
        oldLineNumber: null,
        newLineNumber: null,
      };
    }
    hunk.lines.push(parsed);
  }
  pushFile();
  return files;
}
