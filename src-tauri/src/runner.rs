//! Agent runners — isolate CLI process and protocol details.
//!
//! # Public API
//!
//! * [`AgentRunner`] — trait implemented by every agent backend (v1 has one).
//! * [`CodexExecRunner`] — spawns `codex exec --json` and streams JSONL events.
//! * [`AgentEvent`] — normalized event from any agent stdout JSONL stream.
//! * [`CodexEvent`] — Codex-specific parser retained for protocol tests.
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
    time::{Duration, Instant},
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

/// Semantic event shared by all supported coding-agent adapters.
///
/// The original protocol event is intentionally retained in `raw`, allowing
/// the UI to evolve without coupling the orchestration layer to a vendor's
/// JSON schema.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentEvent {
    pub event_type: String,
    pub raw: serde_json::Value,
    pub kind: AgentEventKind,
    pub thread_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AgentEventKind {
    ThreadStarted,
    TurnCompleted,
    TurnFailed,
    Other,
}

impl AgentEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind,
            AgentEventKind::TurnCompleted | AgentEventKind::TurnFailed
        )
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn error_message(&self) -> Option<String> {
        self.error.clone()
    }
}

impl From<CodexEvent> for AgentEvent {
    fn from(event: CodexEvent) -> Self {
        let thread_id = event.thread_id().map(str::to_string);
        let error = event.error_message();
        let kind = match event.event_type.as_str() {
            "thread.started" => AgentEventKind::ThreadStarted,
            "turn.completed" => AgentEventKind::TurnCompleted,
            "turn.failed" => AgentEventKind::TurnFailed,
            _ => AgentEventKind::Other,
        };
        Self {
            event_type: event.event_type,
            thread_id,
            error,
            raw: event.raw,
            kind,
        }
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
pub type EventStream = std::sync::mpsc::Receiver<AgentEvent>;

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
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
    stderr_thread: std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

/// Process details collected after Codex exits.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub exit_code: Option<i32>,
    pub stderr: String,
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
        self.join_stderr_thread();
    }

