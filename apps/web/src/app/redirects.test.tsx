import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen } from '@testing-library/react'
import { createMemoryRouter, matchRoutes, RouterProvider, useLocation } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'

import { AuthenticatedSessionProvider, type AuthenticatedSession } from '../auth/session'
import { isWorkspacePath, legacyPathOf, workspaceHref } from './href'
import { appRoutes, routesWithHandles } from './routes'
import { WorkspaceBoundary, WorkspaceProvider, WorkspaceRedirect } from './WorkspaceBoundary'
import { rememberWorkspace } from './workspace'

// W1.14a: every old address of a workspace page (the workspace-level routes
// of the route table before W1.14a) still opens that page, now under
// /w/<slug>, with its query and hash; W1.14d deletes these after K4's links move.
const OLD_WORKSPACE_ROUTES = [
  '/',
  '/home',
  '/welcome',
  '/search',
  '/inbox',
  '/later',
  '/threads',
  '/my-work',
  '/runs',
  '/runs/:runId',
  '/pages',
  '/whiteboards',
  '/whiteboards/:whiteboardId',
  '/messages',
  '/messages/agents',
  '/messages/:conversationId',
  '/projects',
  '/projects/new',
  '/new-task',
  '/share',
]

const acme = { id: 'org-acme', name: 'Acme', slug: 'acme', role: 'owner' as const }
const beta = { id: 'org-beta', name: 'Beta', slug: 'beta', role: 'member' as const }
const session: AuthenticatedSession = {
  authenticated: true,
  user: {
    id: 'user-1',
    actorId: 'actor-1',
    actorKind: 'human',
    loginName: 'mischa',
    displayName: 'Mischa',
    defaultOrganizationId: acme.id,
    capabilities: { createProject: true },
  },
  organizations: [acme, beta],
}

const concrete = (path: string) => path.replace(/:[A-Za-z]+/g, 'x1')

function Landed() {
  const location = useLocation()
  return <output aria-label="Landed">{`${location.pathname}${location.search}${location.hash}`}</output>
}

function open(path: string) {
  const router = createMemoryRouter(
    [
      {
        element: <WorkspaceProvider />,
        children: [
          ...OLD_WORKSPACE_ROUTES.map((old) => ({ path: old, element: <WorkspaceRedirect /> })),
          { path: '/dm/:conversationId', element: <WorkspaceRedirect /> },
          { path: '/w/:ws', element: <WorkspaceBoundary />, children: [{ path: '*', element: <Landed /> }] },
        ],
      },
    ],
    { initialEntries: [path] },
  )
  const queryClient = new QueryClient({ defaultOptions: { queries: { enabled: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <AuthenticatedSessionProvider session={session}>
        <RouterProvider router={router} />
      </AuthenticatedSessionProvider>
    </QueryClientProvider>,
  )
}

function openSpaces(path: string) {
  const router = createMemoryRouter(
    [
      {
        element: <WorkspaceProvider />,
        children: [
          { path: '/projects/:projectId/*', element: <WorkspaceRedirect /> },
          { path: '/settings/workspace/*', element: <WorkspaceRedirect /> },
          { path: '/settings/projects/*', element: <WorkspaceRedirect /> },
          { path: '/w/:ws', element: <WorkspaceBoundary />, children: [{ path: '*', element: <Landed /> }] },
        ],
      },
    ],
    { initialEntries: [path] },
  )
  const queryClient = new QueryClient({ defaultOptions: { queries: { enabled: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <AuthenticatedSessionProvider session={session}>
        <RouterProvider router={router} />
      </AuthenticatedSessionProvider>
    </QueryClientProvider>,
  )
}

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

describe('old addresses', () => {
  it('lists every workspace-level route the table had before W1.14a', () => {
    const redirects = routesWithHandles()
      .filter((route) => route.handle?.layout === 'redirect' && route.path && (route.path === '/' || isWorkspacePath(route.path)))
      // A space's old addresses and old settings links are W1.14b's, below.
      .filter((route) => !route.path?.startsWith('/projects/:projectId') && !route.path?.startsWith('/settings/'))
      .map((route) => route.path)
    expect(redirects.filter((path) => path !== '/dm/:conversationId').sort()).toEqual([...OLD_WORKSPACE_ROUTES].sort())
  })

  it.each(OLD_WORKSPACE_ROUTES)('%s opens the same page in the workspace used last, query and hash kept', async (old) => {
    rememberWorkspace('beta')
    const path = concrete(old)
    open(`${path}?view=all#top`)
    const target = workspaceHref('beta', `${path === '/' ? '/home' : path}?view=all#top`)
    expect(await screen.findByLabelText('Landed')).toHaveTextContent(target)
    // It lands on a page of the route table, not another redirect.
    const matches = matchRoutes(appRoutes, target.split('?')[0]) ?? []
    const landed = matches[matches.length - 1]?.route as { handle?: { layout?: string } } | undefined
    expect(landed?.handle?.layout).not.toBe('redirect')
    expect(legacyPathOf(target.split('?')[0])).toBe(path === '/' ? '/home' : path)
  })

  // W1.14b: a space's old addresses open in the space's place under /w/<slug>/s/<id>, in the address's words.
  it.each([
    ['/projects/p1', '/w/beta/s/p1'],
    ['/projects/p1/work-items/t1?tab=runs', '/w/beta/s/p1/tasks/t1?tab=runs'],
    ['/projects/p1/channels/c1#m1', '/w/beta/s/p1/chat/c1#m1'],
    ['/projects/p1/launches', '/w/beta/s/p1/runs'],
    ['/projects/p1/graph', '/w/beta/s/p1/planning/graph'],
    ['/projects/p1/settings?tab=codex', '/w/beta/settings/spaces/p1?tab=codex'],
    ['/settings/workspace/members', '/w/beta/settings/members'],
    ['/settings/projects/p1/automations', '/w/beta/settings/spaces/p1/automations'],
  ])('%s opens at %s', async (old, now) => {
    rememberWorkspace('beta')
    openSpaces(old)
    expect(await screen.findByLabelText('Landed')).toHaveTextContent(now)
  })

  it('opens the default workspace when this browser has used none', async () => {
    open('/inbox')
    expect(await screen.findByLabelText('Landed')).toHaveTextContent('/w/acme/inbox')
  })

  it('opens a DM’s own address in the workspace used last', async () => {
    rememberWorkspace('beta')
    open('/dm/c1')
    expect(await screen.findByLabelText('Landed')).toHaveTextContent('/w/beta/dm/c1')
  })

  it('opens /w/-/… (the home-screen shortcuts) in the workspace used last', async () => {
    rememberWorkspace('beta')
    open('/w/-/inbox?tab=all')
    expect(await screen.findByLabelText('Landed')).toHaveTextContent('/w/beta/inbox?tab=all')
  })

  it('remembers the workspace in the address for the next address without one', async () => {
    open('/w/beta/later')
    expect(await screen.findByLabelText('Landed')).toHaveTextContent('/w/beta/later')
    expect(window.localStorage.getItem('agent-workspace:workspace')).toBe('beta')
  })

  it('says so for a workspace the person isn’t in, and lists theirs', async () => {
    open('/w/elsewhere/home')
    expect(await screen.findByRole('heading', { name: 'You’re not in this workspace' })).toBeVisible()
    expect(screen.getByRole('link', { name: 'Acme' })).toHaveAttribute('href', '/w/acme/home')
    expect(screen.getByRole('link', { name: 'Beta' })).toHaveAttribute('href', '/w/beta/home')
  })
})
