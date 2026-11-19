import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionUser } from '../auth/session'
import {
  useProjects,
  useSwitchProject,
} from '../projects/hooks'
import type { Project } from '../projects/types'
import { WorkspaceContext } from '../app/workspace'
import { resetCommandState } from './commandStore'
import { ProjectRail } from './ProjectRail'

const session = vi.hoisted(() => ({
  organizations: [{ id: 'org-1', name: 'Mischa', slug: 'mischa', role: 'owner' }] as { id: string; name: string; slug: string; role: string }[],
}))

vi.mock('../auth/session', () => ({
  clearAuthenticatedQueryCache: vi.fn(),
  logout: vi.fn(),
  useAuthenticatedSession: () => session,
}))

vi.mock('../projects/hooks', () => ({
  useProjects: vi.fn(),
  useSetRailOrder: () => ({ mutate: vi.fn() }),
  useSwitchProject: vi.fn(),
}))

const viewerProject: Project = {
  id: '019fb600-0000-7000-8000-000000000001',
  organizationId: '019fb600-0000-7000-8000-000000000002',
  key: 'VIEW',
  name: 'Viewer Project',
  archivedAt: null,
  role: 'viewer',
  version: 1,
  attentionCount: 0,
  capabilities: {
    view: true,
    manage: false,
    archive: false,
    navigationWrite: false,
  },
  navigation: {
    location: { kind: 'project_overview' },
    href: '/projects/019fb600-0000-7000-8000-000000000001',
    pinned: false,
    lastOpenedAt: '2026-07-31T06:00:00Z',
    version: 1,
  },
}

const user: SessionUser = {
  id: '019fb600-0000-7000-8000-000000000003',
  actorId: '019fb600-0000-7000-8000-000000000004',
  actorKind: 'human',
  loginName: 'viewer',
  displayName: 'Project Viewer',
  capabilities: { createProject: false },
}

function LocationProbe() {
  return <output aria-label="Current route">{useLocation().pathname}</output>
}

describe('ProjectRail', () => {
  const switchProject = vi.fn()

  beforeEach(() => {
    switchProject.mockReset()
    vi.mocked(useProjects).mockReturnValue({
      data: { projects: [viewerProject] },
      isError: false,
      isPending: false,
    } as ReturnType<typeof useProjects>)
    vi.mocked(useSwitchProject).mockReturnValue({
      mutateAsync: switchProject,
    } as unknown as ReturnType<typeof useSwitchProject>)
  })

  afterEach(cleanup)

  it('navigates a Viewer without attempting the forbidden switch write', async () => {
    const onNavigate = vi.fn()
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/home']}>
          <ProjectRail
            currentProjectId={null}
            mobileOpen={false}
            onNavigate={onNavigate}
            user={user}
          />
          <LocationProbe />
        </MemoryRouter>
      </QueryClientProvider>,
    )

    fireEvent.click(screen.getByRole('link', { name: 'Viewer Project' }))

    await waitFor(() =>
      expect(screen.getByLabelText('Current route')).toHaveTextContent(
        `/s/${viewerProject.id}`,
      ),
    )
    expect(switchProject).not.toHaveBeenCalled()
    expect(onNavigate).toHaveBeenCalledOnce()
  })

  it('opens the command menu from its search tile', () => {
    resetCommandState()
    const onNavigate = vi.fn()
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/home']}>
          <ProjectRail
            currentProjectId={null}
            mobileOpen
            onNavigate={onNavigate}
            user={user}
          />
        </MemoryRouter>
      </QueryClientProvider>,
    )

    const searchButton = screen.getByRole('button', { name: 'Search and commands' })
    expect(searchButton).toHaveAttribute('aria-expanded', 'false')
    fireEvent.click(searchButton)
    expect(searchButton).toHaveAttribute('aria-expanded', 'true')
    // The drawer closes so the menu isn't stacked on top of it on phones.
    expect(onNavigate).toHaveBeenCalledOnce()
    act(() => resetCommandState())
  })
  it('switches workspace from the rail, with what needs you in the others (W1.14a)', async () => {
    const acme = { id: viewerProject.organizationId, name: 'Acme Labs', slug: 'acme', role: 'owner' as const }
    const beta = { id: 'org-beta', name: 'Beta', slug: 'beta', role: 'member' as const }
    session.organizations = [acme, beta]
    const betaProject: Project = { ...viewerProject, id: 'project-beta', organizationId: beta.id, key: 'BETA', name: 'Beta Project', attentionCount: 3 }
    vi.mocked(useProjects).mockReturnValue({
      data: { projects: [viewerProject, betaProject] },
      isError: false,
      isPending: false,
    } as ReturnType<typeof useProjects>)
    render(
      <QueryClientProvider client={new QueryClient()}>
        <MemoryRouter initialEntries={['/w/acme/home']}>
          <WorkspaceContext.Provider value={acme}>
            <ProjectRail currentProjectId={null} mobileOpen={false} onNavigate={vi.fn()} user={user} />
          </WorkspaceContext.Provider>
          <LocationProbe />
        </MemoryRouter>
      </QueryClientProvider>,
    )
    try {
      // Only this workspace's projects, and its own Home.
      expect(screen.getByRole('link', { name: 'Viewer Project' })).toBeVisible()
      expect(screen.queryByRole('link', { name: /Beta Project/ })).toBeNull()
      expect(screen.getByRole('link', { name: 'Home' })).toHaveAttribute('href', '/w/acme/home')
      fireEvent.click(screen.getByRole('button', { name: 'Workspace: Acme Labs, 3 unread in others' }))
      expect(await screen.findByRole('menuitemcheckbox', { name: 'Acme Labs' })).toHaveAttribute('aria-checked', 'true')
      fireEvent.click(screen.getByRole('menuitemcheckbox', { name: 'Beta, 3 unread' }))
      await waitFor(() => expect(screen.getByLabelText('Current route')).toHaveTextContent('/w/beta/home'))
    } finally {
      session.organizations = [{ id: 'org-1', name: 'Mischa', slug: 'mischa', role: 'owner' }]
    }
  })
})
