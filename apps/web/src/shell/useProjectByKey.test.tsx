import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { describe, expect, it } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import type { Project, ProjectListResponse } from '../projects/types'
import { useProjectByKey } from './useProjectByKey'

function project(id: string, organizationId: string): Project {
  return {
    id,
    organizationId,
    key: 'AWD',
    name: `Project ${id}`,
    archivedAt: null,
    role: 'owner',
    version: 1,
    attentionCount: 0,
    capabilities: { view: true, manage: true, archive: true, navigationWrite: true },
    navigation: {
      location: { kind: 'project_overview' },
      href: `/projects/${id}`,
      pinned: false,
      lastOpenedAt: null,
      version: 1,
    },
  }
}

describe('useProjectByKey', () => {
  it('picks no Project when two organizations share the key', async () => {
    const first = project('first', 'organization-a')
    const second = project('second', 'organization-b')
    const queryClient = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
    queryClient.setQueryData<ProjectListResponse>(queryKeys.projects, { projects: [first] })
    // The second one is beyond the rail's first 100: only the server finds it.
    queryClient.setQueryData<ProjectListResponse>(['projects', 'search', 'AWD'], { projects: [first, second] })
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    )

    const ambiguous = renderHook(() => useProjectByKey('awd'), { wrapper })
    await waitFor(() => expect(ambiguous.result.current.pending).toBe(false))
    expect(ambiguous.result.current.project).toBeNull()
    expect(ambiguous.result.current.matches.map((match) => match.id)).toEqual(['first', 'second'])

    const preferred = renderHook(() => useProjectByKey('AWD', 'second'), { wrapper })
    await waitFor(() => expect(preferred.result.current.project?.id).toBe('second'))
  })
})
