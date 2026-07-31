import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { removeProjectFromListCaches } from './hooks'
import type { Project, ProjectListResponse } from './types'

function project(id: string): Project {
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
      pinned: false,
      lastOpenedAt: null,
      version: 1,
    },
  }
}

describe('Project list cache updates', () => {
  it('removes an archived Project from rail and search caches synchronously', () => {
    const queryClient = new QueryClient()
    const retained = project('retained')
    const archived = project('archived')
    queryClient.setQueryData<ProjectListResponse>(queryKeys.projects, {
      projects: [retained, archived],
    })
    queryClient.setQueryData<ProjectListResponse>(
      ['projects', 'search', 'archive'],
      { projects: [archived] },
    )

    removeProjectFromListCaches(queryClient, archived.id)

    expect(
      queryClient.getQueryData<ProjectListResponse>(queryKeys.projects)
        ?.projects,
    ).toEqual([retained])
    expect(
      queryClient.getQueryData<ProjectListResponse>([
        'projects',
        'search',
        'archive',
      ])?.projects,
    ).toEqual([])
  })
})
