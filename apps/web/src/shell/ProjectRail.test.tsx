import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionUser } from '../auth/session'
import {
  useProjectSearch,
  useProjects,
  useSwitchProject,
  useUpdateProjectNavigation,
} from '../projects/hooks'
import type { Project } from '../projects/types'
import { ProjectRail } from './ProjectRail'

vi.mock('../auth/session', () => ({
  clearAuthenticatedQueryCache: vi.fn(),
  logout: vi.fn(),
}))

vi.mock('../projects/hooks', () => ({
  useProjectSearch: vi.fn(),
  useProjects: vi.fn(),
  useSwitchProject: vi.fn(),
  useUpdateProjectNavigation: vi.fn(),
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
  const updateNavigation = vi.fn()

  beforeEach(() => {
    switchProject.mockReset()
    updateNavigation.mockReset()
    vi.mocked(useProjects).mockReturnValue({
      data: { projects: [viewerProject] },
      isError: false,
      isPending: false,
    } as ReturnType<typeof useProjects>)
    vi.mocked(useProjectSearch).mockReturnValue({
      data: { projects: [viewerProject] },
      isFetching: false,
    } as ReturnType<typeof useProjectSearch>)
    vi.mocked(useSwitchProject).mockReturnValue({
      mutateAsync: switchProject,
    } as unknown as ReturnType<typeof useSwitchProject>)
    vi.mocked(useUpdateProjectNavigation).mockReturnValue({
      mutateAsync: updateNavigation,
    } as unknown as ReturnType<typeof useUpdateProjectNavigation>)
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

  it('closes nested Project search on Escape without propagating to its drawer', async () => {
    const onNavigate = vi.fn()
    const documentEscape = vi.fn()
    document.addEventListener('keydown', documentEscape)
    const applicationRoot = document.createElement('div')
    applicationRoot.id = 'root'
    document.body.appendChild(applicationRoot)
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
      { container: applicationRoot },
    )

    const searchButton = screen.getByRole('button', {
      name: 'Search Projects',
    })
    fireEvent.click(searchButton)
    expect(searchButton).toHaveAttribute('aria-expanded', 'true')
    expect(applicationRoot).toHaveAttribute('inert')

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' })

    await waitFor(() =>
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
    )
    await waitFor(() => expect(searchButton).toHaveFocus())
    expect(searchButton).toHaveAttribute('aria-expanded', 'false')
    expect(applicationRoot).not.toHaveAttribute('inert')
    expect(documentEscape).not.toHaveBeenCalled()
    expect(onNavigate).not.toHaveBeenCalled()
    document.removeEventListener('keydown', documentEscape)
  })
})
