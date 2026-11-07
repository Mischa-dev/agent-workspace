import { Suspense, lazy, useState, type CSSProperties, type DragEvent, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { toastError } from '../app/toastError'
import { clearAuthenticatedQueryCache, logout, type SessionUser } from '../auth/session'
import { useDirectOverview } from '../direct/api'
import { useHomeExperience } from '../experience/layout'
import { projectLocationHref } from '../projects/locations'
import { useProjects, useSetRailOrder, useSwitchProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { Avatar, Badge, Icon, Menu, Tooltip, showToast, swatchFor, type IconName } from '../ui'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { openCommandMenu, useCommandState } from './commandStore'
import { inHomeSpace, useInboxUnread } from './homeSpace'
import { loadShellLazy } from './loadShellLazy'
import { isMobileWidth } from './mobileNavigation'
import { usePausedUntil } from './pauseStatus'
import { railOrderAfterDrop, railProjects } from './railProjects'

// Loaded after the first paint: whether you've paused notifications.
const PausedMark = lazy(() => loadShellLazy().then((module) => ({ default: module.PausedMark })))

/** What a dragged project tile carries, so other drags (files, text) don't reorder the rail. */
const RAIL_DRAG_TYPE = 'application/x-agent-workspace-project'

type RailDrag = { id: string; over: { id: string; after: boolean } | null }

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

/**
 * The far-left column: Home, Messages, each project, and the account. Home's
 * own pages (Inbox, My work, Later...) live in its sidebar, so projects get the room.
 */
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
   * Called instead of onNavigate when Home, Messages or a project is chosen. On phones
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
  const pausedUntil = usePausedUntil()
  // AWD-75: no tile for a project where Projects is off for you.
  const projects = (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt && !project.projectsOff)
  const inboxUnread = useInboxUnread() ?? 0
  // AWD-75: no Messages tile with Messages off, and no New project tile
  // with Projects off at Home, where a new project goes.
  const experience = useHomeExperience()
  const visibleProjects = railProjects(projects, currentProjectId)
  // Tiles drag into your own order (not on phones, where the rail is a drawer).
  const setRailOrder = useSetRailOrder()
  const [drag, setDrag] = useState<RailDrag | null>(null)
  const startDrag = (event: DragEvent<HTMLAnchorElement>, project: Project) => {
    if (isMobileWidth() || !project.capabilities.navigationWrite) return
    event.dataTransfer.effectAllowed = 'all'
    event.dataTransfer.setData(RAIL_DRAG_TYPE, project.id)
    setDrag({ id: project.id, over: null })
  }
  const dragOver = (event: DragEvent<HTMLDivElement>) => {
    if (!drag || !event.dataTransfer.types.includes(RAIL_DRAG_TYPE)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'move'
    const tile = (event.target as HTMLElement).closest<HTMLElement>('[data-rail-project]')
    if (!tile) return
    const box = tile.getBoundingClientRect()
    const over = { id: tile.dataset.railProject ?? '', after: event.clientY > box.top + box.height / 2 }
    if (drag.over?.id !== over.id || drag.over.after !== over.after) setDrag({ ...drag, over })
  }
  const drop = (event: DragEvent<HTMLDivElement>) => {
    if (!drag) return
    event.preventDefault()
    const order = drag.over ? railOrderAfterDrop(visibleProjects, drag.id, drag.over) : null
    setDrag(null)
    if (order) setRailOrder.mutate(order, { onError: (reason) => toastError('The rail’s order wasn’t saved', reason) })
  }

  // Opens the project at once from what's cached, then records the visit
  // (the rail's "recent") in the background. The page says if the project is gone.
  const selectProject = (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (!unmodifiedPrimaryClick(event)) return
    event.preventDefault()
    onSpaceSelected()
    if (project.id === currentProjectId) {
      // The project's own tile leads to its overview; on a phone the drawer already shows its sidebar.
      const overview = `/projects/${encodeURIComponent(project.id)}`
      if (!isMobileWidth() && location.pathname !== overview) navigate(overview)
      return
    }
    navigate(projectHref(project))
    if (project.capabilities.navigationWrite) {
      switchProject
        .mutateAsync({ projectId: project.id, expectedVersion: project.navigation.version, navigation: project.navigation })
        .catch(() => undefined)
    }
  }

  const signOut = async () => {
    try {
      await logout()
      clearAuthenticatedQueryCache(queryClient)
      onNavigate()
      navigate('/sign-in', { replace: true })
    } catch (reason) {
      showToast({ tone: 'danger', title: 'You’re still signed in', description: isApiError(reason) ? reason.message : 'Try again in a moment.' })
    }
  }

  const inMessages = location.pathname.startsWith('/messages')
  // Settings open on the project you were in.
  const fromProjectId = /^\/projects\/([^/]+)/.exec(location.pathname)?.[1]
  const settingsState = fromProjectId && fromProjectId !== 'new' ? { fromProjectId } : undefined

  return (
    <>
      <nav aria-label="Spaces" className="aw-rail" data-mobile-open={mobileOpen} data-region="rail" id="project-rail">
        <UtilityTile
          active={inHomeSpace(location.pathname)}
          badge={inboxUnread}
          icon="home"
          label="Home"
          onClick={onSpaceSelected}
          to="/home"
        />
        {experience.moduleOn('messages') ? (
          <UtilityTile
            active={inMessages}
            badge={unreadConversations}
            icon="message"
            label="Messages"
            onClick={onSpaceSelected}
            to="/messages"
          />
        ) : null}
        <span className="aw-rail__divider" role="separator" />
        <div
          aria-label="Pinned and recent projects"
          className="aw-rail__projects project-list"
          data-dragging={drag ? 'true' : undefined}
          onDragLeave={(event) => {
            if (drag?.over && !event.currentTarget.contains(event.relatedTarget as Node | null)) setDrag({ ...drag, over: null })
          }}
          onDragOver={dragOver}
          onDrop={drop}
          role="group"
        >
          {projectsQuery.isPending ? <span aria-label="Loading projects" className="aw-rail__loading" role="status" /> : null}
          {visibleProjects.map((project) => {
            const active = project.id === currentProjectId
            const dropMark = drag?.over?.id === project.id && drag.id !== project.id ? (drag.over.after ? ' is-drop-after' : ' is-drop-before') : ''
            return (
              <Tooltip key={project.id} label={project.name} side="right">
                <a
                  aria-current={active ? 'page' : undefined}
                  aria-label={`${project.name}${project.attentionCount ? `, ${project.attentionCount} unread` : ''}`}
                  className={`aw-tile aw-tile--space${active ? ' is-active' : ''}${project.attentionCount ? ' is-unread' : ''}${drag?.id === project.id ? ' is-dragging' : ''}${dropMark}`}
                  data-rail-project={project.id}
                  href={projectHref(project)}
                  onClick={(event) => selectProject(project, event)}
                  onDragEnd={() => setDrag(null)}
                  onDragStart={(event) => startDrag(event, project)}
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
        {user.capabilities.createProject && experience.moduleOn('projects') ? (
          <UtilityTile icon="plus" label="New project" onClick={onNavigate} to="/projects/new" />
        ) : null}
        <span className="aw-rail__spacer" />
        <Tooltip label="Go anywhere or run a command" shortcut="⌘K" side="right">
          <button
            aria-expanded={commandMenuOpen}
            aria-haspopup="dialog"
            aria-label="Search and commands"
            className="aw-tile aw-tile--utility aw-tile--quiet"
            onClick={(event) => {
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
            // The choices (an hour, until morning) and the resume live in the command menu.
            pausedUntil
              ? {
                  label: 'Resume notifications',
                  icon: 'play',
                  onSelect: () => {
                    onNavigate()
                    openCommandMenu('Resume notifications')
                  },
                }
              : {
                  label: 'Pause notifications',
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
      </nav>

      {location.pathname === '/home' && projectsQuery.isError && (
        <p className="visually-hidden" role="alert">
          Projects didn’t load.
        </p>
      )}
    </>
  )
}
