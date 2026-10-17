import { useState, type MouseEvent } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { defaultWorkspace } from '../settings/workspaceChoice'
import { Badge, Banner, Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, Section, relativeDay } from '../ui'
import { Capacity, NeedsYou, SetupCard, WorkingNow, YourTasks } from './HomeSections'
import { useProjects, useSwitchProject } from './hooks'
import './home.css'
import { ProjectInitials } from './ProjectInitials'
import type { Project } from './types'

function openedLabel(iso: string | null) {
  if (!iso) return 'Not opened yet'
  const day = relativeDay(iso)
  return day ? `Opened ${day}` : 'Not opened yet'
}

/** Home (P3): what needs you, agents working now, capacity, your tasks and recent projects, in one look. */
export function HomePage() {
  const session = useAuthenticatedSession()
  const workspace = defaultWorkspace(session)
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
  const activeProjects = (projects.data?.projects ?? []).filter((project) => !project.archivedAt)
  const recentProjects = [...activeProjects]
    .sort((left, right) => {
      const leftTime = left.navigation.lastOpenedAt ? Date.parse(left.navigation.lastOpenedAt) : 0
      const rightTime = right.navigation.lastOpenedAt ? Date.parse(right.navigation.lastOpenedAt) : 0
      return rightTime - leftTime
    })
    .slice(0, 6)
  const waiting = activeProjects.reduce((total, project) => total + project.attentionCount, 0)
  const canCreate = session.user.capabilities.createProject

  const openProject = async (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    event.preventDefault()
    setSwitchError(null)
    if (!project.capabilities.navigationWrite) {
      navigate(project.navigation.href)
      return
    }
    try {
      const result = await switchProject.mutateAsync({
        projectId: project.id,
        expectedVersion: project.navigation.version,
        navigation: project.navigation,
      })
      navigate(result.href)
    } catch (reason) {
      setSwitchError(isApiError(reason) ? reason.message : 'We couldn’t open that project where you left off. Try again.')
    }
  }

  const createLink = canCreate ? (
    <Link className="aw-btn aw-btn--primary aw-btn--md" to="/projects/new">
      <Icon name="plus" />
      <span className="aw-btn__label">Create project</span>
    </Link>
  ) : null

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-home">
        <PageHeader actions={createLink} title={`Welcome back, ${session.user.displayName}`}>
          {projects.isSuccess
            ? waiting
              ? `${waiting} ${waiting === 1 ? 'thing needs' : 'things need'} you across your projects.`
              : 'You’re all caught up.'
            : 'Pick up where you left off.'}
        </PageHeader>
        {notice && (
          <Banner className="aw-page__banner" tone="success">
            {notice}
          </Banner>
        )}
        {switchError && (
          <Banner className="aw-page__banner" tone="danger">
            {switchError}
          </Banner>
        )}
        {workspace ? <SetupCard organizationId={workspace.id} /> : null}
        <div className="aw-home__grid">
          <div className="aw-home__column">
            <NeedsYou />
            <WorkingNow />
          </div>
          <div className="aw-home__column">
            <Capacity />
            <YourTasks />
            <Section id="recent-projects-heading" title="Recent projects">
              {projects.isPending && <ListSkeleton label="Loading your projects" rows={3} />}
              {projects.isError && (
                <ErrorState action={<Button onClick={() => void projects.refetch()}>Try again</Button>} title="Your projects didn’t load">
                  Check your connection and try again.
                </ErrorState>
              )}
              {projects.isSuccess && !recentProjects.length && (
                <EmptyState action={createLink} icon="home" title={canCreate ? 'Create your first project' : 'No projects yet'}>
                  {canCreate
                    ? 'A project holds its own channels, tasks, repositories and agents.'
                    : 'Ask an owner or admin to invite you to a project.'}
                </EmptyState>
              )}
              {recentProjects.length > 0 && (
                <RowList label="Recent projects">
                  {recentProjects.map((project) => (
                    <Row
                      actions={
                        project.attentionCount > 0 ? (
                          <span className="aw-home__waiting">
                            <Badge count={project.attentionCount} />
                            {project.attentionCount === 1 ? 'needs you' : 'need you'}
                          </span>
                        ) : (
                          <Icon className="aw-row__chev" name="chevron-right" />
                        )
                      }
                      className="aw-row--link"
                      key={project.id}
                      leading={<ProjectInitials project={project} size={36} />}
                      meta={`${project.key} · ${openedLabel(project.navigation.lastOpenedAt)}`}
                      title={
                        <a className="aw-row__link" href={project.navigation.href} onClick={(event) => void openProject(project, event)}>
                          {project.name}
                        </a>
                      }
                    />
                  ))}
                </RowList>
              )}
            </Section>
          </div>
        </div>
      </div>
    </AppShell>
  )
}
