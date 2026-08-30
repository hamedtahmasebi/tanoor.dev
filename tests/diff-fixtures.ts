import { parseUnifiedDiff } from "../src/diff.js";

function equal(actual: unknown, expected: unknown, message: string) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message}\nexpected ${JSON.stringify(expected)}\nreceived ${JSON.stringify(actual)}`);
  }
}

const added = `diff --git a/src/new.ts b/src/new.ts
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/src/new.ts
@@ -0,0 +1,2 @@
+export const answer = 42;
+export default answer;
`;

const removed = `diff --git a/legacy.txt b/legacy.txt
deleted file mode 100644
index 1111111..0000000
--- a/legacy.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-old
-content
`;

const renamed = `diff --git "a/old name.txt" "b/new name.txt"
similarity index 92%
rename from old name.txt
rename to new name.txt
--- "a/old name.txt"
+++ "b/new name.txt"
@@ -1 +1 @@
-before
+after
`;

const [addedFile] = parseUnifiedDiff(added);
equal(
  [addedFile.status, addedFile.oldPath, addedFile.newPath, addedFile.additions, addedFile.deletions],
  ["added", null, "src/new.ts", 2, 0],
  "added-file fixture",
);
equal(
  addedFile.hunks[0].lines.map((line) => [line.oldLineNumber, line.newLineNumber]),
  [[null, 1], [null, 2]],
  "added-file line anchors",
);

const [removedFile] = parseUnifiedDiff(removed);
equal(
  [removedFile.status, removedFile.oldPath, removedFile.newPath, removedFile.additions, removedFile.deletions],
  ["deleted", "legacy.txt", null, 0, 2],
  "removed-file fixture",
);

const [renamedFile] = parseUnifiedDiff(renamed);
equal(
  [renamedFile.status, renamedFile.oldPath, renamedFile.newPath, renamedFile.displayPath],
  ["renamed", "old name.txt", "new name.txt", "new name.txt"],
  "renamed-file fixture",
);
equal(
  renamedFile.hunks[0].lines.map((line) => [line.kind, line.oldLineNumber, line.newLineNumber]),
  [["deletion", 1, null], ["addition", null, 1]],
  "renamed-file hunk anchors",
);

console.log("diff fixtures passed");
