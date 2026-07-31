import { useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useProjects, useSwitchProject } from './hooks'
import { projectLocationHref } from './locations'
import type { Project } from './types'

export function HomePage() {
  const session = useAuthenticatedSession()
  const projects = useProjects()
  const switchProject = useSwitchProject()
  const navigate = useNavigate()
  const location = useLocation()
  const [switchError, setSwitchError] = useState<string | null>(null)
  const notice =
    location.state &&
    typeof location.state === 'object' &&
    'notice' in location.state &&
    typeof location.state.notice === 'string'
      ? location.state.notice
      : null
  const activeProjects = (projects.data?.projects ?? []).filter(
    (project) => !project.archivedAt,
  )
  const recentProjects = [...activeProjects]
    .sort((left, right) => {
      const leftTime = left.navigation.lastOpenedAt
        ? Date.parse(left.navigation.lastOpenedAt)
        : 0
      const rightTime = right.navigation.lastOpenedAt
        ? Date.parse(right.navigation.lastOpenedAt)
        : 0
      return rightTime - leftTime
    })
    .slice(0, 8)

  const openProject = async (
    project: Project,
    event: React.MouseEvent<HTMLAnchorElement>,
  ) => {
    if (
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return
    }
    event.preventDefault()
    setSwitchError(null)
    try {
      const result = await switchProject.mutateAsync({
        projectId: project.id,
        expectedVersion: project.navigation.version,
        navigation: project.navigation,
      })
      navigate(result.href)
    } catch (reason) {
      setSwitchError(
        isApiError(reason)
          ? reason.message
          : 'The Project location could not be restored. Try again.',
      )
    }
  }

  return (
    <AppShell user={session.user}>
      <div className="page-content">
        <header className="page-heading">
          <div>
            <span className="section-label">Global Home</span>
            <h1>Welcome back, {session.user.displayName}</h1>
            <p>Open a recent Project or start a contained workspace.</p>
          </div>
          {session.user.capabilities.createProject && (
            <Link className="primary-link" to="/projects/new">
              Create Project
            </Link>
          )}
        </header>
        {notice && (
          <p className="notice" role="status">
            {notice}
          </p>
        )}
        {switchError && (
          <p className="inline-error" role="alert">
            {switchError}
          </p>
        )}
        <section aria-labelledby="recent-projects-heading" className="page-section">
          <h2 id="recent-projects-heading">Recent Projects</h2>
          {projects.isPending && <ProjectRowsSkeleton />}
          {projects.isError && (
            <div className="inline-error" role="alert">
              <p>Projects could not be loaded.</p>
              <button onClick={() => void projects.refetch()} type="button">
                Try again
              </button>
            </div>
          )}
          {!projects.isPending && !projects.isError && !recentProjects.length && (
            <div className="empty-state compact-empty-state">
              <h3>Create your first Project</h3>
              <p>
                Projects keep conversations, work, repositories, and execution
                context inside one environment.
              </p>
            </div>
          )}
          {recentProjects.length > 0 && (
            <ul className="project-rows">
              {recentProjects.map((project) => (
                <li key={project.id}>
                  <a
                    href={projectLocationHref(
                      project.id,
                      project.navigation.location,
                    )}
                    onClick={(event) => void openProject(project, event)}
                  >
                    <span aria-hidden="true" className="project-row-key">
                      {project.key}
                    </span>
                    <span>
                      <strong>{project.name}</strong>
                      <small>
                        {project.navigation.lastOpenedAt
                          ? `Opened ${new Date(
                              project.navigation.lastOpenedAt,
                            ).toLocaleDateString()}`
                          : 'Not opened yet'}
                      </small>
                    </span>
                    {project.attentionCount > 0 && (
                      <span>
                        {project.attentionCount}{' '}
                        {project.attentionCount === 1 ? 'item' : 'items'} need
                        attention
                      </span>
                    )}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </AppShell>
  )
}

function ProjectRowsSkeleton() {
  return (
    <div aria-label="Loading recent Projects" className="project-rows-skeleton">
      <span />
      <span />
      <span />
    </div>
  )
}
