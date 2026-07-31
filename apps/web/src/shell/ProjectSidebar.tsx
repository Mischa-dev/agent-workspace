import { NavLink } from 'react-router-dom'

import type { Project } from '../projects/types'

export function ProjectSidebar({
  mobileOpen,
  onNavigate,
  project,
  thread,
}: {
  mobileOpen: boolean
  onNavigate: () => void
  project: Project
  thread?: { id: string; name: string } | null
}) {
  return (
    <aside
      aria-label={`${project.name} navigation`}
      className="project-sidebar"
      data-mobile-open={mobileOpen}
      id="project-sidebar"
    >
      <header className="project-heading">
        <span className="section-label">{project.key}</span>
        <p>{project.name}</p>
      </header>
      <nav aria-label="Project">
        <ul>
          <li>
            <NavLink
              end
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}`}
            >
              <span aria-hidden="true">⌂</span>
              Overview
            </NavLink>
          </li>
          {thread && (
            <li>
              <NavLink
                className="thread-link"
                onClick={onNavigate}
                to={`/projects/${encodeURIComponent(project.id)}/threads/${encodeURIComponent(thread.id)}`}
              >
                <span aria-hidden="true">↳</span>
                {thread.name}
              </NavLink>
            </li>
          )}
          <li>
            <NavLink
              onClick={onNavigate}
              to={`/projects/${encodeURIComponent(project.id)}/settings`}
            >
              <span aria-hidden="true">⚙</span>
              Settings
            </NavLink>
          </li>
        </ul>
      </nav>
    </aside>
  )
}