    /// Reap the child process after it has finished naturally (no kill).
    ///
    /// The orchestration worker calls this once stdout reaches EOF.
    pub fn wait(&self) -> RunOutcome {
        let exit_code = if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                child.wait().ok().and_then(|status| status.code())
            } else {
                None
            }
        } else {
            None
        };
        self.finish(exit_code)
    }

    /// Reap the child, but only wait up to `grace` for it to exit on its own.
    ///
    /// Called once stdout reaches EOF. A well-behaved agent exits immediately
    /// after closing stdout, so the grace is rarely used; if the process is
    /// stuck after its output ended, the tree is killed so the turn fails
    /// cleanly instead of hanging in `running` forever.
    pub fn wait_bounded(&self, grace: Duration) -> RunOutcome {
        let deadline = Instant::now() + grace;
        loop {
            {
                let mut guard = match self.child.lock() {
                    Ok(guard) => guard,
                    Err(_) => return self.finish(None),
                };
                let Some(child) = guard.as_mut() else {
                    return self.finish(None);
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code();
                        *guard = None;
                        return self.finish(code);
                    }
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            drop(guard);
                            kill_process_tree(self.pid);
                            let code = self.child.lock().ok().and_then(|mut guard| {
                                guard.take().and_then(|mut child| {
                                    let _ = child.kill();
                                    child.wait().ok().and_then(|status| status.code())
                                })
                            });
                            return self.finish(code);
                        }
                    }
                    Err(_) => {
                        *guard = None;
                        return self.finish(None);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn finish(&self, exit_code: Option<i32>) -> RunOutcome {
        self.join_stderr_thread();
        let stderr = self
            .stderr
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        RunOutcome { exit_code, stderr }
    }

    fn join_stderr_thread(&self) {
        if let Ok(mut guard) = self.stderr_thread.lock() {
            if let Some(thread) = guard.take() {
                let _ = thread.join();
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
    /// Stable identifier used for persistence and UI selection.
    fn agent_id(&self) -> &'static str;

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
/// codex exec --json --sandbox workspace-write "<prompt>"
/// ```
///
/// **Resume turn (`resume_with_flags = true`, default):**
/// ```text
/// codex exec --json --sandbox workspace-write resume <thread_id> "<prompt>"
/// ```
///
/// **Resume turn (`resume_with_flags = false`):**
/// ```text
/// codex exec resume <thread_id> "<prompt>"
/// ```
/// Set `resume_with_flags = false` if the pinned CLI build rejects
/// `--json`/`--sandbox` on `resume`.  See Phase IV handoff
/// notes in `IMPLEMENTATION_STATUS.md`.
#[derive(Debug, Clone)]
pub struct CodexExecRunner {
    /// Path or name of the `codex` binary.  Defaults to `"codex"` (PATH lookup).
    pub codex_bin: String,
    /// Whether to forward `--json --sandbox workspace-write` on
    /// resume turns.  Default: `true`.  Set to `false` when the pinned CLI
    /// build rejects those flags on `resume`.
    pub resume_with_flags: bool,
    /// The model ID to pass via `--model`.  Defaults to `"o4-mini"`.
    pub model: String,
    /// The effort level to pass via `--effort` for o-series models.
    /// `None` suppresses the flag (e.g. for GPT-4 class models).
    pub effort: Option<String>,
}

impl Default for CodexExecRunner {
    fn default() -> Self {
        Self {
            codex_bin: "codex".to_string(),
            resume_with_flags: true,
            model: "o4-mini".to_string(),
            effort: Some("medium".to_string()),
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

    /// Build the CLI args for starting a brand-new Codex thread.
    fn initial_args<'a>(&'a self, prompt: &'a str) -> Vec<&'a str> {
        let mut args = vec!["exec", "--json", "--sandbox", "workspace-write"];
        args.extend(["--model", &self.model]);
        if let Some(effort) = &self.effort {
            if self.model.starts_with('o') {
                args.extend(["--effort", effort.as_str()]);
            }
        }
        args.push(prompt);
        args
    }

    /// Build the CLI args for resuming an existing Codex thread.
    fn resume_args<'a>(&'a self, thread_id: &'a str, prompt: &'a str) -> Vec<&'a str> {
        if self.resume_with_flags {
            let mut args = vec!["exec", "--json", "--sandbox", "workspace-write"];
            args.extend(["--model", &self.model]);
            if let Some(effort) = &self.effort {
                if self.model.starts_with('o') {
                    args.extend(["--effort", effort.as_str()]);
                }
            }
            args.extend(["resume", thread_id, prompt]);
            args
        } else {
            vec!["exec", "resume", thread_id, prompt]
        }
    }
}

impl AgentRunner for CodexExecRunner {
    fn agent_id(&self) -> &'static str {
        "codex"
    }

    fn start_turn(
        &self,
        cwd: &Path,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        spawn_codex(&self.codex_bin, &self.initial_args(prompt), cwd, log_path)
    }

    fn resume_turn(
        &self,
        cwd: &Path,
        thread_id: &str,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        spawn_codex(
            &self.codex_bin,
            &self.resume_args(thread_id, prompt),
            cwd,
            log_path,
        )
    }
}

/// Claude Code CLI adapter. Claude's stream-json protocol is normalized to
/// `AgentEvent`; unsupported protocol records remain visible as `Other`.
#[derive(Debug, Clone)]
pub struct ClaudeExecRunner {
    pub claude_bin: String,
    pub model: String,
}

impl ClaudeExecRunner {
    pub fn initial_args(&self, prompt: &str) -> Vec<String> {
        vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--model".into(),
            self.model.clone(),
        ]
    }

    pub fn resume_args(&self, session_id: &str, prompt: &str) -> Vec<String> {
        let mut args = vec!["--resume".into(), session_id.into()];
        args.extend(self.initial_args(prompt));
        args
    }

    pub fn parse_event(line: &str) -> Option<AgentEvent> {
        let raw: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let event_type = raw.get("type")?.as_str()?.to_string();
        let subtype = raw.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        let session_id = raw
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let kind = if event_type == "system" && subtype == "init" {
            AgentEventKind::ThreadStarted
        } else if event_type == "result"
            && raw.get("is_error").and_then(|v| v.as_bool()) == Some(true)
        {
            AgentEventKind::TurnFailed
        } else if event_type == "result" {
            AgentEventKind::TurnCompleted
        } else {
            AgentEventKind::Other
        };
        let error = raw
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| {
                raw.get("result")
                    .and_then(|v| v.as_str())
                    .filter(|_| matches!(kind, AgentEventKind::TurnFailed))
            })
            .map(str::to_string);
        Some(AgentEvent {
            event_type,
            raw,
            kind,
            thread_id: session_id,
            error,
        })
    }
}

impl AgentRunner for ClaudeExecRunner {
    fn agent_id(&self) -> &'static str {
        "claude"
    }

    fn start_turn(
        &self,
        cwd: &Path,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        let args = self.initial_args(prompt);
        spawn_process(&self.claude_bin, &args, cwd, log_path, Self::parse_event)
    }

    fn resume_turn(
        &self,
        cwd: &Path,
        thread_id: &str,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        let args = self.resume_args(thread_id, prompt);
        spawn_process(&self.claude_bin, &args, cwd, log_path, Self::parse_event)
    }
}

/// opencode CLI adapter. Session identifiers are treated the same as thread
/// identifiers by the orchestration layer.
#[derive(Debug, Clone)]
pub struct OpenCodeExecRunner {
    pub opencode_bin: String,
    pub model: String,
}

impl OpenCodeExecRunner {
    pub fn initial_args(&self, prompt: &str) -> Vec<String> {
        vec![
            "run".into(),
            "--format".into(),
            "json".into(),
            "--model".into(),
            self.model.clone(),
            prompt.into(),
        ]
    }

    pub fn resume_args(&self, session_id: &str, prompt: &str) -> Vec<String> {
        vec![
            "run".into(),
            "--format".into(),
            "json".into(),
            "--session".into(),
            session_id.into(),
            "--model".into(),
            self.model.clone(),
            prompt.into(),
        ]
    }

    pub fn parse_event(line: &str) -> Option<AgentEvent> {
        let raw: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let event_type = raw
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("message")
            .to_string();
        let session_id = raw
            .get("session_id")
            .or_else(|| raw.get("sessionID"))
            .or_else(|| raw.get("sessionId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let reason = raw
            .get("part")
            .and_then(|part| part.get("reason"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let kind = match event_type.as_str() {
            "session.started" | "session.created" => AgentEventKind::ThreadStarted,
            "result" | "done" | "turn.completed" => {
                if raw.get("error").is_some() {
                    AgentEventKind::TurnFailed
                } else {
                    AgentEventKind::TurnCompleted
                }
            }
            "step_finish"
                if matches!(
                    reason,
                    "stop" | "end_turn" | "completed" | "length" | "max_tokens"
                ) =>
            {
                AgentEventKind::TurnCompleted
            }
            "error" | "turn.failed" | "session.error" => AgentEventKind::TurnFailed,
            _ => AgentEventKind::Other,
        };
        let error = raw
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| {
                raw.get("part")
                    .and_then(|part| part.get("error"))
                    .and_then(|value| value.as_str())
            })
            .map(str::to_string);
        Some(AgentEvent {
            event_type,
            raw,
            kind,
            thread_id: session_id,
            error,
        })
    }
}

impl AgentRunner for OpenCodeExecRunner {
    fn agent_id(&self) -> &'static str {
        "opencode"
    }

    fn start_turn(
        &self,
        cwd: &Path,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        let args = self.initial_args(prompt);
        spawn_process(&self.opencode_bin, &args, cwd, log_path, Self::parse_event)
    }

    fn resume_turn(
        &self,
        cwd: &Path,
        thread_id: &str,
        prompt: &str,
        log_path: &Path,
    ) -> Result<(RunHandle, EventStream), AppError> {
        let args = self.resume_args(thread_id, prompt);
        spawn_process(&self.opencode_bin, &args, cwd, log_path, Self::parse_event)
    }
}

/// Build a concrete runner from persisted/user-selected agent settings.
pub fn build_runner(
    agent_id: &str,
    binary: String,
    model: String,
    effort: Option<String>,
) -> Result<std::sync::Arc<dyn AgentRunner>, AppError> {
    match agent_id {
        "codex" => Ok(std::sync::Arc::new(CodexExecRunner {
            codex_bin: binary,
            model,
            effort,
            ..Default::default()
        })),
        "claude" => Ok(std::sync::Arc::new(ClaudeExecRunner {
            claude_bin: binary,
            model,
        })),
        "opencode" => Ok(std::sync::Arc::new(OpenCodeExecRunner {
            opencode_bin: binary,
            model,
        })),
        other => Err(AppError::InvalidOperation(format!(
            "Unsupported agent '{other}'"
        ))),
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
    /// `authenticated`, `not_authenticated`, or `unknown`.
    pub auth_status: String,
    /// Human-readable output from `codex login status`, when available.
    pub auth_detail: Option<String>,
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

    let (auth_status, auth_detail) = if binary_found {
        match Command::new(codex_bin).args(["login", "status"]).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let message = if stdout.is_empty() { stderr } else { stdout };
                (
                    if out.status.success() {
                        "authenticated"
                    } else if auth_env_present {
                        "unknown"
                    } else {
                        "not_authenticated"
                    }
                    .to_string(),
                    if auth_env_present && !out.status.success() {
                        Some(if message.is_empty() {
                            "An API-key environment variable is present, but Codex did not confirm an active login".to_string()
                        } else {
                            format!(
                                "API-key environment variable present; Codex reports: {message}"
                            )
                        })
                    } else if message.is_empty() {
                        None
                    } else {
                        Some(message)
                    },
                )
            }
            Err(error) => ("unknown".to_string(), Some(error.to_string())),
        }
    } else if auth_env_present {
        (
            "unknown".to_string(),
            Some(
                "An API-key environment variable is present, but the Codex binary is unavailable"
                    .to_string(),
            ),
        )
    } else {
        ("unknown".to_string(), None)
    };

    HealthStatus {
        binary_found,
        version,
        auth_env_present,
        auth_status,
        auth_detail,
        detail,
    }
}

/// Generic binary availability check for agents whose CLI does not expose a
/// Codex-compatible login-status command.
pub fn binary_health_check(binary: &str) -> HealthStatus {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => HealthStatus {
            binary_found: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            auth_env_present: false,
            auth_status: "unknown".to_string(),
            auth_detail: None,
            detail: None,
        },
        Ok(output) => HealthStatus {
            binary_found: true,
            version: None,
            auth_env_present: false,
            auth_status: "unknown".to_string(),
            auth_detail: None,
            detail: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        },
        Err(error) => HealthStatus {
            binary_found: false,
            version: None,
            auth_env_present: false,
            auth_status: "unknown".to_string(),
            auth_detail: None,
            detail: Some(error.to_string()),
        },
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
    spawn_process(codex_bin, args, cwd, log_path, |line| {
        CodexEvent::parse(line).map(AgentEvent::from)
    })
}

fn spawn_process(
    binary: &str,
    args: &[impl AsRef<str>],
    cwd: &Path,
    log_path: &Path,
    parser: fn(&str) -> Option<AgentEvent>,
) -> Result<(RunHandle, EventStream), AppError> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::InvalidOperation(format!(
                "Cannot create log directory '{}': {e}",
                parent.display()
            ))
        })?;
    }

    let mut cmd = Command::new(binary);
    cmd.args(args.iter().map(AsRef::as_ref))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    configure_child(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::InvalidOperation(format!("Failed to spawn '{binary}': {e}")))?;

    let pid = child.id();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let child = Arc::new(Mutex::new(Some(child)));

    let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
    let stderr_output = Arc::new(Mutex::new(String::new()));
    let stderr_thread = spawn_io_threads(
        stdout,
        stderr,
        log_path.to_path_buf(),
        stderr_output.clone(),
        tx,
        parser,
    );

    Ok((
        RunHandle {
            pid,
            child,
            stderr: stderr_output,
            stderr_thread: Arc::new(Mutex::new(Some(stderr_thread))),
        },
        rx,
    ))
}

