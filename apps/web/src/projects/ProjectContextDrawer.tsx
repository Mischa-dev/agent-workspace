import type { Project } from './types'

export function ProjectContextDrawer({ project }: { project: Project }) {
  return (
    <aside aria-label="Project context" className="context-drawer">
      <span className="section-label">Project context</span>
      <h2>{project.name}</h2>
      <dl>
        <div>
          <dt>Key</dt>
          <dd>{project.key}</dd>
        </div>
        <div>
          <dt>Role</dt>
          <dd>{project.role || 'Member'}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd>{project.archivedAt ? 'Archived' : 'Active'}</dd>
        </div>
      </dl>
      <p className="context-note">
        Project access and actions are resolved from the signed-in Actor on the
        server.
      </p>
    </aside>
  )
}
