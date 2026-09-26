import { useState, type MouseEvent } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Badge, Banner, Button, EmptyState, ErrorState, Icon, MessageSkeleton, PageHeader, Row, RowList, Section } from '../ui'
import { useProjects, useSwitchProject } from './hooks'
import { ProjectInitials } from './ProjectSwitcher'
import type { Project } from './types'

function openedLabel(iso: string | null) {
  if (!iso) return 'Not opened yet'
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return 'Not opened yet'
  const startOf = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime()
  const days = Math.round((startOf(new Date()) - startOf(date)) / 86_400_000)
  if (days === 0) return 'Opened today'
  if (days === 1) return 'Opened yesterday'
  return `Opened ${date.toLocaleDateString([], { month: 'short', day: 'numeric' })}`
}

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
  const activeProjects = (projects.data?.projects ?? []).filter((project) => !project.archivedAt)
  const recentProjects = [...activeProjects]
    .sort((left, right) => {
      const leftTime = left.navigation.lastOpenedAt ? Date.parse(left.navigation.lastOpenedAt) : 0
      const rightTime = right.navigation.lastOpenedAt ? Date.parse(right.navigation.lastOpenedAt) : 0
      return rightTime - leftTime
    })
    .slice(0, 8)
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
      setSwitchError(isApiError(reason) ? reason.message : 'We couldn’t open that Project where you left off. Try again.')
    }
  }

  const createLink = canCreate ? (
    <Link className="aw-btn aw-btn--primary aw-btn--md" to="/projects/new">
      <Icon name="plus" />
      <span className="aw-btn__label">Create Project</span>
    </Link>
  ) : null

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--narrow">
        <PageHeader actions={createLink} title={`Welcome back, ${session.user.displayName}`}>
          {projects.isSuccess
            ? waiting
              ? `${waiting} ${waiting === 1 ? 'thing needs' : 'things need'} you across your Projects.`
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
        <Section id="recent-projects-heading" title="Your Projects">
          {projects.isPending && <MessageSkeleton label="Loading your Projects" rows={3} />}
          {projects.isError && (
            <ErrorState action={<Button onClick={() => void projects.refetch()}>Try again</Button>} title="Your Projects didn’t load">
              Check your connection and try again.
            </ErrorState>
          )}
          {projects.isSuccess && !recentProjects.length && (
            <EmptyState action={createLink} icon="home" title={canCreate ? 'Create your first Project' : 'No Projects yet'}>
              {canCreate
                ? 'A Project holds its own channels, work items, repositories and agents.'
                : 'Ask an owner or admin to invite you to a Project.'}
            </EmptyState>
          )}
          {recentProjects.length > 0 && (
            <RowList label="Your Projects">
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
    </AppShell>
  )
}
