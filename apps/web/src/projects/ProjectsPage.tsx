import { useDeferredValue, useState, type MouseEvent } from 'react'
import { useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import {
  Badge,
  Banner,
  ButtonLink,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  TextField,
  countLabel,
  relativeDay,
} from '../ui'
import { useProjects, useProjectSearch, useSwitchProject } from './hooks'
import './home.css'
import { ProjectInitials } from './ProjectInitials'
import type { Project } from './types'

/** The list asks the server for this many; past it, typing searches the rest. */
const LIST_LIMIT = 100

function projectMeta(project: Project) {
  const opened = project.navigation.lastOpenedAt ? relativeDay(project.navigation.lastOpenedAt) : null
  return [project.key, project.navigation.pinned ? 'Pinned' : null, opened ? `Opened ${opened}` : 'Not opened yet']
    .filter(Boolean)
    .join(' · ')
}

/** Every project you belong to, by name: the rail and Home show only the recent few. */
export function ProjectsPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const switchProject = useSwitchProject()
  const [query, setQuery] = useState('')
  const wanted = useDeferredValue(query.trim())
  const all = useProjects()
  const search = useProjectSearch(wanted)
  const source = wanted ? search : all
  const projects = (source.data?.projects ?? [])
    .filter((project) => !project.archivedAt)
    .sort((left, right) => left.name.localeCompare(right.name))
  const total = (all.data?.projects ?? []).filter((project) => !project.archivedAt).length
  const canCreate = session.user.capabilities.createProject

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

  return (
    <AppShell home mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--narrow">
        <PageHeader
          actions={
            canCreate ? (
              <ButtonLink icon="plus" to="/projects/new" variant="primary">
                New project
              </ButtonLink>
            ) : null
          }
          icon="list"
          title="All projects"
        >
          {all.isSuccess ? (total ? `${countLabel(total, 'project')} you belong to.` : 'You don’t belong to any projects yet.') : ' '}
        </PageHeader>
        {total > 8 || wanted ? (
          <TextField
            aria-label="Find a project"
            className="aw-projects__search"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Find a project by name or key"
            type="search"
            value={query}
          />
        ) : null}
        {source.isPending ? <ListSkeleton label="Loading your projects" rows={6} /> : null}
        {source.isError ? (
          <ErrorState error={source.error} onRetry={() => void source.refetch()} title="Your projects didn’t load" />
        ) : null}
        {source.isSuccess && projects.length === 0 ? (
          wanted ? (
            <EmptyState icon="search" title="No projects match">
              Try another name or key, or clear the search.
            </EmptyState>
          ) : (
            <EmptyState
              action={
                canCreate ? (
                  <ButtonLink icon="plus" to="/projects/new" variant="primary">
                    New project
                  </ButtonLink>
                ) : null
              }
              icon="home"
              title={canCreate ? 'Create your first project' : 'No projects yet'}
            >
              {canCreate
                ? 'A project holds its own channels, tasks, repositories and agents.'
                : 'Ask a workspace owner or admin to add you to a project.'}
            </EmptyState>
          )
        ) : null}
        {projects.length > 0 ? (
          <RowList label="Projects">
            {projects.map((project) => (
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
                meta={projectMeta(project)}
                title={
                  <a className="aw-row__link" href={project.navigation.href} onClick={(event) => openProject(project, event)}>
                    {project.name}
                  </a>
                }
              />
            ))}
          </RowList>
        ) : null}
        {!wanted && total >= LIST_LIMIT ? (
          <Banner className="aw-projects__note" tone="info">
            Showing the first {LIST_LIMIT}. Find a project by name to see the others.
          </Banner>
        ) : null}
      </div>
    </AppShell>
  )
}
