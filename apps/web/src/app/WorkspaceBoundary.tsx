import { useEffect } from 'react'
import { Navigate, Outlet, useLocation, useParams } from 'react-router-dom'

import { useAuthenticatedSession, type OrganizationSummary } from '../auth/session'
import { useProject, useProjects } from '../projects/hooks'
import { CommandCenter } from '../shell/CommandCenter'
import { useShellPage } from '../shell/shellStore'
import { ButtonLink, EmptyState } from '../ui'
import { CURRENT_WORKSPACE, isWorkspacePath, withinWorkspace, workspaceHref, workspaceSlugOf } from './href'
import { rememberWorkspace, resolveWorkspace, useWorkspace, WorkspaceContext } from './workspace'

const PROJECT_ADDRESS = /^\/projects\/([^/]+)/

function decoded(segment: string | undefined) {
  if (!segment) return null
  try {
    return decodeURIComponent(segment)
  } catch {
    return null
  }
}

/**
 * Works out the workspace every signed-in page is in and remembers it for
 * addresses that don't name one: the slug in `/w/<slug>/...`, else the
 * workspace of the project in `/projects/<id>/...`, else the last one used
 * in this browser, else the person's default.
 */
export function WorkspaceProvider() {
  const session = useAuthenticatedSession()
  const { pathname } = useLocation()
  const slug = workspaceSlugOf(pathname)
  const projectId = slug ? null : decoded(PROJECT_ADDRESS.exec(pathname)?.[1])
  const projects = useProjects()
  const listed = projectId ? projects.data?.projects.find((project) => project.id === projectId)?.organizationId : undefined
  // The rail's list holds only active projects, and not all of them: an
  // archived or far-down project's own read (the page asks for it too) names its workspace.
  const opened = useProject(projectId && projects.data && !listed ? projectId : '')
  const projectWorkspace = listed ?? opened.data?.organizationId
  const named = slug
    ? session.organizations.find((organization) => organization.slug === slug)
    : projectWorkspace
      ? session.organizations.find((organization) => organization.id === projectWorkspace)
      : undefined
  const workspace = named ?? resolveWorkspace(session)
  const remembered = named?.slug
  useEffect(() => {
    if (remembered) rememberWorkspace(remembered)
  }, [remembered])
  return (
    <WorkspaceContext.Provider value={workspace}>
      <Outlet />
      {/* Shortcuts and the palette follow the workspace in the address, so they live inside it. */}
      <CommandCenter />
    </WorkspaceContext.Provider>
  )
}

/**
 * `/w/:ws`: a workspace the person is in shows its pages; `/w/-/...` opens
 * the last one used; any other slug says the person isn't in it.
 */
export function WorkspaceBoundary() {
  const { ws = '' } = useParams()
  const session = useAuthenticatedSession()
  const location = useLocation()
  if (ws === CURRENT_WORKSPACE) {
    const workspace = resolveWorkspace(session)
    if (!workspace) return <NotInWorkspacePage workspaces={[]} />
    const rest = withinWorkspace(location.pathname)
    return <Navigate replace state={location.state} to={`${workspaceHref(workspace.slug, rest || '/home')}${location.search}${location.hash}`} />
  }
  if (!session.organizations.some((organization) => organization.slug === ws)) {
    return <NotInWorkspacePage workspaces={session.organizations} />
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
