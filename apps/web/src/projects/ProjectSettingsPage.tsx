import { lazy, Suspense, useEffect, useRef, useState, type FormEvent } from 'react'
import { Navigate, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { ExecutionAccountSettings } from '../execution/ExecutionAccountSettings'
import { ExternalAgentSettings } from '../external-agents/ExternalAgentSettings'
import { SponsoredExecutionSettings } from '../external-agents/SponsoredExecutionSettings'
import { GitHubIntegrationSettings } from '../github/GitHubIntegrationSettings'
import { RepositorySettings } from '../repositories/RepositorySettings'
import { TeamSettings } from '../team/TeamSettings'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, DetailList, PageHeader, PageTabs, Section, SettingsSkeleton, TextField, type IconName } from '../ui'
import { roleLabel } from './format'
import { NavigationSyncAlert } from './NavigationSyncAlert'
import { ProjectPageError, ProjectPageLoading } from './ProjectPageStates'
import { useArchiveProject, useProject } from './hooks'
import { projectLocationHref } from './locations'
import type { Project } from './types'
import { useRememberProjectLocation } from './useRememberProjectLocation'
import { WorkflowSettings } from '../work-items/WorkflowSettings'

const LaunchSettings = lazy(() => import('../launch/LaunchSettings').then((module) => ({ default: module.LaunchSettings })))

type SettingsTab = { id: string; label: string; icon: IconName }

function settingsTabs(project: Project, organizationRole: string | undefined): SettingsTab[] {
  const manage = project.capabilities.manage
  const member = project.role !== 'viewer'
  return [
    { id: 'general', label: 'General', icon: 'settings' as const },
    ...(manage && organizationRole ? [{ id: 'members', label: 'Members', icon: 'users' as const }] : []),
    ...(manage ? [{ id: 'agents', label: 'Agents', icon: 'bot' as const }] : []),
    ...(member ? [{ id: 'codex', label: 'Codex', icon: 'terminal' as const }] : []),
    ...(member ? [{ id: 'launch', label: 'Launch', icon: 'play' as const }] : []),
    ...(member ? [{ id: 'workflow', label: 'Workflow', icon: 'work' as const }] : []),
    { id: 'repositories', label: 'Repositories', icon: 'repo' as const },
    ...(manage ? [{ id: 'github', label: 'GitHub', icon: 'pull-request' as const }] : []),
  ]
}

