import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { Link, Outlet, RouterProvider, createMemoryRouter, useNavigate } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
import { AppShell } from './AppShell'
import { MobileNavigationProvider } from './MobileNavigationProvider'
import { useMobileNavigation } from './mobileNavigation'
import { ShellLayout } from './ShellLayout'
import { useShellPanel } from './shellStore'

const user: SessionUser = {
  id: 'user-a',
  actorId: 'actor-a',
  actorKind: 'human',
  loginName: 'mischa',
  displayName: 'Mischa Nelson',
  capabilities: { createProject: true },
}

vi.mock('../auth/session', () => ({ useAuthenticatedSession: () => ({ user }) }))

vi.mock('./ProjectRail', () => ({
  ProjectRail: ({ mobileOpen }: { mobileOpen: boolean }) => <nav data-mobile-open={mobileOpen} id="project-rail" />,
}))

vi.mock('./ProjectSidebar', () => ({
  ProjectSidebar: ({ project, thread }: { project: Project; thread?: { name: string } | null }) => (
    <aside id="project-sidebar" style={{ overflow: 'auto' }}>
      {project.name}
      {thread ? ` / ${thread.name}` : ''}
    </aside>
  ),
}))

const project = {
  id: 'project-a',
  name: 'Agent Workspace',
  archivedAt: null,
} as unknown as Project

function Nav() {
  const navigation = useMobileNavigation()
  const navigate = useNavigate()
  return (
    <>
      <Link to="/projects/project-a/channels/c1">Channel</Link>
      <Link to="/projects/project-a/threads/t1">Thread</Link>
      <Link to="/projects/project-a/pages">Pages</Link>
      <button onClick={() => navigation.setOpen(true)} type="button">
        Open navigation
      </button>
      <button
        onClick={() => {
          navigation.keepOpenOnNextNavigation()
          navigate('/search')
        }}
        type="button"
      >
        Switch space
      </button>
    </>
  )
}

function ChannelPage() {
  const panel = useShellPanel(<p>Channel details</p>, 'details')
  return panel ? <Nav /> : null
}

function ThreadPage() {
  return (
    <AppShell project={project} thread={{ id: 't1', name: 'Walking skeleton' }} user={user}>
      <Nav />
    </AppShell>
  )
}

function ProjectPage() {
  return (
    <AppShell project={project} user={user}>
      <Nav />
    </AppShell>
  )
}

function SearchPage() {
  return (
    <AppShell user={user}>
      <Nav />
    </AppShell>
  )
}

function renderShell(path: string) {
  const client = new QueryClient()
  client.setQueryData(queryKeys.projects, { projects: [project] })
  const router = createMemoryRouter(
    [
      {
        element: (
          <MobileNavigationProvider>
            <Outlet />
          </MobileNavigationProvider>
        ),
        children: [
          {
            element: <ShellLayout />,
            children: [
              {
                path: '/projects/:projectId/channels/:channelId',
                element: (
                  <AppShell project={project} user={user}>
                    <ChannelPage />
                  </AppShell>
                ),
              },
              { path: '/projects/:projectId/threads/:threadId', element: <ThreadPage /> },
              { path: '/projects/:projectId/pages', element: <ProjectPage /> },
              { path: '/search', element: <SearchPage /> },
            ],
          },
        ],
      },
    ],
    { initialEntries: [path] },
  )
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
}

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('the persistent shell', () => {
  it('keeps the same sidebar, and its scroll, from channel to thread to page', () => {
    renderShell('/projects/project-a/channels/c1')
    const sidebar = document.getElementById('project-sidebar')
    const rail = document.getElementById('project-rail')
    expect(sidebar).toHaveTextContent('Agent Workspace')
    if (!sidebar) throw new Error('no sidebar')
    sidebar.scrollTop = 40

    fireEvent.click(screen.getByRole('link', { name: 'Thread' }))
    expect(document.getElementById('project-sidebar')).toBe(sidebar)
    expect(sidebar).toHaveTextContent('Agent Workspace / Walking skeleton')

    fireEvent.click(screen.getByRole('link', { name: 'Pages' }))
    expect(document.getElementById('project-sidebar')).toBe(sidebar)
    expect(document.getElementById('project-rail')).toBe(rail)
    expect(sidebar.scrollTop).toBe(40)
    expect(sidebar).not.toHaveTextContent('Walking skeleton')
  })

  it('draws the panel the page asks for, and drops it when the page goes', () => {
    renderShell('/projects/project-a/channels/c1')
    expect(document.getElementById('context-region')).toHaveTextContent('Channel details')

    fireEvent.click(screen.getByRole('link', { name: 'Pages' }))
    expect(document.getElementById('context-region')).toBeNull()
  })

  it('keeps the phone drawer open across a space change', () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('max-width'),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }))
    renderShell('/projects/project-a/pages')
    const rail = document.getElementById('project-rail')
    act(() => fireEvent.click(screen.getByRole('button', { name: 'Open navigation' })))
    expect(rail).toHaveAttribute('data-mobile-open', 'true')

    act(() => fireEvent.click(screen.getByRole('button', { name: 'Switch space' })))
    expect(document.getElementById('project-rail')).toBe(rail)
    expect(rail).toHaveAttribute('data-mobile-open', 'true')
  })
})