/// Apply platform-specific flags to the child [`Command`].
///
/// * **Windows** — `CREATE_NO_WINDOW` suppresses the console window flash.
/// * **Unix** — `process_group(0)` places the child in its own process group
///   so `kill -9 -<pgid>` reaches all its grandchildren.
pub(crate) fn configure_child(cmd: &mut Command) {
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
///    alongside the JSONL log, and captures it for actionable process errors.
fn spawn_io_threads(
    stdout: impl std::io::Read + Send + 'static,
    stderr: impl std::io::Read + Send + 'static,
    log_path: std::path::PathBuf,
    stderr_output: Arc<Mutex<String>>,
    tx: std::sync::mpsc::Sender<AgentEvent>,
    parser: fn(&str) -> Option<AgentEvent>,
) -> std::thread::JoinHandle<()> {
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
        read_lines_lossy(stdout, |line| {
            if let Some(ref mut f) = log {
                let _ = writeln!(f, "{line}");
            }
            if let Some(event) = parser(line) {
                if tx.send(event).is_err() {
                    return false; // receiver dropped — orchestration layer shut down
                }
            }
            true
        });
        // `tx` drops here, closing the EventStream for the receiver.
    });

    // --- Stderr thread ---
    std::thread::spawn(move || {
        let mut log = std::fs::File::create(&stderr_path).ok();
        let mut captured = String::new();
        read_lines_lossy(stderr, |line| {
            if let Some(ref mut f) = log {
                let _ = writeln!(f, "{line}");
            }
            if !captured.is_empty() {
                captured.push('\n');
            }
            captured.push_str(line);
            true
        });
        if let Ok(mut output) = stderr_output.lock() {
            *output = captured;
        }
    })
}

