import { type MouseEvent } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import homeClaims from 'virtual:modules/slots/home'

import { useAuthenticatedSession } from '../auth/session'
import { useHomeExperience } from '../experience/layout'
import { ownerOf } from '../modules/ownership'
import { AppShell } from '../shell/AppShell'
import { defaultWorkspace } from '../settings/workspaceChoice'
import { Badge, Banner, ButtonLink, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, Section, relativeDay } from '../ui'
import { firstName, homeGreeting } from './greeting'
import { Capacity, NeedsYou, QuietLine, SetupCard, WorkingNow, YourTasks } from './HomeSections'
import { useQuietSections } from './homeQuiet'
import { useProjects, useSwitchProject } from './hooks'
import './home.css'
import { MoreLink } from './MoreLink'
import { ProjectInitials } from './ProjectInitials'
import type { Project } from './types'
import '../styles/pages.css'

const RECENT_PROJECTS = 6

function openedLabel(iso: string | null) {
  if (!iso) return 'Not opened yet'
  const day = relativeDay(iso)
  return day ? `Opened ${day}` : 'Not opened yet'
}

function openedAt(project: Project) {
  return project.navigation.lastOpenedAt ? Date.parse(project.navigation.lastOpenedAt) || 0 : 0
}

/** Home (P3): what needs you, agents working now, capacity, your tasks and recent projects, in one look. */
export function HomePage() {
  const session = useAuthenticatedSession()
  const workspace = defaultWorkspace(session)
  const experience = useHomeExperience()
  const projects = useProjects()
  const switchProject = useSwitchProject()
  const quiet = useQuietSections()
  const navigate = useNavigate()
  const location = useLocation()
  const notice =
    location.state &&
    typeof location.state === 'object' &&
    'notice' in location.state &&
    typeof location.state.notice === 'string'
      ? location.state.notice
      : null
  const activeProjects = (projects.data?.projects ?? []).filter((project) => !project.archivedAt)
  const recentProjects = [...activeProjects].sort((left, right) => openedAt(right) - openedAt(left)).slice(0, RECENT_PROJECTS)
  const firstVisit = projects.isSuccess && activeProjects.every((project) => !project.navigation.lastOpenedAt)
  const canCreate = session.user.capabilities.createProject
  const noProjects = projects.isSuccess && activeProjects.length === 0

  // Opens the project at once where you left it, and records the visit in the background.
  const openProject = (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    event.preventDefault()
    navigate(project.navigation.href)
    if (project.capabilities.navigationWrite) {
      switchProject
        .mutateAsync({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation })
        .catch(() => undefined)
    }
  }

  // AWD-75: a Home section of a module that's off doesn't show.
  const homeOn = (section: string) => {
    const module = ownerOf(homeClaims, section)
    return !module || experience.moduleOn(module)
  }
  // Sections this layout shows; the ones that loaded empty fold into one line above the rest.
  const sections = (
    [
      ['needsYou', homeOn('needsYou')],
      ['working', experience.shows('home.working_now')],
      ['capacity', experience.shows('home.capacity')],
      ['tasks', experience.shows('workspace.my_work')],
    ] as const
  )
    .filter(([, shown]) => shown)
    .map(([section]) => section)
  const left = sections.filter((section) => (section === 'needsYou' || section === 'working') && !quiet[section])
  const right = sections.filter((section) => (section === 'capacity' || section === 'tasks') && !quiet[section])

  const newProject = (
    <ButtonLink icon="plus" to="/projects/new" variant="primary">
      New project
    </ButtonLink>
  )

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-home">
        {/* With no projects yet, the empty state below holds the one New project button. */}
        <PageHeader actions={canCreate && !noProjects && homeOn('recent') ? newProject : null} title={homeGreeting(firstName(session.user.displayName), firstVisit)} />
        {notice && (
          <Banner className="aw-page__banner" tone="success">
            {notice}
          </Banner>
        )}
        {workspace ? <SetupCard organizationId={workspace.id} /> : null}
        <QuietLine quiet={quiet} sections={sections} />
        <div className="aw-home__grid">
          {left.length ? (
            <div className="aw-home__column">
              {left.includes('needsYou') ? <NeedsYou /> : null}
              {left.includes('working') ? <WorkingNow /> : null}
            </div>
          ) : null}
          <div className="aw-home__column">
            {right.includes('capacity') ? <Capacity /> : null}
            {right.includes('tasks') ? <YourTasks /> : null}
            {homeOn('recent') ? (
            <Section
              actions={activeProjects.length > 0 ? <MoreLink to="/projects">All projects</MoreLink> : null}
              id="recent-projects-heading"
              title="Recent projects"
            >
              {projects.isPending && <ListSkeleton label="Loading your projects" rows={3} />}
              {projects.isError && <ErrorState error={projects.error} onRetry={() => void projects.refetch()} title="Your projects didn’t load" />}
              {noProjects && (
                <EmptyState action={canCreate ? newProject : null} icon="home" title={canCreate ? 'Create your first project' : 'No projects yet'}>
                  {canCreate
                    ? 'A project holds its own channels, tasks, repositories and agents.'
                    : 'Ask a workspace owner or admin to add you to a project.'}
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
                      meta={experience.shows('tech.task_keys') ? `${project.key} · ${openedLabel(project.navigation.lastOpenedAt)}` : openedLabel(project.navigation.lastOpenedAt)}
                      title={
                        <a className="aw-row__link" href={project.navigation.href} onClick={(event) => openProject(project, event)}>
                          {project.name}
                        </a>
                      }
                    />
                  ))}
                </RowList>
              )}
            </Section>
            ) : null}
          </div>
        </div>
      </div>
    </AppShell>
  )
}
