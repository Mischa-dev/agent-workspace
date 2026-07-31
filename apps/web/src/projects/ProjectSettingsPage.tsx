import { FormEvent, useEffect, useRef, useState } from 'react'
import { Navigate, useNavigate, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { ExecutionAccountSettings } from '../execution/ExecutionAccountSettings'
import { AppShell } from '../shell/AppShell'
import { NavigationSyncAlert } from './NavigationSyncAlert'
import { ProjectContextDrawer } from './ProjectContextDrawer'
import { useArchiveProject, useProject } from './hooks'
import { projectLocationHref } from './locations'
import {
  ProjectPageError,
  ProjectPageLoading,
} from './ProjectOverviewPage'
import { useRememberProjectLocation } from './useRememberProjectLocation'

export function ProjectSettingsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  const archive = useArchiveProject()
  const navigate = useNavigate()
  const errorRef = useRef<HTMLDivElement>(null)
  const [confirmation, setConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [archiveSubmissionStarted, setArchiveSubmissionStarted] =
    useState(false)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'settings' },
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
  if (project.data.archivedAt && !archiveSubmissionStarted) {
    return (
      <Navigate
        replace
        to={projectLocationHref(project.data.id, {
          kind: 'project_overview',
        })}
      />
    )
  }

  const submitArchive = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (confirmation !== project.data.key) return
    setArchiveSubmissionStarted(true)
    setError(null)
    try {
      await archive.mutateAsync({
        projectId: project.data.id,
        expectedVersion: project.data.version,
      })
      navigate('/home', {
        replace: true,
        state: { notice: `${project.data.name} was archived. Its history was preserved.` },
      })
    } catch (reason) {
      setArchiveSubmissionStarted(false)
      if (isApiError(reason) && reason.code === 'version_conflict') {
        const details =
          reason.details && typeof reason.details === 'object'
            ? (reason.details as Record<string, unknown>)
            : {}
        const currentVersion =
          typeof details.currentVersion === 'number'
            ? details.currentVersion
            : null
        await project.refetch()
        setError(
          `The Project changed after this page loaded${
            currentVersion === null
              ? ''
              : ` (current version ${currentVersion})`
          }. Review the refreshed details and try again.`,
        )
      } else {
        setError(
          isApiError(reason)
            ? reason.message
            : 'The Project could not be archived.',
        )
      }
    }
  }

  return (
    <AppShell
      context={<ProjectContextDrawer project={project.data} />}
      project={project.data}
      user={session.user}
    >
      <div className="page-content narrow-page">
        <NavigationSyncAlert {...navigationSync} />
        <header className="page-heading">
          <div>
            <span className="section-label">{project.data.key}</span>
            <h1>Project settings</h1>
            <p>Manage the durable Project boundary.</p>
          </div>
        </header>
        <section aria-labelledby="project-details-heading" className="page-section">
          <h2 id="project-details-heading">Details</h2>
          <dl className="detail-list">
            <div>
              <dt>Name</dt>
              <dd>{project.data.name}</dd>
            </div>
            <div>
              <dt>Key</dt>
              <dd>{project.data.key}</dd>
            </div>
          </dl>
        </section>

        <section aria-labelledby="execution-account-heading" className="page-section">
          <h2 id="execution-account-heading">Codex account</h2>
          <p>
            Connect the personal Execution Profile used for hosted runs. Provider
            credentials are handled by the isolated Codex profile and are never
            returned to this page.
          </p>
          <ExecutionAccountSettings
            actorId={session.user.actorId}
            projectId={project.data.id}
          />
        </section>

        {project.data.capabilities.archive && !project.data.archivedAt && (
          <section
            aria-labelledby="archive-project-heading"
            className="archive-project"
          >
            <h2 id="archive-project-heading">Archive Project</h2>
            <p>
              Archive removes this Project from ordinary navigation and rejects
              new mutations. Conversation and execution history is preserved.
            </p>
            {error && (
              <div
                className="form-error-summary"
                ref={errorRef}
                role="alert"
                tabIndex={-1}
              >
                {error}
              </div>
            )}
            <form onSubmit={submitArchive}>
              <div className="field">
                <label htmlFor="archive-confirmation">
                  Type <strong>{project.data.key}</strong> to confirm
                </label>
                <input
                  autoComplete="off"
                  id="archive-confirmation"
                  onChange={(event) => setConfirmation(event.target.value)}
                  spellCheck={false}
                  value={confirmation}
                />
              </div>
              <button
                className="danger-button"
                disabled={
                  archive.isPending || confirmation !== project.data.key
                }
                type="submit"
              >
                {archive.isPending ? 'Archiving…' : 'Archive Project'}
              </button>
            </form>
          </section>
        )}
      </div>
    </AppShell>
  )
}
