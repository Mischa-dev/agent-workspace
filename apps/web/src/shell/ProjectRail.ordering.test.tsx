import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionUser } from '../auth/session'
import {
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

const user: SessionUser = {
  id: 'user-a',
  actorId: 'actor-a',
  actorKind: 'human',
  loginName: 'mischa',
  displayName: 'Mischa Nelson',
  capabilities: { createProject: false },
}

function project(
  id: string,
  options: {
    lastOpenedAt: string
    pinned?: boolean
  },
): Project {
  return {
    id,
    organizationId: 'organization-a',
    key: id.toLocaleUpperCase(),
    name: `Project ${id}`,
    archivedAt: null,
    role: 'owner',
    version: 1,
    attentionCount: 0,
    capabilities: {
      view: true,
      manage: true,
      archive: true,
      navigationWrite: true,
    },
    navigation: {
      location: { kind: 'project_overview' },
      href: `/projects/${id}`,
      pinned: options.pinned ?? false,
      lastOpenedAt: options.lastOpenedAt,
      version: 1,
    },
  }
}

describe('ProjectRail ordering', () => {
  beforeEach(() => {
    vi.mocked(useSwitchProject).mockReturnValue({
      mutateAsync: vi.fn(),
    } as unknown as ReturnType<typeof useSwitchProject>)
    vi.mocked(useUpdateProjectNavigation).mockReturnValue({
      mutateAsync: vi.fn(),
    } as unknown as ReturnType<typeof useUpdateProjectNavigation>)
  })

  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('keeps server-ordered pins first, then five recent unpinned Projects and the active fallback', () => {
    const projects = [
      project('recent-3', { lastOpenedAt: '2026-07-31T03:00:00Z' }),
      project('pin-1', {
        lastOpenedAt: '2026-07-31T01:00:00Z',
        pinned: true,
      }),
      project('recent-1', { lastOpenedAt: '2026-07-31T05:00:00Z' }),
      project('pin-2', {
        lastOpenedAt: '2026-07-31T09:00:00Z',
        pinned: true,
      }),
      project('recent-6', { lastOpenedAt: '2026-07-31T00:00:00Z' }),
      project('active-old', { lastOpenedAt: '2026-07-30T23:00:00Z' }),
      project('recent-2', { lastOpenedAt: '2026-07-31T04:00:00Z' }),
      project('recent-5', { lastOpenedAt: '2026-07-31T01:00:00Z' }),
      project('recent-4', { lastOpenedAt: '2026-07-31T02:00:00Z' }),
    ]
    vi.mocked(useProjects).mockReturnValue({
      data: { projects },
      isError: false,
      isPending: false,
    } as ReturnType<typeof useProjects>)

    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    const { container } = render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/home']}>
          <ProjectRail
            currentProjectId="active-old"
            mobileOpen={false}
            onNavigate={vi.fn()}
            user={user}
          />
        </MemoryRouter>
      </QueryClientProvider>,
    )

    const visibleProjectNames = Array.from(
      container.querySelectorAll<HTMLAnchorElement>(
        '#project-rail .project-list a',
      ),
      (link) => link.getAttribute('aria-label'),
    )
    expect(visibleProjectNames).toEqual([
      'Project pin-1',
      'Project pin-2',
      'Project recent-1',
      'Project recent-2',
      'Project recent-3',
      'Project recent-4',
      'Project recent-5',
      'Project active-old',
    ])
    expect(visibleProjectNames).not.toContain('Project recent-6')
  })
})
