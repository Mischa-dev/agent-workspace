import { Suspense, lazy, useState, type CSSProperties, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { clearAuthenticatedQueryCache, logout, type SessionUser } from '../auth/session'
import { useDirectOverview } from '../direct/api'
import { projectLocationHref } from '../projects/locations'
import { useProjects, useSwitchProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { Avatar, Badge, Icon, Menu, Tooltip, swatchFor, type IconName } from '../ui'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { openCommandMenu, useCommandState } from './commandStore'
import { loadShellLazy } from './loadShellLazy'

// Loaded after the first paint: whether you've paused notifications.
const PausedMark = lazy(() => loadShellLazy().then((module) => ({ default: module.PausedMark })))

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
  state,
  to,
}: {
  active?: boolean
  badge?: number
  icon: IconName
  label: string
  onClick?: () => void
  state?: unknown
  to: string
}) {
  return (
    <Tooltip label={label} side="right">
      <Link
        aria-current={active ? 'page' : undefined}
        aria-label={badge ? `${label}, ${badge} unread` : label}
        className={`aw-tile aw-tile--utility${active ? ' is-active' : ''}${badge ? ' is-unread' : ''}`}
        onClick={onClick}
        state={state}
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
  const switchProject = useSwitchProject()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const location = useLocation()
  // Your dot shows whether live updates reach you, not a status you set.
  const connection = useOptionalRealtime()?.connection
  const presence = connection === 'live' ? 'online' : connection ? 'away' : undefined
  const commandMenuOpen = useCommandState().open === 'commands'
  const [status, setStatus] = useState<string | null>(null)
  const projects = (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt)
  // Every notification belongs to a project, so the inbox's unread count is the sum.
  const inboxUnread = (projectsQuery.data?.projects ?? []).reduce((sum, project) => sum + project.attentionCount, 0)
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

  const selectProject = async (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (!unmodifiedPrimaryClick(event)) return
    event.preventDefault()
    setStatus(null)
    if (!project.capabilities.navigationWrite) {
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
      onSpaceSelected()
      navigate(result.href)
    } catch (reason) {
      setStatus(isApiError(reason) ? reason.message : 'Couldn’t switch Projects. Try again.')
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
  // Settings open on the Project you were in.
  const fromProjectId = /^\/projects\/([^/]+)/.exec(location.pathname)?.[1]
  const settingsState = fromProjectId && fromProjectId !== 'new' ? { fromProjectId } : undefined

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
        <UtilityTile active={location.pathname === '/my-work'} icon="checklist" label="My work" onClick={onNavigate} to="/my-work" />
        <UtilityTile active={location.pathname === '/later'} icon="bookmark" label="Later" onClick={onNavigate} to="/later" />
        <UtilityTile active={location.pathname.startsWith('/runs')} icon="activity" label="Runs" onClick={onNavigate} to="/runs" />
        <UtilityTile
          active={location.pathname.startsWith('/whiteboards')}
          icon="whiteboard"
          label="Whiteboards"
          onClick={onNavigate}
          to="/whiteboards"
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
        <Tooltip label="Go anywhere or run a command" shortcut="⌘K" side="right">
          <button
            aria-expanded={commandMenuOpen}
            aria-haspopup="dialog"
            aria-label="Search and commands"
            className="aw-tile aw-tile--utility aw-tile--quiet"
            onClick={(event) => {
              setStatus(null)
              // Focus comes back here when the menu closes, also after a tap.
              event.currentTarget.focus()
              onNavigate()
              openCommandMenu()
            }}
            type="button"
          >
            <TileFace>
              <Icon name="search" size={20} />
            </TileFace>
          </button>
        </Tooltip>
        <UtilityTile
          active={location.pathname.startsWith('/settings')}
          icon="settings"
          label="Settings"
          onClick={onNavigate}
          state={settingsState}
          to="/settings"
        />
        <Menu
          items={[
            { heading: user.displayName },
            {
              label: 'Pause or resume notifications',
              icon: 'moon',
              onSelect: () => {
                onNavigate()
                openCommandMenu('Pause notifications')
              },
            },
            { label: 'Settings', icon: 'settings', onSelect: () => { onNavigate(); navigate('/settings', { state: settingsState }) } },
            { label: 'Appearance', icon: 'palette', onSelect: () => { onNavigate(); navigate('/settings/account/appearance') } },
            { separator: true },
            { label: 'Sign out', icon: 'log-out', onSelect: () => void signOut() },
          ]}
          label="Account"
          placement="right-start"
          trigger={
            <button aria-label={`Account: ${user.displayName}`} className="aw-rail__me" type="button">
              <Avatar name={user.displayName} presence={presence} seed={user.actorId} size={36} />
              <Suspense fallback={null}>
                <PausedMark />
              </Suspense>
            </button>
          }
        />
        <p aria-live="assertive" className="visually-hidden" role={status ? 'alert' : undefined}>
          {status}
        </p>
      </nav>

      {location.pathname === '/home' && projectsQuery.isError && (
        <p className="visually-hidden" role="alert">
          Projects could not be loaded.
        </p>
      )}
    </>
  )
}
