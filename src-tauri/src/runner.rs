//! Codex agent runner — isolates all Codex CLI process and protocol details.
//!
//! # Public API
//!
//! * [`AgentRunner`] — trait implemented by every agent backend (v1 has one).
//! * [`CodexExecRunner`] — spawns `codex exec --json` and streams JSONL events.
//! * [`CodexEvent`] — one parsed event from the stdout JSONL stream.
//! * [`EventStream`] — channel receiver of [`CodexEvent`]s.
//! * [`RunHandle`] — cancel a running turn and reap the child process.
//! * [`health_check`] — quick binary + auth check for the Settings page.
//!
//! # Independence
//!
//! This module has no dependency on Tauri, SQLite, or the worktree module.
//! It can be exercised against a real `codex` install or a mock shell script
//! without any app setup.

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use crate::error::AppError;

// ---------------------------------------------------------------------------
// CodexEvent
// ---------------------------------------------------------------------------

/// One event from the Codex `--json` JSONL stdout stream.
///
/// The `type` field (accessible as [`event_type`][Self::event_type]) uses
/// Codex's dotted naming: `"thread.started"`, `"turn.completed"`, etc.
/// The complete raw JSON is preserved so every detail can be forwarded to
/// the UI without loss.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexEvent {
    /// Dotted type string from the JSONL `"type"` field.
    pub event_type: String,
    /// Full parsed JSON object for this event.
    pub raw: serde_json::Value,
}

impl CodexEvent {
    /// Try to parse a single JSONL line.
    ///
    /// Returns `None` if the line is not valid JSON or has no `"type"` field.
    /// Lines that are not valid events (e.g. blank lines between records)
    /// should be silently skipped by the caller.
    pub fn parse(line: &str) -> Option<Self> {
        let raw: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let event_type = raw.get("type")?.as_str()?.to_string();
        Some(Self { event_type, raw })
    }

    /// Extract the Codex `thread_id` from a `thread.started` event.
    /// Returns `None` for all other event types.
    pub fn thread_id(&self) -> Option<&str> {
        if self.event_type == "thread.started" {
            self.raw.get("thread_id").and_then(|v| v.as_str())
        } else {
            None
        }
    }

    /// `true` for `turn.completed` and `turn.failed` — the two events that
    /// mark the definitive end of a Codex turn.
    pub fn is_terminal(&self) -> bool {
        matches!(self.event_type.as_str(), "turn.completed" | "turn.failed")
    }

    /// Return the human-readable error string for `turn.failed` or `error`
    /// events, where the payload may be `{"error":"..."}`,
    /// `{"error":{"message":"..."}}`, or `{"message":"..."}`.
    pub fn error_message(&self) -> Option<String> {
        // Try `error` key first (string or nested object with "message").
        if let Some(e) = self.raw.get("error") {
            if let Some(s) = e.as_str() {
                return Some(s.to_string());
            }
            if let Some(s) = e.get("message").and_then(|m| m.as_str()) {
                return Some(s.to_string());
            }
        }
        // Fall back to top-level `message` key (used by transient `error` events).
        self.raw
            .get("message")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }
}

// ---------------------------------------------------------------------------
// EventStream and RunHandle
// ---------------------------------------------------------------------------

/// Receiver end of the channel that carries [`CodexEvent`]s from the
/// background I/O thread to the orchestration layer.
///
/// The channel closes (returns `Err` on `recv()`) when the Codex process's
/// stdout is exhausted — either because it exited normally or was killed.
pub type EventStream = std::sync::mpsc::Receiver<CodexEvent>;

/// Handle to a running Codex process.
///
/// Cheaply [`Clone`]able — both the original and the clone refer to the same
/// underlying child process via an `Arc<Mutex<Option<Child>>>`.  This lets the
/// orchestration layer store one copy for cancellation while the background
/// I/O thread holds another for reaping.
#[derive(Clone)]
pub struct RunHandle {
    /// PID of the spawned `codex` process.
    pub pid: u32,
    child: std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>,
}

