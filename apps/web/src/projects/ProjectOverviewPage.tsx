import { Link, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { NavigationSyncAlert } from './NavigationSyncAlert'
import { ProjectContextDrawer } from './ProjectContextDrawer'
import { useProject } from './hooks'
import { useRememberProjectLocation } from './useRememberProjectLocation'

export function ProjectOverviewPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'project_overview' },
    project.isSuccess &&
      !project.data.archivedAt &&
      project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )

  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }

  return (
    <AppShell
      context={<ProjectContextDrawer project={project.data} />}
      project={project.data}
      user={session.user}
    >
      <div className="page-content">
        <NavigationSyncAlert {...navigationSync} />
        <header className="page-heading">
          <div>
            <span className="section-label">{project.data.key}</span>
            <h1>{project.data.name}</h1>
            <p>A contained environment for conversation, work, and execution.</p>
          </div>
          {project.data.capabilities.manage && (
            <Link
              className="secondary-link"
              to={`/projects/${encodeURIComponent(project.data.id)}/settings`}
            >
              Project settings
            </Link>
          )}
        </header>
        {project.data.archivedAt && (
          <p className="notice" role="status">
            This Project is archived. Its history remains available, but new
            mutations are disabled.
          </p>
        )}
        <section aria-labelledby="project-status-heading" className="page-section">
          <h2 id="project-status-heading">Project status</h2>
          <dl className="detail-list">
            <div>
              <dt>Project key</dt>
              <dd>{project.data.key}</dd>
            </div>
            <div>
              <dt>Your access</dt>
              <dd>{project.data.role || 'Member'}</dd>
            </div>
            <div>
              <dt>Attention</dt>
              <dd>
                {project.data.attentionCount
                  ? `${project.data.attentionCount} items`
                  : 'No current items'}
              </dd>
            </div>
          </dl>
        </section>
      </div>
    </AppShell>
  )
}

export function ProjectPageLoading() {
  return (
    <div aria-busy="true" className="page-content">
      <div aria-label="Loading Project" className="page-skeleton">
        <span />
        <span />
        <span />
      </div>
    </div>
  )
}

export function ProjectPageError({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="page-content">
      <div className="error-state" role="alert">
        <h1>Project unavailable</h1>
        <p>
          The Project does not exist, is no longer accessible, or could not be
          loaded.
        </p>
        <button onClick={onRetry} type="button">
          Try again
        </button>
      </div>
    </div>
  )
}
