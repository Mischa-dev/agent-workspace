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
import { resetCommandState } from './commandStore'
import { ProjectRail } from './ProjectRail'

vi.mock('../auth/session', () => ({
  clearAuthenticatedQueryCache: vi.fn(),
  logout: vi.fn(),
}))

vi.mock('../projects/hooks', () => ({
  useProjects: vi.fn(),
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
        `/projects/${viewerProject.id}`,
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
})
