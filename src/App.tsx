type NavItem = {
  label: string;
  icon: string;
  count?: number;
  active?: boolean;
};

const navItems: NavItem[] = [
  { label: "All tasks", icon: "▦", count: 0, active: true },
  { label: "Needs review", icon: "◌", count: 0 },
  { label: "In progress", icon: "↻", count: 0 },
  { label: "Approved", icon: "✓", count: 0 },
];

function App() {
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <div>
            <p className="brand-name">Forge</p>
            <p className="brand-subtitle">AI workflow automation</p>
          </div>
        </div>

        <div className="project-switcher">
          <span className="project-icon" aria-hidden="true">⌂</span>
          <div className="project-copy">
            <span className="eyebrow">Current project</span>
            <span className="project-name">No project selected</span>
          </div>
          <span className="chevron" aria-hidden="true">⌄</span>
        </div>

        <nav className="primary-nav" aria-label="Task views">
          <span className="nav-heading">Workspace</span>
          {navItems.map((item) => (
            <button className={`nav-item${item.active ? " active" : ""}`} key={item.label} type="button">
              <span className="nav-icon" aria-hidden="true">{item.icon}</span>
              <span>{item.label}</span>
              <span className="nav-count">{item.count}</span>
            </button>
          ))}
        </nav>

        <div className="sidebar-footer">
          <button className="footer-link" type="button">
            <span aria-hidden="true">⚙</span>
            Settings
          </button>
          <div className="agent-status">
            <span className="status-dot" />
            <span>Codex agent</span>
            <span className="status-label">Not connected</span>
          </div>
        </div>
      </aside>

      <main className="main-panel">
        <header className="topbar">
          <div>
            <p className="breadcrumb">Workspace <span>/</span> Tasks</p>
            <h1>All tasks</h1>
          </div>
          <div className="topbar-actions">
            <button className="icon-button" type="button" aria-label="Search tasks">⌕</button>
            <button className="help-button" type="button" aria-label="Help">?</button>
            <button className="avatar" type="button" aria-label="Account">HM</button>
          </div>
        </header>

        <section className="content-area">
          <div className="page-intro">
            <div>
              <p className="section-kicker">Task queue</p>
              <h2>Build with a clear next step.</h2>
              <p className="intro-copy">Create a task, give Codex the right context, and review every change before it lands.</p>
            </div>
            <button className="primary-button" type="button" disabled title="Task creation arrives in Phase II">
              <span aria-hidden="true">＋</span>
              New task
            </button>
          </div>

          <div className="empty-state" role="status">
            <div className="empty-illustration" aria-hidden="true">
              <div className="paper-back" />
              <div className="paper-front">
                <span />
                <span />
                <span />
              </div>
              <div className="sparkle sparkle-one">✦</div>
              <div className="sparkle sparkle-two">✦</div>
            </div>
            <h3>Your task queue is empty</h3>
            <p>Select a project to get started. Task creation and project setup will be available in Phase II.</p>
            <button className="secondary-button" type="button" disabled>
              Select a project
            </button>
          </div>

          <div className="build-note">
            <span className="note-icon" aria-hidden="true">✦</span>
            <div>
              <strong>Phase I scaffold is ready</strong>
              <p>The shell, navigation, and secure command capability boundary are in place.</p>
            </div>
            <span className="note-tag">v0.1</span>
          </div>
        </section>
      </main>
    </div>
  );
}

export default App;
