import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { Link, RouterProvider, createMemoryRouter, useNavigate } from 'react-router-dom'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import type { SessionUser } from '../auth/session'
import type { Project } from '../projects/types'
import { ShellLayout } from './ShellLayout'
import { useShellPage, useShellPanel } from './shellStore'

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
  ProjectRail: ({ homeActive, mobileOpen, onSpaceSelected }: { homeActive: boolean; mobileOpen: boolean; onSpaceSelected: () => void }) => (
    <nav data-home-active={homeActive} data-mobile-open={mobileOpen} id="project-rail">
      <button onClick={onSpaceSelected} type="button">
        Pick a space
      </button>
    </nav>
  ),
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
  const navigate = useNavigate()
  return (
    <>
      <Link to="/projects/project-a/channels/c1">Channel</Link>
      <Link to="/projects/project-a/threads/t1">Thread</Link>
      <Link to="/projects/project-a/pages">Pages</Link>
      <Link to="/inbox">Inbox</Link>
      <button onClick={() => navigate('/search')} type="button">
        Go to search
      </button>
    </>
  )
}

function ChannelPage() {
  useShellPage({ project })
  const panel = useShellPanel(<p>Channel details</p>, 'details')
  return panel ? <Nav /> : null
}

function ThreadPage() {
  useShellPage({ project, thread: { id: 't1', name: 'Walking skeleton' } })
  return <Nav />
}

/** A page that hasn't loaded its project yet: the route still names it. */
function LoadingProjectPage() {
  useShellPage({})
  return <Nav />
}

function PlainPage() {
  useShellPage({ mainClassName: 'aw-main--page' })
  return <Nav />
}

function renderShell(path: string) {
  const client = new QueryClient()
  client.setQueryData(queryKeys.projects, { projects: [project] })
  const router = createMemoryRouter(
    [
      {
        element: <ShellLayout />,
        children: [
          { path: '/projects/:projectId/channels/:channelId', element: <ChannelPage />, handle: { space: 'project', layout: 'shell' } },
          { path: '/projects/:projectId/threads/:threadId', element: <ThreadPage />, handle: { space: 'project', layout: 'shell' } },
          { path: '/projects/:projectId/pages', element: <LoadingProjectPage />, handle: { space: 'project', layout: 'shell' } },
          { path: '/inbox', element: <PlainPage />, handle: { space: 'home', layout: 'shell' } },
          { path: '/whiteboards/:id', element: <PlainPage />, handle: { space: 'home', layout: 'canvas' } },
          { path: '/search', element: <PlainPage />, handle: { space: 'workspace', layout: 'shell' } },
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
    act(() => fireEvent.click(screen.getByRole('button', { name: 'Agent Workspace: open navigation' })))
    expect(rail).toHaveAttribute('data-mobile-open', 'true')

    act(() => fireEvent.click(screen.getByRole('button', { name: 'Pick a space' })))
    act(() => fireEvent.click(screen.getByRole('button', { name: 'Go to search' })))
    expect(document.getElementById('project-rail')).toBe(rail)
    expect(rail).toHaveAttribute('data-mobile-open', 'true')
  })

  it('shows the space the route names until the page names its own', () => {
    renderShell('/projects/project-a/pages')
    expect(document.getElementById('project-sidebar')).toHaveTextContent('Agent Workspace')
    expect(document.getElementById('project-rail')).toHaveAttribute('data-home-active', 'false')

    fireEvent.click(screen.getByRole('link', { name: 'Inbox' }))
    expect(document.getElementById('project-sidebar')?.textContent ?? '').not.toContain('Agent Workspace')
    expect(document.querySelector('.aw-shell')).toHaveAttribute('data-has-sidebar', 'true')
    expect(document.getElementById('project-rail')).toHaveAttribute('data-home-active', 'true')
  })
})