impl RunHandle {
    /// Kill the Codex process tree and reap the child process.
    ///
    /// **Windows** — runs `taskkill /F /T /PID <pid>` so all grandchildren
    /// are also terminated.  **Unix** — sends `SIGKILL` to the entire process
    /// group (`kill -9 -<pgid>`); the child is started with `process_group(0)`
    /// so its PGID equals its own PID.
    ///
    /// Safe to call from any thread; safe to call more than once (idempotent).
    pub fn cancel(&self) {
        kill_process_tree(self.pid);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Reap the child process after it has finished naturally (no kill).
    ///
    /// The background I/O thread calls this once stdout reaches EOF.
    pub fn wait(&self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.wait();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentRunner trait
// ---------------------------------------------------------------------------

/// Abstracts over agent execution backends.  v1 ships exactly one
/// implementation: [`CodexExecRunner`].
///
/// Both methods are synchronous — they spawn a background thread for I/O and
/// return immediately.  The caller receives a [`RunHandle`] for cancellation
/// and an [`EventStream`] to consume events.
pub trait AgentRunner: Send + Sync {
    /// Start the first turn for a task.
    ///
    /// `cwd` is the task's worktree directory.
    /// `log_path` is where raw stdout JSONL lines are written; its parent
    /// directory must exist (or will be created).
    fn start_turn(
        &self,
        cwd: &Path,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError>;

    /// Resume an existing Codex thread with a follow-up prompt.
    ///
    /// `thread_id` was captured from the `thread.started` event of the
    /// initial turn.
    fn resume_turn(
        &self,
        cwd: &Path,
        thread_id: &str,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError>;
}

// ---------------------------------------------------------------------------
// CodexExecRunner
// ---------------------------------------------------------------------------

/// Runs Codex by spawning `codex exec --json` as a child process and
/// streaming JSONL events from its stdout.
///
/// # Invocations
///
/// **Initial turn:**
/// ```text
/// codex exec --json --sandbox workspace-write --full-auto "<prompt>"
/// ```
///
/// **Resume turn (`resume_with_flags = true`, default):**
/// ```text
/// codex exec --json --sandbox workspace-write --full-auto resume <thread_id> "<prompt>"
/// ```
///
/// **Resume turn (`resume_with_flags = false`):**
/// ```text
/// codex exec resume <thread_id> "<prompt>"
/// ```
/// Set `resume_with_flags = false` if the pinned CLI build rejects
/// `--json`/`--sandbox`/`--full-auto` on `resume`.  See Phase IV handoff
/// notes in `IMPLEMENTATION_STATUS.md`.
#[derive(Debug, Clone)]
pub struct CodexExecRunner {
    /// Path or name of the `codex` binary.  Defaults to `"codex"` (PATH lookup).
    pub codex_bin: String,
    /// Whether to forward `--json --sandbox workspace-write --full-auto` on
    /// resume turns.  Default: `true`.  Set to `false` when the pinned CLI
    /// build rejects those flags on `resume`.
    pub resume_with_flags: bool,
}

impl Default for CodexExecRunner {
    fn default() -> Self {
        Self {
            codex_bin: "codex".to_string(),
            resume_with_flags: true,
        }
    }
}

impl CodexExecRunner {
    pub fn new(codex_bin: impl Into<String>) -> Self {
        Self {
            codex_bin: codex_bin.into(),
            ..Default::default()
        }
    }
}

impl AgentRunner for CodexExecRunner {
    fn start_turn(
        &self,
        cwd: &Path,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        spawn_codex(
            &self.codex_bin,
            &[
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--full-auto",
                prompt,
            ],
            cwd,
            log_path,
        )
    }

    fn resume_turn(
        &self,
        cwd: &Path,
        thread_id: &str,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        if self.resume_with_flags {
            spawn_codex(
                &self.codex_bin,
                &[
                    "exec",
                    "--json",
                    "--sandbox",
                    "workspace-write",
                    "--full-auto",
                    "resume",
                    thread_id,
                    prompt,
                ],
                cwd,
                log_path,
            )
        } else {
            // Fallback: omit flags that some CLI builds reject on resume.
            spawn_codex(
                &self.codex_bin,
                &["exec", "resume", thread_id, prompt],
                cwd,
                log_path,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

/// Result of the Codex binary health check.  Returned by [`health_check`]
/// and exposed to the Settings UI via the `check_codex_health` Tauri command.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    /// `true` if the binary was found and could be launched.
    pub binary_found: bool,
    /// Output of `codex --version`, trimmed.  `None` if not obtained.
    pub version: Option<String>,
    /// `true` if `CODEX_API_KEY` or `OPENAI_API_KEY` is set in the environment.
    pub auth_env_present: bool,
    /// Detail string (e.g. OS error) if the binary could not be launched.
    pub detail: Option<String>,
}

/// Check whether the `codex` binary is usable.
///
/// Runs `codex --version` — no network call, safe to invoke from a settings
/// page.  Also checks for common API-key environment variables as a proxy for
/// auth readiness (the real auth state cannot be checked without a live API
/// call, which would cost tokens).
pub fn health_check(codex_bin: &str) -> HealthStatus {
    let result = Command::new(codex_bin).arg("--version").output();

    let (binary_found, version, detail) = match result {
        Ok(out) if out.status.success() => {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (true, if v.is_empty() { None } else { Some(v) }, None)
        }
        Ok(out) => {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (true, None, if msg.is_empty() { None } else { Some(msg) })
        }
        Err(e) => (false, None, Some(e.to_string())),
    };

    let auth_env_present =
        std::env::var("CODEX_API_KEY").is_ok() || std::env::var("OPENAI_API_KEY").is_ok();

    HealthStatus {
        binary_found,
        version,
        auth_env_present,
        detail,
    }
}

// ---------------------------------------------------------------------------
// Internal process machinery
// ---------------------------------------------------------------------------

/// Core spawn helper — used by both [`AgentRunner`] methods.
fn spawn_codex(
    codex_bin: &str,
    args: &[&str],
    cwd: &Path,
    log_path: &Path,
) -> Result<(RunHandle, EventStream), AppError> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::InvalidOperation(format!(
                "Cannot create log directory '{}': {e}",
                parent.display()
            ))
        })?;
    }

    let mut cmd = Command::new(codex_bin);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    configure_child(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::InvalidOperation(format!("Failed to spawn '{codex_bin}': {e}")))?;

    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let child = Arc::new(Mutex::new(Some(child)));

    let (tx, rx) = std::sync::mpsc::channel::<CodexEvent>();
    spawn_io_threads(stdout, stderr, log_path.to_path_buf(), tx);

    Ok((RunHandle { pid, child }, rx))
}

/// Apply platform-specific flags to the child [`Command`].
///
/// * **Windows** — `CREATE_NO_WINDOW` suppresses the console window flash.
/// * **Unix** — `process_group(0)` places the child in its own process group
///   so `kill -9 -<pgid>` reaches all its grandchildren.
fn configure_child(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // 0x0800_0000 = CREATE_NO_WINDOW
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// Kill the process tree rooted at `pid`.
///
/// This is best-effort — failures are silently ignored so callers can proceed
/// with cleanup regardless of the process's current state.
fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        // /F = force  /T = include child processes  /PID = target PID
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // process_group(0) made the child its own group leader, so
        // pgid == pid.  Negating the PID sends the signal to the group.
        let _ = Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .status();
    }
}

/// Spawn two background threads:
///
/// 1. **Stdout thread** — reads lines from the Codex process's stdout,
///    writes every raw line to `log_path`, and sends parsed [`CodexEvent`]s
///    over `tx`.  When stdout reaches EOF, `tx` is dropped which closes the
///    [`EventStream`] on the receiver side.
///
/// 2. **Stderr thread** — reads and persists stderr to `<stem>.stderr`
///    alongside the JSONL log.  UI state is driven by stdout only; stderr
///    exists for debugging.
fn spawn_io_threads(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    log_path: std::path::PathBuf,
    tx: std::sync::mpsc::Sender<CodexEvent>,
) {
    // Derive the stderr log path from the stdout log path.
    let stderr_path = {
        let stem = log_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        log_path.with_file_name(format!("{stem}.stderr"))
    };

    // --- Stdout thread ---
    std::thread::spawn(move || {
        let mut log = std::fs::File::create(&log_path).ok();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Some(ref mut f) = log {
                let _ = writeln!(f, "{line}");
            }
            if let Some(event) = CodexEvent::parse(&line) {
                if tx.send(event).is_err() {
                    break; // receiver dropped — orchestration layer shut down
                }
            }
        }
        // `tx` drops here, closing the EventStream for the receiver.
    });

    // --- Stderr thread ---
    std::thread::spawn(move || {
        let mut log = std::fs::File::create(&stderr_path).ok();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(ref mut f) = log {
                let _ = writeln!(f, "{line}");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // CodexEvent parsing
    // -----------------------------------------------------------------------

    const THREAD_STARTED: &str = r#"{"type":"thread.started","thread_id":"thrd_abc123"}"#;
    const TURN_STARTED: &str = r#"{"type":"turn.started"}"#;
    const TURN_COMPLETED: &str =
        r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":50}}"#;
    const TURN_FAILED_STR: &str = r#"{"type":"turn.failed","error":"rate limit exceeded"}"#;
    const TURN_FAILED_OBJ: &str =
        r#"{"type":"turn.failed","error":{"message":"context too long"}}"#;
    const RECONNECTING: &str = r#"{"type":"error","message":"Reconnecting..."}"#;
    const ITEM_COMPLETED: &str =
        r#"{"type":"item.completed","item":{"type":"agent_message","content":"Done."}}"#;

    #[test]
    fn parse_thread_started() {
        let ev = CodexEvent::parse(THREAD_STARTED).expect("must parse");
        assert_eq!(ev.event_type, "thread.started");
        assert_eq!(ev.thread_id(), Some("thrd_abc123"));
        assert!(!ev.is_terminal());
    }

    #[test]
    fn parse_turn_started() {
        let ev = CodexEvent::parse(TURN_STARTED).expect("must parse");
        assert_eq!(ev.event_type, "turn.started");
        assert!(ev.thread_id().is_none());
        assert!(!ev.is_terminal());
    }

    #[test]
    fn parse_turn_completed() {
        let ev = CodexEvent::parse(TURN_COMPLETED).expect("must parse");
        assert_eq!(ev.event_type, "turn.completed");
        assert!(ev.is_terminal());
        assert!(ev.thread_id().is_none());
        assert!(ev.error_message().is_none());
    }

    #[test]
    fn parse_turn_failed_string_error() {
        let ev = CodexEvent::parse(TURN_FAILED_STR).expect("must parse");
        assert_eq!(ev.event_type, "turn.failed");
        assert!(ev.is_terminal());
        assert_eq!(ev.error_message().as_deref(), Some("rate limit exceeded"));
    }

    #[test]
    fn parse_turn_failed_object_error() {
        let ev = CodexEvent::parse(TURN_FAILED_OBJ).expect("must parse");
        assert!(ev.is_terminal());
        assert_eq!(ev.error_message().as_deref(), Some("context too long"));
    }

    #[test]
    fn parse_reconnecting_error_is_not_terminal() {
        let ev = CodexEvent::parse(RECONNECTING).expect("must parse");
        assert_eq!(ev.event_type, "error");
        assert!(
            !ev.is_terminal(),
            "transient reconnection errors are non-terminal"
        );
        assert_eq!(ev.error_message().as_deref(), Some("Reconnecting..."));
    }

    #[test]
    fn parse_item_event() {
        let ev = CodexEvent::parse(ITEM_COMPLETED).expect("must parse");
        assert_eq!(ev.event_type, "item.completed");
        assert!(!ev.is_terminal());
    }

    #[test]
    fn parse_returns_none_for_non_json() {
        assert!(CodexEvent::parse("not json").is_none());
        assert!(CodexEvent::parse("").is_none());
        assert!(CodexEvent::parse("   ").is_none());
    }

    #[test]
    fn parse_returns_none_for_json_without_type_field() {
        assert!(CodexEvent::parse(r#"{"thread_id":"x"}"#).is_none());
    }

    #[test]
    fn parse_strips_surrounding_whitespace() {
        let ev = CodexEvent::parse(&format!("\n  {THREAD_STARTED}  \n"));
        assert!(
            ev.is_some(),
            "whitespace-padded line must parse successfully"
        );
    }

    #[test]
    fn thread_id_returns_none_for_non_thread_started() {
        let ev = CodexEvent::parse(TURN_COMPLETED).unwrap();
        assert!(ev.thread_id().is_none());
    }

    // -----------------------------------------------------------------------
    // Health check
    // -----------------------------------------------------------------------

    #[test]
    fn health_check_missing_binary() {
        let s = health_check("/nonexistent/codex_binary_forge_test");
        assert!(!s.binary_found);
        assert!(s.version.is_none());
        assert!(s.detail.is_some(), "detail must explain the launch failure");
    }

    // -----------------------------------------------------------------------
    // CodexExecRunner defaults
    // -----------------------------------------------------------------------

    #[test]
    fn runner_default_settings() {
        let r = CodexExecRunner::default();
        assert_eq!(r.codex_bin, "codex");
        assert!(r.resume_with_flags, "flags must be enabled by default");
    }

    // -----------------------------------------------------------------------
    // I/O thread integration (Unix only — uses `sh` to emit fake JSONL)
    // -----------------------------------------------------------------------

    /// Spawn a shell that emits two fake JSONL events and verify they are
    /// received on the EventStream and written to the log file.
    #[cfg(unix)]
    #[test]
    fn io_threads_stream_and_log_events() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join("turn.jsonl");

        let payload = format!(
            r#"printf '{}\n{}\n'"#,
            THREAD_STARTED.replace('\'', r"\'"),
            TURN_COMPLETED.replace('\'', r"\'"),
        );

        let (handle, rx) =
            spawn_codex("sh", &["-c", &payload], tmp.path(), &log).expect("sh must be available");

        // Collect all events until the channel closes.
        let events: Vec<_> = rx.iter().collect();
        let _ = handle.wait();

        assert_eq!(events.len(), 2, "must receive exactly two events");
        assert_eq!(events[0].event_type, "thread.started");
        assert_eq!(events[0].thread_id(), Some("thrd_abc123"));
        assert_eq!(events[1].event_type, "turn.completed");
        assert!(events[1].is_terminal());

        let log_text = std::fs::read_to_string(&log).unwrap();
        assert!(
            log_text.contains("thread.started"),
            "log must contain raw stdout lines"
        );
        assert!(log_text.contains("turn.completed"));
    }

    /// Lines that are not valid JSONL (e.g. human-readable progress on stderr
    /// accidentally mixed into stdout) must be silently dropped.
    #[cfg(unix)]
    #[test]
    fn invalid_lines_are_silently_skipped() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join("turn.jsonl");

        // Mix valid events with garbage lines.
        let payload = format!(
            r#"printf 'not json\n{}\njunk line\n{}\n'"#,
            THREAD_STARTED.replace('\'', r"\'"),
            TURN_COMPLETED.replace('\'', r"\'"),
        );

        let (handle, rx) = spawn_codex("sh", &["-c", &payload], tmp.path(), &log).unwrap();
        let events: Vec<_> = rx.iter().collect();
        let _ = handle.wait();

        // Only the two valid JSON events must reach the receiver.
        assert_eq!(events.len(), 2);
    }
}
