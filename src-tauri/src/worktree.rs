//! Git worktree operations via the system `git` CLI.
//!
//! This module is intentionally independent of Tauri — it can be unit-tested
//! against throwaway git repositories without any app state.  All operations
//! shell out to the binary named by [`WorktreeManager::git_bin`]; the default
//! is `"git"`, resolved via PATH.

use std::path::Path;

use crate::error::AppError;

// ---------------------------------------------------------------------------
// WorktreeManager
// ---------------------------------------------------------------------------

/// Drives every git operation Tanoor needs.
///
/// Branch names are decided when tasks are created, typically as
/// `task/<generated-slug>-<task-id-prefix>`.
/// Worktree path convention (caller-controlled): `{app_data_dir}/worktrees/{task_id}`.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    /// Name or absolute path of the `git` binary.  Defaults to `"git"`.
    pub git_bin: String,
}

impl Default for WorktreeManager {
    fn default() -> Self {
        Self {
            git_bin: "git".to_string(),
        }
    }
}

impl WorktreeManager {
    pub fn new(git_bin: impl Into<String>) -> Self {
        Self {
            git_bin: git_bin.into(),
        }
    }

    // -----------------------------------------------------------------------
    // Internal helper
    // -----------------------------------------------------------------------

    /// Run a git sub-command in `cwd` and return trimmed stdout on success,
    /// or an [`AppError::Git`] that includes stderr on failure.
    fn git(&self, args: &[&str], cwd: &Path) -> Result<String, AppError> {
        let out = std::process::Command::new(&self.git_bin)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| AppError::Git(format!("Failed to launch '{}': {e}", self.git_bin)))?;

        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            Err(AppError::Git(format!(
                "`git {}` failed: {detail}",
                args.join(" ")
            )))
        }
    }

    // -----------------------------------------------------------------------
    // Repository inspection
    // -----------------------------------------------------------------------

    /// Returns `true` if `path` is inside a git work tree.
    ///
    /// Uses `git -C <path> rev-parse --git-dir` so it works whether `path`
    /// is the repo root, a subdirectory, or a linked worktree.
    pub fn is_git_repo(&self, path: &Path) -> bool {
        std::process::Command::new(&self.git_bin)
            .args(["-C", &path.to_string_lossy(), "rev-parse", "--git-dir"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Returns the full 40-character SHA of HEAD in `repo_root`.
    ///
    /// ```text
    /// git rev-parse HEAD
    /// ```
    pub fn head_sha(&self, repo_root: &Path) -> Result<String, AppError> {
        self.git(&["rev-parse", "HEAD"], repo_root)
    }

    // -----------------------------------------------------------------------
    // Worktree lifecycle
    // -----------------------------------------------------------------------

    /// Create a new linked worktree at `worktree_path` on a new branch
    /// `branch_name`, starting from `base_ref`.
    ///
    /// ```text
    /// git worktree add <worktree_path> -b <branch_name> <base_ref>
    /// ```
    ///
    /// `worktree_path` should be an absolute path to avoid ambiguity.
    pub fn add_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        branch_name: &str,
        base_ref: &str,
    ) -> Result<(), AppError> {
        let wt = worktree_path.to_string_lossy();
        self.git(
            &["worktree", "add", wt.as_ref(), "-b", branch_name, base_ref],
            repo_root,
        )?;
        Ok(())
    }

    /// Restore a worktree that was removed externally, checking out its
    /// existing branch after pruning Git's stale worktree metadata.
    pub fn ensure_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        branch_name: &str,
    ) -> Result<(), AppError> {
        if worktree_path.exists() {
            return Ok(());
        }
        let _ = self.git(&["worktree", "prune"], repo_root);
        let path = worktree_path.to_string_lossy();
        self.git(&["worktree", "add", path.as_ref(), branch_name], repo_root)?;
        Ok(())
    }

    /// Remove a linked worktree and delete its associated branch.
    ///
    /// * `--force` handles worktrees with dirty or untracked files.
    /// * If `worktree_path` no longer exists on disk, `git worktree prune`
    ///   is used to clean up stale index entries instead.
    /// * Branch deletion is non-fatal — if the branch is already gone the
    ///   error is swallowed.
    ///
    /// ```text
    /// git worktree remove --force <worktree_path>   # or prune if dir is gone
    /// git branch -D <branch_name>
    /// ```
    pub fn remove_worktree(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        branch_name: &str,
    ) -> Result<(), AppError> {
        self.remove_worktree_keep_branch(repo_root, worktree_path)?;
        // Non-fatal: branch may already be gone.
        let _ = self.git(&["branch", "-D", branch_name], repo_root);
        Ok(())
    }

    /// Remove a linked worktree while retaining its branch. Confirmation uses
    /// this path when merge-on-confirm is disabled so the approved commit
    /// remains reachable as `task/<task_id>`.
    pub fn remove_worktree_keep_branch(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
    ) -> Result<(), AppError> {
        let wt = worktree_path.to_string_lossy();
        if worktree_path.exists() {
            self.git(&["worktree", "remove", "--force", wt.as_ref()], repo_root)?;
        } else {
            let _ = self.git(&["worktree", "prune"], repo_root);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Diff
    // -----------------------------------------------------------------------

    /// Compute the cumulative unified diff of the worktree against `base_ref`.
    ///
    /// Returns an empty string when there are no changes relative to `base_ref`.
    ///
    /// To ensure new (untracked) files created by Codex appear in the diff,
    /// the index is temporarily updated with `git add -A` before diffing and
    /// then restored with `git reset HEAD .`.  The working tree itself is
    /// never touched.
    ///
    /// ```text
    /// git add -A
    /// git diff --cached <base_ref>
    /// git reset HEAD .
    /// ```
    pub fn diff(&self, worktree_path: &Path, base_ref: &str) -> Result<String, AppError> {
        self.git(&["add", "-A"], worktree_path)?;
        let diff = self.git(&["diff", "--cached", base_ref], worktree_path)?;
        // Restore the index; the working tree is unchanged.
        let _ = self.git(&["reset", "HEAD", "."], worktree_path);
        Ok(diff)
    }

    // -----------------------------------------------------------------------
    // Commit and merge
    // -----------------------------------------------------------------------

    /// Stage all changes and create a commit with `message` in `worktree_path`.
    ///
    /// Returns the new commit SHA.  If there is nothing to commit the current
    /// HEAD SHA is returned without creating an empty commit.
    ///
    /// ```text
    /// git add -A
    /// git commit -m <message>   # skipped when nothing is staged
    /// git rev-parse HEAD
    /// ```
    pub fn commit_all(&self, worktree_path: &Path, message: &str) -> Result<String, AppError> {
        self.git(&["add", "-A"], worktree_path)?;

        // `git diff --cached --quiet` exits 0 when nothing is staged.
        let nothing_staged = std::process::Command::new(&self.git_bin)
            .args(["diff", "--cached", "--quiet"])
            .current_dir(worktree_path)
            .status()
            .map_err(|e| AppError::Git(format!("Failed to launch git: {e}")))?
            .success();

        if !nothing_staged {
            self.git(&["commit", "-m", message], worktree_path)?;
        }

        self.git(&["rev-parse", "HEAD"], worktree_path)
    }

    /// Merge `branch_name` into the current branch of `repo_root` using
    /// `--no-ff` so the merge is always a named merge commit.
    ///
    /// ```text
    /// git merge --no-ff <branch_name>
    /// ```
    pub fn merge_branch(&self, repo_root: &Path, branch_name: &str) -> Result<(), AppError> {
        match self.git(&["merge", "--no-ff", branch_name], repo_root) {
            Ok(_) => Ok(()),
            Err(error) => {
                // A conflicted merge must not poison the user's main checkout.
                // `merge --abort` is harmlessly ignored when Git rejected the
                // merge before creating MERGE_HEAD (for example, dirty files).
                let _ = self.git(&["merge", "--abort"], repo_root);
                Err(AppError::Git(format!(
                    "Could not merge '{branch_name}'. The merge was aborted; the task branch and worktree were kept for retry. {error}"
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    // --- helpers ---

    fn git_ok(args: &[&str], dir: &Path) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git must be available on PATH for these tests");
        assert!(st.success(), "`git {args:?}` failed in {dir:?}");
    }

    /// Initialise a bare-minimum git repository with one commit.
    /// Returns the HEAD SHA.
    fn init_repo(dir: &Path) -> String {
        git_ok(&["init"], dir);
        git_ok(&["config", "user.email", "test@tanoor.test"], dir);
        git_ok(&["config", "user.name", "Tanoor Test"], dir);
        git_ok(&["config", "commit.gpgsign", "false"], dir);
        std::fs::write(dir.join("README.md"), "# test repo\n").unwrap();
        git_ok(&["add", "."], dir);
        git_ok(&["commit", "-m", "initial"], dir);
        head_sha_raw(dir)
    }

    fn head_sha_raw(dir: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["-C", dir.to_str().unwrap(), "rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    fn wm() -> WorktreeManager {
        WorktreeManager::default()
    }

    // --- repository inspection ---

    #[test]
    fn head_sha_returns_initial_commit() {
        let dir = TempDir::new().unwrap();
        let expected = init_repo(dir.path());
        let got = wm().head_sha(dir.path()).unwrap();
        assert_eq!(got, expected);
        assert_eq!(got.len(), 40, "must be full 40-char SHA");
    }

    #[test]
    fn is_git_repo_true_for_initialized_dir() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path());
        assert!(wm().is_git_repo(dir.path()));
    }

    #[test]
    fn is_git_repo_false_for_plain_directory() {
        let dir = TempDir::new().unwrap();
        assert!(!wm().is_git_repo(dir.path()));
    }

    #[test]
    fn is_git_repo_false_for_nonexistent_path() {
        assert!(!wm().is_git_repo(Path::new("/this/definitely/does/not/exist/12345")));
    }

    // --- worktree lifecycle ---

    #[test]
    fn add_and_remove_worktree() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t001", &base)
            .unwrap();

        assert!(
            wt.path().join("README.md").exists(),
            "worktree must contain initial files"
        );
        assert!(
            wm().is_git_repo(wt.path()),
            "worktree must be recognised as a git work tree"
        );

        wm().remove_worktree(repo.path(), wt.path(), "task/t001")
            .unwrap();

        let branches = String::from_utf8(
            Command::new("git")
                .args(["-C", repo.path().to_str().unwrap(), "branch"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            !branches.contains("task/t001"),
            "branch must be deleted after removal"
        );
    }

    #[test]
    fn dirty_worktree_is_force_removed() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t002", &base)
            .unwrap();

        // Dirty state: untracked file + modified tracked file.
        std::fs::write(wt.path().join("untracked.txt"), "dirty\n").unwrap();
        std::fs::write(wt.path().join("README.md"), "modified content\n").unwrap();

        // Must succeed despite dirty state (--force).
        wm().remove_worktree(repo.path(), wt.path(), "task/t002")
            .unwrap();
    }

    #[test]
    fn remove_worktree_when_directory_already_deleted() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t003", &base)
            .unwrap();

        // Simulate an external deletion of the worktree directory.
        std::fs::remove_dir_all(wt.path()).unwrap();

        // prune-based path must succeed.
        wm().remove_worktree(repo.path(), wt.path(), "task/t003")
            .unwrap();
    }

    #[test]
    fn ensure_worktree_restores_externally_deleted_worktree() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());
        wm().add_worktree(repo.path(), wt.path(), "task/recover", &base)
            .unwrap();
        std::fs::write(wt.path().join("committed.txt"), "kept\n").unwrap();
        wm().commit_all(wt.path(), "Tanoor: recovery fixture")
            .unwrap();
        std::fs::remove_dir_all(wt.path()).unwrap();

        wm().ensure_worktree(repo.path(), wt.path(), "task/recover")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.path().join("committed.txt"))
                .unwrap()
                .trim(),
            "kept"
        );
        wm().remove_worktree(repo.path(), wt.path(), "task/recover")
            .unwrap();
    }

    // --- diff ---

    #[test]
    fn diff_is_empty_on_fresh_worktree() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t004", &base)
            .unwrap();

        let diff = wm().diff(wt.path(), &base).unwrap();
        assert!(diff.is_empty(), "fresh worktree must produce an empty diff");

        wm().remove_worktree(repo.path(), wt.path(), "task/t004")
            .unwrap();
    }

    #[test]
    fn diff_reflects_modified_file() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t005", &base)
            .unwrap();
        std::fs::write(wt.path().join("README.md"), "# modified heading\n").unwrap();

        let diff = wm().diff(wt.path(), &base).unwrap();
        assert!(!diff.is_empty());
        assert!(diff.contains("README.md"));
        assert!(diff.contains("modified heading"));

        // Index must be restored: diff a second time returns the same result.
        let diff2 = wm().diff(wt.path(), &base).unwrap();
        assert_eq!(diff, diff2, "repeated diff must be idempotent");

        wm().remove_worktree(repo.path(), wt.path(), "task/t005")
            .unwrap();
    }

    #[test]
    fn diff_reflects_new_untracked_file() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t006", &base)
            .unwrap();
        std::fs::write(wt.path().join("new_file.py"), "print('hello')\n").unwrap();

        let diff = wm().diff(wt.path(), &base).unwrap();
        assert!(!diff.is_empty(), "new untracked files must appear in diff");
        assert!(diff.contains("new_file.py"));

        wm().remove_worktree(repo.path(), wt.path(), "task/t006")
            .unwrap();
    }

    // --- commit and merge ---

    #[test]
    fn commit_all_creates_new_commit() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t007", &base)
            .unwrap();
        std::fs::write(wt.path().join("output.txt"), "codex result\n").unwrap();

        let new_sha = wm()
            .commit_all(wt.path(), "Tanoor: apply Codex changes")
            .unwrap();

        assert_ne!(new_sha, base, "new commit SHA must differ from base");
        assert_eq!(new_sha.len(), 40);

        // Working tree is clean relative to HEAD after the commit.
        let status = String::from_utf8(
            Command::new("git")
                .args(["-C", wt.path().to_str().unwrap(), "status", "--porcelain"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            status.is_empty(),
            "working tree must be clean after commit_all"
        );

        // Diff against base must still show the committed change.
        let diff = wm().diff(wt.path(), &base).unwrap();
        assert!(
            !diff.is_empty(),
            "diff against base_ref must include committed changes"
        );

        wm().remove_worktree(repo.path(), wt.path(), "task/t007")
            .unwrap();
    }

    #[test]
    fn commit_all_is_noop_when_nothing_changed() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t008", &base)
            .unwrap();

        // No changes — commit_all must return the current HEAD without creating an empty commit.
        let returned_sha = wm().commit_all(wt.path(), "Tanoor: empty commit").unwrap();
        assert_eq!(returned_sha, head_sha_raw(wt.path()));

        let log_count: usize = String::from_utf8(
            Command::new("git")
                .args([
                    "-C",
                    wt.path().to_str().unwrap(),
                    "rev-list",
                    "--count",
                    "HEAD",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .parse()
        .unwrap();
        assert_eq!(
            log_count, 1,
            "no extra commit must be created when nothing changed"
        );

        wm().remove_worktree(repo.path(), wt.path(), "task/t008")
            .unwrap();
    }

    #[test]
    fn remove_worktree_keep_branch_preserves_approved_commit() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t008-keep", &base)
            .unwrap();
        std::fs::write(wt.path().join("approved.txt"), "approved\n").unwrap();
        let approved_sha = wm().commit_all(wt.path(), "Tanoor: approved").unwrap();

        wm().remove_worktree_keep_branch(repo.path(), wt.path())
            .unwrap();

        let branch_sha = String::from_utf8(
            Command::new("git")
                .args([
                    "-C",
                    repo.path().to_str().unwrap(),
                    "rev-parse",
                    "task/t008-keep",
                ])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        assert_eq!(branch_sha, approved_sha);
    }

    #[test]
    fn merge_branch_lands_changes_in_main_repo() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t009", &base)
            .unwrap();
        std::fs::write(wt.path().join("feature.txt"), "feature output\n").unwrap();
        wm().commit_all(wt.path(), "Tanoor: feature").unwrap();

        // Remove the worktree but keep the branch.
        let wt_path = wt.path().to_path_buf();
        // We need to keep wt alive so the path stays valid; remove the worktree via git.
        let wt_str = wt_path.to_string_lossy();
        let _ = Command::new("git")
            .args([
                "-C",
                repo.path().to_str().unwrap(),
                "worktree",
                "remove",
                "--force",
                wt_str.as_ref(),
            ])
            .status();

        // Merge the task branch into the main repo.
        wm().merge_branch(repo.path(), "task/t009").unwrap();

        // The merged file must now be present in the main repo.
        assert!(
            repo.path().join("feature.txt").exists(),
            "merged file must appear in the main repo"
        );

        // Verify a merge commit was created (--no-ff).
        let log = String::from_utf8(
            Command::new("git")
                .args(["-C", repo.path().to_str().unwrap(), "log", "--oneline"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            log.lines().count() >= 3,
            "must have initial + feature + merge commit"
        );
    }

    #[test]
    fn merge_conflict_is_aborted_and_task_branch_is_retained() {
        let repo = TempDir::new().unwrap();
        let wt = TempDir::new().unwrap();
        let base = init_repo(repo.path());

        wm().add_worktree(repo.path(), wt.path(), "task/t010", &base)
            .unwrap();
        std::fs::write(wt.path().join("README.md"), "# task version\n").unwrap();
        wm().commit_all(wt.path(), "Tanoor: task version").unwrap();

        std::fs::write(repo.path().join("README.md"), "# main version\n").unwrap();
        git_ok(&["add", "README.md"], repo.path());
        git_ok(&["commit", "-m", "main version"], repo.path());

        let error = wm().merge_branch(repo.path(), "task/t010").unwrap_err();
        assert!(error.to_string().contains("merge was aborted"));
        assert_eq!(
            std::fs::read_to_string(repo.path().join("README.md"))
                .unwrap()
                .trim(),
            "# main version"
        );
        let merge_head = Command::new("git")
            .args([
                "-C",
                repo.path().to_str().unwrap(),
                "rev-parse",
                "-q",
                "--verify",
                "MERGE_HEAD",
            ])
            .status()
            .unwrap();
        assert!(
            !merge_head.success(),
            "main checkout must not remain mid-merge"
        );
        assert!(wt.path().exists(), "task worktree must remain for retry");
        assert!(
            !head_sha_raw(wt.path()).is_empty(),
            "task branch must remain reachable"
        );
    }
}
