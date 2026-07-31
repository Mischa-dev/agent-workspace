import { useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import {
  clearAuthenticatedQueryCache,
  logout,
  type SessionUser,
} from '../auth/session'
import { projectLocationHref } from '../projects/locations'
import {
  useProjects,
  useSwitchProject,
  useUpdateProjectNavigation,
} from '../projects/hooks'
import { ProjectInitials, ProjectSwitcher } from '../projects/ProjectSwitcher'
import type { Project } from '../projects/types'

function projectRecency(project: Project) {
  const value = project.navigation.lastOpenedAt
  return value ? Date.parse(value) || 0 : 0
}

function projectHref(project: Project) {
  return projectLocationHref(project.id, project.navigation.location)
}

function unmodifiedPrimaryClick(event: React.MouseEvent<HTMLAnchorElement>) {
  return (
    event.button === 0 &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey
  )
}

export function ProjectRail({
  currentProjectId,
  mobileOpen,
  onNavigate,
  user,
}: {
  currentProjectId: string | null
  mobileOpen: boolean
  onNavigate: () => void
  user: SessionUser
}) {
  const projectsQuery = useProjects()
  const switchProject = useSwitchProject()
  const updateNavigation = useUpdateProjectNavigation()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const searchButtonRef = useRef<HTMLButtonElement>(null)
  const [switcherOpen, setSwitcherOpen] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const projects = (projectsQuery.data?.projects ?? []).filter(
    (project) => !project.archivedAt,
  )
  const pinned = projects.filter((project) => project.navigation.pinned)
  const recent = projects
    .filter((project) => !project.navigation.pinned)
    .sort((left, right) => projectRecency(right) - projectRecency(left))
    .slice(0, 5)
  const activeProject = projects.find(
    (project) => project.id === currentProjectId,
  )
  const visibleProjects = [...pinned, ...recent]
  if (
    activeProject &&
    !visibleProjects.some((project) => project.id === activeProject.id)
  ) {
    visibleProjects.push(activeProject)
  }

  const closeSwitcher = () => {
    setSwitcherOpen(false)
    window.requestAnimationFrame(() => searchButtonRef.current?.focus())
  }

  const selectProject = async (
    project: Project,
    event: React.MouseEvent<HTMLAnchorElement>,
  ) => {
    if (!unmodifiedPrimaryClick(event)) return
    event.preventDefault()
    setStatus(null)
    try {
      const result = await switchProject.mutateAsync({
        projectId: project.id,
        expectedVersion: project.navigation.version,
        navigation: project.navigation,
      })
      setSwitcherOpen(false)
      onNavigate()
      navigate(result.href)
    } catch (reason) {
      setStatus(
        isApiError(reason) ? reason.message : 'Unable to switch Projects.',
      )
    }
  }

  const togglePin = async (project: Project) => {
    setStatus(null)
    try {
      await updateNavigation.mutateAsync({
        projectId: project.id,
        expectedVersion: project.navigation.version,
        navigation: project.navigation,
        pinned: !project.navigation.pinned,
      })
    } catch (reason) {
      setStatus(isApiError(reason) ? reason.message : 'Unable to update the pin.')
    }
  }

  const signOut = async () => {
    setStatus(null)
    try {
      await logout()
      clearAuthenticatedQueryCache(queryClient)
      onNavigate()
      navigate('/sign-in', { replace: true })
    } catch (reason) {
      setStatus(isApiError(reason) ? reason.message : 'Unable to sign out.')
    }
  }

  return (
    <>
      <nav
        aria-label="Projects"
        className="project-rail"
        data-mobile-open={mobileOpen}
        id="project-rail"
      >
        <Link
          aria-label="Global Home"
          className="brand-mark"
          onClick={onNavigate}
          to="/home"
        >
          A
        </Link>
        <div className="project-list" aria-label="Pinned and recent Projects">
          {projectsQuery.isPending && (
            <span aria-label="Loading Projects" className="rail-loading">
              ···
            </span>
          )}
          {visibleProjects.map((project) => (
            <a
              aria-current={
                project.id === currentProjectId ? 'page' : undefined
              }
              aria-label={`${project.name}${project.attentionCount ? `, ${project.attentionCount} items need attention` : ''}`}
              className="project-button"
              data-active={project.id === currentProjectId}
              href={projectHref(project)}
              key={project.id}
              onClick={(event) => void selectProject(project, event)}
            >
              <ProjectInitials project={project} />
              {project.attentionCount > 0 && (
                <span aria-hidden="true" className="rail-attention">
                  {project.attentionCount > 9 ? '9+' : project.attentionCount}
                </span>
              )}
            </a>
          ))}
        </div>

        <div className="project-rail-actions">
          <button
            aria-expanded={switcherOpen}
            aria-haspopup="dialog"
            aria-label="Search Projects"
            className="project-button"
            onClick={() => {
              setStatus(null)
              setSwitcherOpen(true)
            }}
            ref={searchButtonRef}
            type="button"
          >
            <span aria-hidden="true">⌕</span>
          </button>
          {user.capabilities.createProject && (
            <Link
              aria-label="Create Project"
              className="project-button"
              onClick={onNavigate}
              to="/projects/new"
            >
              <span aria-hidden="true">＋</span>
            </Link>
          )}
          <button
            aria-label={`Sign out ${user.displayName}`}
            className="project-button account-button"
            onClick={() => void signOut()}
            type="button"
          >
            <span aria-hidden="true">
              {user.displayName.trim().charAt(0).toLocaleUpperCase() || 'U'}
            </span>
          </button>
        </div>
        <p
          aria-live="assertive"
          className="visually-hidden"
          role={status ? 'alert' : undefined}
        >
          {status}
        </p>
      </nav>

      {switcherOpen && (
        <ProjectSwitcher
          currentProjectId={currentProjectId}
          error={status}
          onClose={closeSwitcher}
          onSelect={(project, event) => void selectProject(project, event)}
          onTogglePin={(project) => void togglePin(project)}
          projects={projects}
        />
      )}

      {location.pathname === '/home' && projectsQuery.isError && (
        <p className="visually-hidden" role="alert">
          Projects could not be loaded.
        </p>
      )}
    </>
  )
}
