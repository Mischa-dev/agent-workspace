import { Suspense, lazy, useState, type CSSProperties, type DragEvent, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link, useLocation, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { legacyPathOf, spaceHref, spaceIdOf, workspaceHref } from '../app/href'
import { toastError } from '../app/toastError'
import { useWorkspace, useWorkspaceHref, useWorkspaceId } from '../app/workspace'
import { clearAuthenticatedQueryCache, logout, useAuthenticatedSession, type OrganizationSummary, type SessionUser } from '../auth/session'
import { useDirectOverview } from '../direct/api'
import { useHomeExperience } from '../experience/layout'
import { projectLocationHref } from '../projects/locations'
import { useProjects, useSetRailOrder, useSwitchProject } from '../projects/hooks'
import type { Project } from '../projects/types'
import { Avatar, Badge, Icon, Menu, Tooltip, initials, showToast, swatchFor, type IconName } from '../ui'
import { useOptionalRealtime } from '../realtime/realtimeContext'
import { shownStatus, useStatuses } from '../status/api'
import { statusLine } from '../status/statusWords'
import { openCommandMenu, useCommandState } from './commandStore'
import { useNeedsYouBadge } from './homeSpace'
import { loadShellLazy } from './loadShellLazy'
import { isMobileWidth } from './mobileNavigation'
import { usePausedUntil } from './pauseStatus'
import { railOrderAfterDrop, railOrderKeepingOthers, railProjects } from './railProjects'

// Loaded after the first paint: whether you've paused notifications.
const PausedMark = lazy(() => loadShellLazy().then((module) => ({ default: module.PausedMark })))
const StatusDialog = lazy(() => import('../status/StatusDialog').then((module) => ({ default: module.StatusDialog })))

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
 * The workspace switcher (W1.14a, R19): every workspace the account is in,
 * each with what needs the person in its projects, and the one they're in
 * ticked. Shown once there's more than one. The counts come from the
 * projects list until the live connection carries them per workspace (W1.6).
 */
function WorkspaceSwitcher({
  current,
  onNavigate,
  projects,
  workspaces,
}: {
  current: OrganizationSummary
  onNavigate: () => void
  projects: readonly Project[]
  workspaces: readonly OrganizationSummary[]
}) {
  const navigate = useNavigate()
  const unread = (workspace: OrganizationSummary) =>
    projects.reduce((sum, project) => sum + (project.organizationId === workspace.id ? project.attentionCount : 0), 0)
  const elsewhere = workspaces.reduce((sum, workspace) => sum + (workspace.id === current.id ? 0 : unread(workspace)), 0)
  return (
    <Menu
      items={[
        { heading: 'Workspaces' },
        ...workspaces.map((workspace) => ({
          label: workspace.name,
          checked: workspace.id === current.id,
          badge: workspace.id === current.id ? undefined : unread(workspace) || undefined,
          onSelect: () => {
            onNavigate()
            navigate(workspaceHref(workspace.slug))
          },
        })),
      ]}
      label="Workspaces"
      placement="right-start"
      trigger={
        <button
          aria-label={`Workspace: ${current.name}${elsewhere ? `, ${elsewhere} unread in others` : ''}`}
          className={`aw-tile aw-tile--space aw-tile--workspace${elsewhere ? ' is-unread' : ''}`}
          data-region="workspace-switcher"
          type="button"
        >
          <TileFace swatch={swatchFor(current.id)}>{initials(current.name)}</TileFace>
          {elsewhere > 0 ? <Badge className="aw-tile__badge" count={elsewhere} max={9} /> : null}
        </button>
      }
    />
  )
}

/**
 * The far-left column: the workspace, Home, Messages, each project, and the account. Home's
 * own pages (Inbox, My work, Later...) live in its sidebar, so projects get the room.
 */
export function ProjectRail({
  currentProjectId,
  homeActive = false,
  mobileOpen,
  onNavigate,
  onSpaceSelected = onNavigate,
  user,
}: {
  currentProjectId: string | null
  /** The page is in the Home space (from its route). */
  homeActive?: boolean
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
  const session = useAuthenticatedSession()
  const workspace = useWorkspace()
  const href = useWorkspaceHref()
  const several = session.organizations.length > 1
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
  const openProjects = (projectsQuery.data?.projects ?? []).filter((project) => !project.archivedAt)
  // W1.14a: the projects of the workspace you're in.
  const projects = openProjects.filter(
    (project) => !project.modulesOff?.includes('projects') && (!several || !workspace || project.organizationId === workspace.id),
  )
  const needsYou = useNeedsYouBadge() ?? 0
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
    if (order) setRailOrder.mutate(railOrderKeepingOthers(order, openProjects), { onError: (reason) => toastError('The rail’s order wasn’t saved', reason) })
  }

  // Opens the project at once from what's cached, then records the visit
  // (the rail's "recent") in the background. The page says if the project is gone.
  const selectProject = (project: Project, event: MouseEvent<HTMLAnchorElement>) => {
    if (!unmodifiedPrimaryClick(event)) return
    event.preventDefault()
    onSpaceSelected()
    if (project.id === currentProjectId) {
      // The project's own tile leads to its overview; on a phone the drawer already shows its sidebar.
      const overview = spaceHref(project.id)
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

  const here = legacyPathOf(location.pathname)
  const inMessages = here.startsWith('/messages')
  const workspaceId = useWorkspaceId()
  const statuses = useStatuses(workspaceId, [user.actorId])
  const myStatus = shownStatus(statuses.data ?? [], user.actorId)
  const [statusOpen, setStatusOpen] = useState(false)
  // Settings open on the project you were in.
  const fromProjectId = spaceIdOf(location.pathname)
  const settingsState = fromProjectId ? { fromProjectId } : undefined

  return (
    <>
      <nav aria-label="Spaces" className="aw-rail" data-mobile-open={mobileOpen} data-region="rail" id="project-rail">
        {/* The switcher belongs to the account, not to a workspace's layout: a
            layout can't hide the way out of its own workspace. */}
        {several && workspace ? (
          <>
            <WorkspaceSwitcher current={workspace} onNavigate={onSpaceSelected} projects={openProjects} workspaces={session.organizations} />
            <span className="aw-rail__divider" role="separator" />
          </>
        ) : null}
        <UtilityTile
          active={homeActive}
          badge={needsYou}
          icon="home"
          label="Home"
          onClick={onSpaceSelected}
          to={href('/home')}
        />
        {experience.moduleOn('messages') ? (
          <UtilityTile
            active={inMessages}
            badge={unreadConversations}
            icon="message"
            label="Messages"
            onClick={onSpaceSelected}
            to={href('/messages')}
          />
        ) : null}
        <span className="aw-rail__divider" role="separator" />
        {/* Where Projects is off and no space shows, there's nothing to list (W1.39). */}
        {experience.moduleOn('projects') || visibleProjects.length > 0 ? (
          <div
            aria-label={experience.moduleOn('projects') ? 'Pinned and recent projects' : 'Pinned and recent spaces'}
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
        ) : null}
        {user.capabilities.createProject && experience.moduleOn('projects') ? (
          <UtilityTile icon="plus" label="New project" onClick={onNavigate} to={href('/projects/new')} />
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
            {
              label: myStatus ? `Status: ${statusLine(myStatus)}` : 'Set a status',
              icon: 'smile-plus',
              disabled: !workspaceId,
              onSelect: () => setStatusOpen(true),
            },
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
      {statusOpen && workspaceId ? (
        <Suspense fallback={null}>
          <StatusDialog current={myStatus} onClose={() => setStatusOpen(false)} open workspaceId={workspaceId} />
        </Suspense>
      ) : null}

      {here === '/home' && projectsQuery.isError && (
        <p className="visually-hidden" role="alert">
          Projects didn’t load.
        </p>
      )}
    </>
  )
}
