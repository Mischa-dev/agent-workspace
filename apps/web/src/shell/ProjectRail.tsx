import { useEffect, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { clearAuthenticatedQueryCache, logout, type SessionUser } from '../auth/session'
import { useDirectOverview } from '../direct/api'
import { useInboxSummary } from '../inbox/api'
import { projectLocationHref } from '../projects/locations'
import { useProjects, useSwitchProject, useUpdateProjectNavigation } from '../projects/hooks'
import { ProjectSwitcher } from '../projects/ProjectSwitcher'
import type { Project } from '../projects/types'
import { Avatar, Badge, Icon, Menu, Tooltip, swatchFor, type IconName } from '../ui'

function projectRecency(project: Project) {
  const value = project.navigation.lastOpenedAt
  return value ? Date.parse(value) || 0 : 0
}

function projectHref(project: Project) {
  return projectLocationHref(project.id, project.navigation.location)
}

function unmodifiedPrimaryClick(event: MouseEvent<HTMLAnchorElement>) {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
}

function TileFace({ children, swatch }: { children: ReactNode; swatch?: string }) {
  return (
    <>
      <span aria-hidden="true" className="aw-tile__indicator" />
      <span
        className="aw-tile__face"
        style={swatch ? ({ '--tile-bg': `var(--swatch-${swatch})` } as CSSProperties) : undefined}
      >
        {children}
      </span>
    </>
  )
}

function UtilityTile({
  active = false,
  badge,
  icon,
  label,
  onClick,
  to,
}: {
  active?: boolean
  badge?: number
  icon: IconName
  label: string
  onClick?: () => void
  to: string
}) {
  return (
    <Tooltip label={label} side="right">
      <Link
        aria-current={active ? 'page' : undefined}
        aria-label={badge ? `${label}, ${badge} unread` : label}
        className={`aw-tile aw-tile--utility${active ? ' is-active' : ''}${badge ? ' is-unread' : ''}`}
        onClick={onClick}
        to={to}
      >
        <TileFace>
          <Icon name={icon} size={20} />
        </TileFace>
        {badge ? <Badge className="aw-tile__badge" count={badge} max={9} /> : null}
      </Link>
    </Tooltip>
  )
}

/** The far-left column: Home, Messages, each Project, and the account. */
export function ProjectRail({
  currentProjectId,
  mobileOpen,
  onNavigate,
  onSpaceSelected = onNavigate,
  user,
}: {
  currentProjectId: string | null
  mobileOpen: boolean
  onNavigate: () => void
  /**
   * Called instead of onNavigate when a Project or Messages is chosen. On phones
   * the drawer stays open so its sidebar can show that space's channels.
   */
  onSpaceSelected?: () => void
  user: SessionUser
}) {
  const projectsQuery = useProjects()
  const direct = useDirectOverview()
  const unreadConversations = direct.data?.conversations.filter((conversation) => conversation.unread).length ?? 0
  const inboxUnread = useInboxSummary().data?.unreadCount ?? 0
  const switchProject = useSwitchProject()
  const updateNavigation = useUpdateProjectNavigation()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  const searchButtonRef = useRef<HTMLButtonElement>(null)
  const [switcherOpen, setSwitcherOpen] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const projects = (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt)
  const pinned = projects.filter((project) => project.navigation.pinned)
  const recent = projects
    .filter((project) => !project.navigation.pinned)
    .sort((left, right) => projectRecency(right) - projectRecency(left))
    .slice(0, 5)
  const activeProject = projects.find((project) => project.id === currentProjectId)
  const visibleProjects = [...pinned, ...recent]
  if (activeProject && !visibleProjects.some((project) => project.id === activeProject.id)) {
    visibleProjects.push(activeProject)
  }

  // Cmd/Ctrl+K finds a Project from anywhere.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k' && !event.shiftKey && !event.altKey) {
        event.preventDefault()
        setStatus(null)
        setSwitcherOpen(true)
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])

  const closeSwitcher = () => {
    setSwitcherOpen(false)
    window.requestAnimationFrame(() => searchButtonRef.current?.focus())
  }

  const selectProject = async (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (!unmodifiedPrimaryClick(event)) return
    event.preventDefault()
    setStatus(null)
    if (!project.capabilities.navigationWrite) {
      setSwitcherOpen(false)
      onSpaceSelected()
      navigate(projectHref(project))
      return
    }
    try {
      const result = await switchProject.mutateAsync({
        projectId: project.id,
        expectedVersion: project.navigation.version,
        navigation: project.navigation,
      })
      setSwitcherOpen(false)
      onSpaceSelected()
      navigate(result.href)
    } catch (reason) {
      setStatus(isApiError(reason) ? reason.message : 'Couldn’t switch Projects. Try again.')
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
      setStatus(isApiError(reason) ? reason.message : 'Couldn’t update the pin. Try again.')
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
      setStatus(isApiError(reason) ? reason.message : 'Couldn’t sign out. Try again.')
    }
  }

  const inMessages = location.pathname.startsWith('/messages')

  return (
    <>
      <nav aria-label="Projects" className="aw-rail" data-mobile-open={mobileOpen} data-region="rail" id="project-rail">
        <UtilityTile active={location.pathname === '/home'} icon="home" label="Home" onClick={onNavigate} to="/home" />
        <UtilityTile
          active={location.pathname === '/inbox'}
          badge={inboxUnread}
          icon="inbox"
          label="Inbox"
          onClick={onNavigate}
          to="/inbox"
        />
        <UtilityTile
          active={inMessages}
          badge={unreadConversations}
          icon="message"
          label="Messages"
          onClick={onSpaceSelected}
          to="/messages"
        />
        <span className="aw-rail__divider" role="separator" />
        <div aria-label="Pinned and recent Projects" className="aw-rail__projects project-list" role="group">
          {projectsQuery.isPending ? <span aria-label="Loading Projects" className="aw-rail__loading" role="status" /> : null}
          {visibleProjects.map((project) => {
            const active = project.id === currentProjectId
            return (
              <Tooltip key={project.id} label={project.name} side="right">
                <a
                  aria-current={active ? 'page' : undefined}
                  aria-label={`${project.name}${project.attentionCount ? `, ${project.attentionCount} items need attention` : ''}`}
                  className={`aw-tile aw-tile--space${active ? ' is-active' : ''}${project.attentionCount ? ' is-unread' : ''}`}
                  href={projectHref(project)}
                  onClick={(event) => void selectProject(project, event)}
                >
                  <TileFace swatch={swatchFor(project.id)}>{project.key.slice(0, 2)}</TileFace>
                  {project.attentionCount > 0 ? (
                    <Badge className="aw-tile__badge" count={project.attentionCount} max={9} />
                  ) : null}
                </a>
              </Tooltip>
            )
          })}
        </div>
        {user.capabilities.createProject ? (
          <UtilityTile icon="plus" label="Create Project" onClick={onNavigate} to="/projects/new" />
        ) : null}
        <span className="aw-rail__spacer" />
        <Tooltip label="Find a Project" shortcut="⌘K" side="right">
          <button
            aria-expanded={switcherOpen}
            aria-haspopup="dialog"
            aria-label="Search Projects"
            className="aw-tile aw-tile--utility aw-tile--quiet"
            onClick={() => {
              setStatus(null)
              setSwitcherOpen(true)
            }}
            ref={searchButtonRef}
            type="button"
          >
            <TileFace>
              <Icon name="search" size={20} />
            </TileFace>
          </button>
        </Tooltip>
        <UtilityTile
          active={location.pathname.startsWith('/settings/appearance')}
          icon="palette"
          label="Appearance"
          onClick={onNavigate}
          to="/settings/appearance"
        />
        <Menu
          items={[
            { heading: user.displayName },
            { label: 'Account settings', onSelect: () => { onNavigate(); navigate('/settings/account') } },
            { label: 'Appearance', icon: 'palette', onSelect: () => { onNavigate(); navigate('/settings/appearance') } },
            { separator: true },
            { label: 'Sign out', icon: 'log-out', onSelect: () => void signOut() },
          ]}
          label="Account"
          placement="right-start"
          trigger={
            <button aria-label={`Account: ${user.displayName}`} className="aw-rail__me" type="button">
              <Avatar name={user.displayName} presence="online" seed={user.actorId} size={36} />
            </button>
          }
        />
        <p aria-live="assertive" className="visually-hidden" role={status ? 'alert' : undefined}>
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