export function ProjectSettingsPage() {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const [parameters] = useSearchParams()
  const project = useProject(projectId)
  const archive = useArchiveProject()
  const navigate = useNavigate()
  const errorRef = useRef<HTMLDivElement>(null)
  const [confirmation, setConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [archiveSubmissionStarted, setArchiveSubmissionStarted] = useState(false)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])
  const navigationSync = useRememberProjectLocation(
    projectId,
    { kind: 'settings' },
    project.isSuccess && !project.data.archivedAt && project.data.capabilities.navigationWrite,
    project.data?.navigation,
  )
  const organization = session.organizations.find((candidate) => candidate.id === project.data?.organizationId)

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
    return <Navigate replace to={projectLocationHref(project.data.id, { kind: 'project_overview' })} />
  }

  const tabs = settingsTabs(project.data, organization?.role)
  const current = tabs.find((tab) => tab.id === parameters.get('tab'))?.id ?? 'general'
  const settingsHref = `/projects/${encodeURIComponent(project.data.id)}/settings`

  const submitArchive = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (confirmation !== project.data.key) return
    setArchiveSubmissionStarted(true)
    setError(null)
    try {
      await archive.mutateAsync({ projectId: project.data.id, expectedVersion: project.data.version })
      navigate('/home', {
        replace: true,
        state: { notice: `${project.data.name} was archived. Everything in it was kept.` },
      })
    } catch (reason) {
      setArchiveSubmissionStarted(false)
      if (isApiError(reason) && reason.code === 'version_conflict') {
        await project.refetch()
        setError('Someone changed this Project while you were here. Check the details above and try again.')
      } else {
        setError(isApiError(reason) ? reason.message : 'The Project wasn’t archived. Try again.')
      }
    }
  }

  return (
    <AppShell project={project.data} user={session.user}>
      <div className="aw-page aw-page--settings">
        <NavigationSyncAlert {...navigationSync} />
        <PageHeader icon="settings" title="Project settings">
          {project.data.name} · {project.data.key}
        </PageHeader>
        <PageTabs
          current={current}
          label="Settings sections"
          tabs={tabs.map((tab) => ({
            ...tab,
            to: tab.id === 'general' ? settingsHref : `${settingsHref}?tab=${tab.id}`,
          }))}
        />

        {current === 'general' && (
          <>
            <Section id="project-details-heading" title="About this Project">
              <DetailList
                items={[
                  { label: 'Name', value: project.data.name },
                  { label: 'Key', value: <code className="aw-inline-code">{project.data.key}</code> },
                  { label: 'Your role', value: roleLabel(project.data.role) },
                  ...(organization ? [{ label: 'Organization', value: organization.name }] : []),
                ]}
              />
              {project.data.role === 'viewer' && (
                <Banner role="note" tone="info">
                  Viewer access is read-only. You can look at the Project and its repositories, but members, agents,
                  Codex and GitHub are managed by members and admins.
                </Banner>
              )}
            </Section>
            {project.data.capabilities.archive && !project.data.archivedAt && (
              <Section
                description="Archiving takes it out of everyone’s sidebar and stops new messages, work items and runs. Everything already in it is kept and stays readable."
                id="archive-project-heading"
                title="Archive this Project"
              >
                <form className="aw-danger" onSubmit={submitArchive}>
                  {error && (
                    <Banner ref={errorRef} tabIndex={-1} tone="danger">
                      {error}
                    </Banner>
                  )}
                  <TextField
                    autoComplete="off"
                    id="archive-confirmation"
                    label={
                      <>
                        Type <strong>{project.data.key}</strong> to confirm
                      </>
                    }
                    mono
                    onChange={(event) => setConfirmation(event.target.value)}
                    spellCheck={false}
                    value={confirmation}
                  />
                  <div>
                    <Button
                      busy={archive.isPending}
                      disabled={confirmation !== project.data.key}
                      icon="archive"
                      type="submit"
                      variant="danger"
                    >
                      Archive Project
                    </Button>
                  </div>
                </form>
              </Section>
            )}
          </>
        )}

        {current === 'members' && organization && (
          <TeamSettings organizationRole={organization.role} projectId={project.data.id} />
        )}

        {current === 'agents' && <ExternalAgentSettings projectId={project.data.id} />}

        {current === 'workflow' && <WorkflowSettings projectId={project.data.id} />}

        {current === 'codex' && (
          <>
            <Section
              description="The Codex sign-in used when you start a run. It’s yours alone: nobody else in the Project can use it."
              id="execution-account-heading"
              title="Codex account"
            >
              <ExecutionAccountSettings actorId={session.user.actorId} projectId={project.data.id} />
            </Section>
            <Section
              description="Agents can ask to run Codex on your account. Nothing runs until you approve it here."
              id="sponsored-execution-heading"
              title="Run requests from agents"
            >
              <SponsoredExecutionSettings projectId={project.data.id} />
            </Section>
          </>
        )}

        {current === 'launch' && (
          <Suspense fallback={<SettingsSkeleton />}>
            <LaunchSettings projectId={project.data.id} />
          </Suspense>
        )}

        {current === 'repositories' && (
          <Section
            description="The code runs can work in. The server keeps its own copy of each repository, so nothing is read from your computer."
            id="repositories-heading"
            title="Repositories"
          >
            <RepositorySettings projectId={project.data.id} />
          </Section>
        )}

        {current === 'github' && (
          <Section
            description="Pull requests and checks reach this Project through a GitHub App installation that a host operator sets up."
            id="github-integration-heading"
            title="GitHub"
          >
            <GitHubIntegrationSettings projectId={project.data.id} />
          </Section>
        )}
      </div>
    </AppShell>
  )
}
