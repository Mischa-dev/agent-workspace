import { describe, expect, it } from 'vitest'

import { normalizeNavigation, normalizeProject } from './api'

function projectWithCapabilities(capabilities: unknown) {
  return normalizeProject({
    id: 'project-a',
    organizationId: 'organization-a',
    key: 'A',
    name: 'Project A',
    role: 'member',
    version: 1,
    capabilities,
    navigation: {
      location: { kind: 'project_overview' },
      version: 1,
    },
  })
}

describe('Project wire normalization', () => {
  it('fails every missing capability closed', () => {
    expect(projectWithCapabilities(undefined).capabilities).toEqual({
      view: false,
      manage: false,
      archive: false,
      navigationWrite: false,
    })
    expect(
      projectWithCapabilities({ manage: true }).capabilities,
    ).toEqual({
      view: false,
      manage: true,
      archive: false,
      navigationWrite: false,
    })
  })

  it('accepts only explicit boolean capability grants', () => {
    expect(
      projectWithCapabilities({
        view: 'true',
        navigationWrite: 1,
        archive: true,
      }).capabilities,
    ).toEqual({
      view: false,
      manage: false,
      archive: true,
      navigationWrite: false,
    })
  })

  it('falls back to the Project overview for stale or archived remembered locations', () => {
    expect(
      normalizeNavigation('project-a', {
        location: { kind: 'project_overview' },
        href: '/projects/project-a/threads/archived-thread',
        version: 4,
      }),
    ).toMatchObject({
      location: { kind: 'project_overview' },
      href: '/projects/project-a',
      version: 4,
    })

    expect(
      normalizeNavigation('project-a', {
        location: { kind: 'thread', conversationId: '' },
        href: '/projects/project-a/threads/stale-thread',
        version: 5,
      }),
    ).toMatchObject({
      location: { kind: 'project_overview' },
      href: '/projects/project-a',
      version: 5,
    })

    expect(
      normalizeProject({
        id: 'project-a',
        archivedAt: '2026-07-31T08:00:00Z',
        navigation: {
          location: { kind: 'thread', conversationId: 'thread-a' },
          href: '/projects/project-a/threads/thread-a',
          version: 6,
        },
      }).navigation,
    ).toMatchObject({
      location: { kind: 'project_overview' },
      href: '/projects/project-a',
      version: 6,
    })
  })
})
