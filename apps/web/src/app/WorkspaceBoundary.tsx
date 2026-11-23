import { lazy, Suspense, useEffect } from 'react'
import { useIsFetching } from '@tanstack/react-query'
import { Navigate, Outlet, useLocation, useParams } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'
import { useAuthenticatedSession, type OrganizationSummary } from '../auth/session'
import { AvatarSourcesProvider } from '../profiles/AvatarSourcesProvider'
import { useOnlineMark } from '../realtime/realtimeContext'
import { useProject, useProjects } from '../projects/hooks'
import { CommandCenter } from '../shell/CommandCenter'
import { useShellPage } from '../shell/shellStore'
import { AvatarPresence, ButtonLink, EmptyState } from '../ui'
import { CURRENT_WORKSPACE, isWorkspacePath, setCurrentWorkspaceSlug, spaceIdOf, withinWorkspace, workspaceHref, workspaceSlugOf } from './href'
import { rememberWorkspace, resolveWorkspace, useWorkspace, WorkspaceContext } from './workspace'

// Loads after the page: it keeps recent reading on this device when the person and workspace allow it (W1.31).
const OfflineReading = lazy(() => import('../data/OfflineReading'))

/** The workspace of the space with this id (`''`: none), from the rail's list or the space's own read. */
function useSpaceWorkspace(spaceId: string | null): string | undefined {
  const projects = useProjects()
  const listed = spaceId ? projects.data?.projects.find((project) => project.id === spaceId)?.organizationId : undefined
  // The rail's list holds only active projects, and not all of them: an
  // archived or far-down project's own read (the page asks for it too) names its workspace.
  const opened = useProject(spaceId && projects.data && !listed ? spaceId : '')
  return listed ?? opened.data?.organizationId
}

/**
 * Works out the workspace every signed-in page is in and remembers it for
 * addresses that don't name one: the slug in `/w/<slug>/...`, else the
 * workspace of the space in an old `/projects/<id>/...`, else the last one
 * used in this browser, else the person's default. Links built below it
 * (app/href.ts) go to that workspace.
 */
export function WorkspaceProvider() {
  const session = useAuthenticatedSession()
  const { pathname } = useLocation()
  const slug = workspaceSlugOf(pathname)
  const projectWorkspace = useSpaceWorkspace(slug ? null : spaceIdOf(pathname))
  const named = slug
    ? session.organizations.find((organization) => organization.slug === slug)
    : projectWorkspace
      ? session.organizations.find((organization) => organization.id === projectWorkspace)
      : undefined
  const workspace = named ?? resolveWorkspace(session)
  // Before any page below renders, so every link it builds is in this workspace.
  setCurrentWorkspaceSlug(workspace?.slug)
  const remembered = named?.slug
  useEffect(() => {
    if (remembered) rememberWorkspace(remembered)
  }, [remembered])
  return (
    <WorkspaceContext.Provider value={workspace}>
      <AvatarSourcesProvider workspaceId={workspace?.id}>
        <AvatarPresence.Provider value={useOnlineMark}>
          <Outlet />
          {/* Shortcuts and the palette follow the workspace in the address, so they live inside it. */}
          <CommandCenter />
          <Suspense fallback={null}>
            <OfflineReading actorId={session.user.actorId} workspaceId={workspace?.id} workspaces={session.organizations} />
          </Suspense>
        </AvatarPresence.Provider>
      </AvatarSourcesProvider>
    </WorkspaceContext.Provider>
  )
}

/**
 * `/w/:ws`: a workspace the person is in shows its pages; `/w/-/...` opens
 * the last one used; `/w/<its id>/...` opens the same page at its slug; any
 * other slug says the person isn't in it.
 */
export function WorkspaceBoundary() {
  const { ws = '' } = useParams()
  const session = useAuthenticatedSession()
  const location = useLocation()
  const checking = useIsFetching({ queryKey: queryKeys.session, exact: true }) > 0
  if (ws === CURRENT_WORKSPACE) {
    const workspace = resolveWorkspace(session)
    if (!workspace) return <NotInWorkspacePage workspaces={[]} />
    const rest = withinWorkspace(location.pathname)
    return <Navigate replace state={location.state} to={`${workspaceHref(workspace.slug, rest || '/home')}${location.search}${location.hash}`} />
  }
  if (!session.organizations.some((organization) => organization.slug === ws)) {
    // The server writes a workspace's id where it doesn't know the slug: the same page at the slug.
    const byId = session.organizations.find((organization) => organization.id === ws)
    if (byId) {
      return <Navigate replace state={location.state} to={`${workspaceHref(byId.slug, withinWorkspace(location.pathname) || '/home')}${location.search}${location.hash}`} />
    }
    // The session shown may be a kept copy still being checked: wait for the answer before saying so.
    if (checking) return null
    return <NotInWorkspacePage workspaces={session.organizations} />
  }
  return <SpaceInWorkspace ws={ws} />
}

/**
 * A space's address names the workspace it's in; one that names another
 * workspace (a link built in the wrong one, or a space moved) is replaced
 * with the right one.
 */
function SpaceInWorkspace({ ws }: { ws: string }) {
  const session = useAuthenticatedSession()
  const location = useLocation()
  const spaceWorkspace = useSpaceWorkspace(spaceIdOf(location.pathname))
  const owner = spaceWorkspace ? session.organizations.find((organization) => organization.id === spaceWorkspace) : undefined
  if (owner && owner.slug !== ws) {
    return <Navigate replace state={location.state} to={`${workspaceHref(owner.slug, withinWorkspace(location.pathname))}${location.search}${location.hash}`} />
  }
  return <Outlet />
}

/**
 * An old address of a workspace page (`/inbox`, `/runs/1`, `/`), or a DM's
 * own (`/dm/1`): the same page in the workspace the person is in, with its
 * query and hash. Kept for a release after links move (W1.14d).
 */
export function WorkspaceRedirect() {
  const workspace = useWorkspace()
  const session = useAuthenticatedSession()
  const location = useLocation()
  if (!workspace) return <NotInWorkspacePage workspaces={session.organizations} />
  const path = location.pathname === '/' || !isWorkspacePath(location.pathname) ? '/home' : location.pathname
  return <Navigate replace state={location.state} to={workspaceHref(workspace.slug, `${path}${location.search}${location.hash}`)} />
}

/** An address in a workspace the person isn't in (or no longer is): the ones they are in. */
export function NotInWorkspacePage({ workspaces }: { workspaces: readonly OrganizationSummary[] }) {
  useShellPage({ mainClassName: 'aw-main--page' })
  return (
    <div className="aw-page" role="alert">
      <EmptyState
        action={
          workspaces.length > 0 ? (
            <div aria-label="Your workspaces" className="aw-empty-choices" role="group">
              {workspaces.map((workspace) => (
                <ButtonLink key={workspace.id} to={workspaceHref(workspace.slug)}>
                  {workspace.name}
                </ButtonLink>
              ))}
            </div>
          ) : null
        }
        icon="lock"
        title="You’re not in this workspace"
      >
        {workspaces.length > 0
          ? 'The link may be old, or you may have left it. These are the workspaces you’re in.'
          : 'You aren’t in any workspace yet. Ask someone to invite you.'}
      </EmptyState>
    </div>
  )
}
