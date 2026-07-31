const projects = [
  { initials: 'AW', label: 'Agent Workspace', active: true },
  { initials: '＋', label: 'Add project', active: false },
]

const channels = ['general', 'implementation', 'research']

export function App() {
  return (
    <main className="app-shell">
      <nav aria-label="Projects" className="project-rail">
        <div className="brand-mark" aria-label="Agent Workspace">
          A
        </div>
        <div className="project-list">
          {projects.map((project) => (
            <button
              aria-current={project.active ? 'page' : undefined}
              aria-label={project.label}
              className="project-button"
              data-active={project.active}
              key={project.label}
              type="button"
            >
              {project.initials}
            </button>
          ))}
        </div>
      </nav>

      <aside className="project-sidebar">
        <header className="project-heading">
          <span className="eyebrow">Project</span>
          <h1>Agent Workspace</h1>
        </header>
        <section aria-labelledby="channels-heading">
          <h2 id="channels-heading">Channels</h2>
          <ul>
            {channels.map((channel, index) => (
              <li key={channel}>
                <button data-selected={index === 0} type="button">
                  <span aria-hidden="true">#</span>
                  {channel}
                </button>
              </li>
            ))}
          </ul>
        </section>
      </aside>

      <section className="workspace" aria-labelledby="workspace-heading">
        <header className="workspace-heading">
          <div>
            <span className="eyebrow">Channel</span>
            <h2 id="workspace-heading"># general</h2>
          </div>
          <span className="status-chip">M0 foundation</span>
        </header>
        <div className="empty-state">
          <span className="empty-state-mark" aria-hidden="true">
            ◌
          </span>
          <h3>The workspace is ready for its first Thread.</h3>
          <p>
            M0 will connect this shell to durable conversation, isolated
            worktrees, and a hosted execution worker.
          </p>
        </div>
      </section>

      <aside className="context-drawer">
        <span className="eyebrow">Context</span>
        <h2>Project host</h2>
        <dl>
          <div>
            <dt>Milestone</dt>
            <dd>M0</dd>
          </div>
          <div>
            <dt>Provider</dt>
            <dd>Codex App Server</dd>
          </div>
          <div>
            <dt>Runtime</dt>
            <dd>Red Hat ARM64</dd>
          </div>
        </dl>
      </aside>
    </main>
  )
}