/// Read a byte stream line by line, decoding each line with
/// [`String::from_utf8_lossy`] and invoking `on_line` for every line.
///
/// Unlike [`BufRead::lines`], this never aborts the stream on invalid UTF-8 or
/// a transient read error: bad bytes become replacement characters and reading
/// continues to true EOF. Draining the pipe to EOF is essential — if the reader
/// stopped early, the child would block on a full stdout pipe and never exit,
/// hanging the turn. `on_line` returns `false` to stop early (receiver gone).
fn read_lines_lossy(reader: impl std::io::Read, mut on_line: impl FnMut(&str) -> bool) {
    let mut reader = BufReader::new(reader);
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) => break, // EOF
            Ok(_) => {
                while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                    bytes.pop();
                }
                if !on_line(&String::from_utf8_lossy(&bytes)) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Line reader resilience
    // -----------------------------------------------------------------------

    #[test]
    fn reader_survives_invalid_utf8_and_reads_every_line() {
        // A stream where the middle line carries an invalid UTF-8 byte (0xFF).
        // The old `.lines()` reader aborted here, silently dropping every
        // following line and hanging the turn; this reader must keep going.
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(br#"{"type":"a"}"#);
        data.push(b'\n');
        data.extend_from_slice(br#"{"type":"b","x":""#);
        data.push(0xFF);
        data.extend_from_slice(br#""}"#);
        data.push(b'\n');
        data.extend_from_slice(br#"{"type":"c"}"#); // final line, no trailing newline

        let mut lines = Vec::new();
        read_lines_lossy(std::io::Cursor::new(data), |line| {
            lines.push(line.to_string());
            true
        });

        assert_eq!(lines.len(), 3, "all lines must be read past the bad byte");
        assert!(lines[0].contains("\"a\""));
        assert!(lines[1].contains("\"b\""));
        assert!(
            lines[1].contains('\u{FFFD}'),
            "bad byte becomes replacement char"
        );
        assert!(lines[2].contains("\"c\""), "reading continues to EOF");
    }

    #[test]
    fn reader_stops_when_consumer_requests_it() {
        let data = b"one\ntwo\nthree\n";
        let mut lines = Vec::new();
        read_lines_lossy(std::io::Cursor::new(&data[..]), |line| {
            lines.push(line.to_string());
            line != "two" // stop after receiving "two"
        });
        assert_eq!(lines, vec!["one", "two"]);
    }

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
        let s = health_check("/nonexistent/codex_binary_tanoor_test");
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
        assert_eq!(r.model, "o4-mini");
        assert_eq!(r.effort.as_deref(), Some("medium"));
    }

    #[test]
    fn claude_events_normalize_session_and_result() {
        let started = ClaudeExecRunner::parse_event(
            r#"{"type":"system","subtype":"init","session_id":"session-1"}"#,
        )
        .unwrap();
        assert_eq!(started.kind, AgentEventKind::ThreadStarted);
        assert_eq!(started.thread_id(), Some("session-1"));

        let completed = ClaudeExecRunner::parse_event(
            r#"{"type":"result","subtype":"success","session_id":"session-1"}"#,
        )
        .unwrap();
        assert_eq!(completed.kind, AgentEventKind::TurnCompleted);
        assert!(completed.is_terminal());
    }

    #[test]
    fn agent_argument_builders_keep_prompts_as_single_arguments() {
        let claude = ClaudeExecRunner {
            claude_bin: "claude".to_string(),
            model: "sonnet".to_string(),
        };
        assert_eq!(claude.initial_args("make it work")[1], "make it work");

        let opencode = OpenCodeExecRunner {
            opencode_bin: "opencode".to_string(),
            model: "openai/gpt-5".to_string(),
        };
        assert_eq!(
            opencode.resume_args("session-1", "fix tests")[4],
            "session-1"
        );
    }

    #[test]
    fn opencode_step_finish_only_ends_on_terminal_reason() {
        let intermediate = OpenCodeExecRunner::parse_event(
            r#"{"type":"step_finish","sessionID":"session-1","part":{"type":"step-finish","reason":"tool-calls"}}"#,
        )
        .unwrap();
        assert!(!intermediate.is_terminal());
        assert_eq!(intermediate.thread_id(), Some("session-1"));

        let completed = OpenCodeExecRunner::parse_event(
            r#"{"type":"step_finish","sessionID":"session-1","part":{"type":"step-finish","reason":"stop"}}"#,
        )
        .unwrap();
        assert_eq!(completed.kind, AgentEventKind::TurnCompleted);
        assert!(completed.is_terminal());
    }

    #[test]
    fn current_cli_arguments_do_not_use_full_auto() {
        let r = CodexExecRunner::default();
        assert_eq!(
            r.initial_args("prompt"),
            [
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--model",
                "o4-mini",
                "--effort",
                "medium",
                "prompt",
            ]
        );
        assert_eq!(
            r.resume_args("thread-id", "prompt"),
            [
                "exec",
                "--json",
                "--sandbox",
                "workspace-write",
                "--model",
                "o4-mini",
                "--effort",
                "medium",
                "resume",
                "thread-id",
                "prompt",
            ]
        );
    }

    #[test]
    fn process_failure_captures_stderr() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let log = tmp.path().join("turn.jsonl");
        let current_exe = std::env::current_exe().unwrap();
        let (handle, events) = spawn_codex(
            current_exe.to_str().unwrap(),
            &["--definitely-not-a-valid-libtest-argument"],
            tmp.path(),
            &log,
        )
        .unwrap();

        assert_eq!(events.iter().count(), 0);
        let outcome = handle.wait();
        assert_ne!(outcome.exit_code, Some(0));
        assert!(!outcome.stderr.is_empty());
        assert_eq!(
            outcome.stderr,
            std::fs::read_to_string(log.with_extension("stderr"))
                .unwrap()
                .trim_end()
        );
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
