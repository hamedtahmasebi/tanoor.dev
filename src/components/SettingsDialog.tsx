import { useEffect, useState } from "react";
import { useStore } from "../store";
import type { UpdateSettingsInput } from "../types";

const DEFAULTS: UpdateSettingsInput = {
  codexBin: "codex",
  gitBin: "git",
  maxConcurrentTasks: 2,
  mergeOnConfirm: false,
};

function HealthBadge({ ready, label }: { ready: boolean; label: string }) {
  return <span className={`settings-health-badge${ready ? " ready" : " unavailable"}`}><i />{label}</span>;
}

export function SettingsDialog() {
  const {
    settings,
    systemHealth,
    isLoadingSettings,
    isSavingSettings,
    isCheckingHealth,
    error,
    closeSettings,
    saveSettings,
    refreshSystemHealth,
    clearError,
  } = useStore();
  const [draft, setDraft] = useState<UpdateSettingsInput>(DEFAULTS);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (settings) setDraft({
      codexBin: settings.codexBin,
      gitBin: settings.gitBin,
      maxConcurrentTasks: settings.maxConcurrentTasks,
      mergeOnConfirm: settings.mergeOnConfirm,
    });
  }, [settings]);

  const valid = draft.codexBin.trim().length > 0
    && draft.gitBin.trim().length > 0
    && Number.isInteger(draft.maxConcurrentTasks)
    && draft.maxConcurrentTasks >= 1
    && draft.maxConcurrentTasks <= 16;

  const submit = async () => {
    setSaved(false);
    try {
      await saveSettings(draft);
      setSaved(true);
    } catch {
      // The store keeps the actionable backend validation message visible.
    }
  };

  return (
    <div className="settings-overlay" role="presentation" onClick={(event) => { if (event.target === event.currentTarget) closeSettings(); }}>
      <section className="settings-dialog" role="dialog" aria-modal="true" aria-label="Settings">
        <header className="settings-header"><div><span>Tanoor</span><h1>Settings</h1></div><button type="button" aria-label="Close settings" onClick={closeSettings}>×</button></header>
        {error && <div className="review-dialog-error" role="alert"><span>{error}</span><button type="button" onClick={clearError}>Dismiss</button></div>}
        {isLoadingSettings || !settings ? <div className="settings-loading"><span className="loading-spinner" /> Loading settings</div> : (
          <div className="settings-content">
            <section className="settings-section">
              <div className="settings-section-heading"><div><h2>Command-line tools</h2><p>Overrides apply to the next task, follow-up, project check, or approval.</p></div><button type="button" className="settings-refresh" disabled={isCheckingHealth} onClick={() => void refreshSystemHealth()}>{isCheckingHealth ? "Checking…" : "Check status"}</button></div>
              <label className="settings-field"><span>Codex binary</span><input value={draft.codexBin} spellCheck={false} onChange={(event) => { setSaved(false); setDraft((value) => ({ ...value, codexBin: event.target.value })); }} /><small>Executable name on PATH or an absolute file path.</small></label>
              <div className="settings-health-row">
                <HealthBadge ready={systemHealth?.codex.binaryFound ?? false} label={systemHealth?.codex.binaryFound ? "Codex found" : "Codex unavailable"} />
                <code>{systemHealth?.codex.version ?? systemHealth?.codex.detail ?? "Not checked"}</code>
              </div>
              <div className="settings-auth-row"><span>Authentication</span><strong className={`auth-${systemHealth?.codex.authStatus ?? "unknown"}`}>{(systemHealth?.codex.authStatus ?? "unknown").replace("_", " ")}</strong><small>{systemHealth?.codex.authDetail ?? "No authentication status available."}</small></div>
              <label className="settings-field"><span>Git binary</span><input value={draft.gitBin} spellCheck={false} onChange={(event) => { setSaved(false); setDraft((value) => ({ ...value, gitBin: event.target.value })); }} /><small>Used for repository checks, worktrees, diffs, commits, and merges.</small></label>
              <div className="settings-health-row">
                <HealthBadge ready={systemHealth?.git.binaryFound ?? false} label={systemHealth?.git.binaryFound ? "Git found" : "Git unavailable"} />
                <code>{systemHealth?.git.version ?? systemHealth?.git.detail ?? "Not checked"}</code>
              </div>
            </section>

            <section className="settings-section">
              <div className="settings-section-heading"><div><h2>Execution</h2><p>Active tasks are never cancelled when these values change.</p></div></div>
              <label className="settings-field settings-number"><span>Maximum concurrent tasks</span><input type="number" min={1} max={16} step={1} value={draft.maxConcurrentTasks} onChange={(event) => { setSaved(false); setDraft((value) => ({ ...value, maxConcurrentTasks: Number(event.target.value) })); }} /><small>Between 1 and 16. A lower limit applies as soon as running tasks finish.</small></label>
              <label className="settings-toggle"><input type="checkbox" checked={draft.mergeOnConfirm} onChange={(event) => { setSaved(false); setDraft((value) => ({ ...value, mergeOnConfirm: event.target.checked })); }} /><span><strong>Merge on confirm by default</strong><small>The approval dialog can still override this for each task.</small></span></label>
              <div className="settings-readonly"><div><span>Sandbox policy</span><code>workspace-write</code></div><small>Fixed for Tanoor v1. Tasks can write inside their isolated worktree; this policy cannot be weakened here.</small></div>
            </section>
          </div>
        )}
        <footer className="settings-footer"><span>{saved ? "Settings saved. No restart required." : "Settings persist across launches."}</span><button type="button" disabled={isSavingSettings || !valid} onClick={() => void submit()}>{isSavingSettings ? "Saving…" : "Save settings"}</button></footer>
      </section>
    </div>
  );
}
