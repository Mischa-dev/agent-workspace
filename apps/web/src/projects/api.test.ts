import { afterEach, describe, expect, it, vi } from 'vitest'

import { archiveProject, createProject, normalizeNavigation, normalizeProject } from './api'

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
      href: '/w/-/s/project-a',
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
      href: '/w/-/s/project-a',
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
      href: '/w/-/s/project-a',
      version: 6,
    })
  })
})

describe('archiveProject', () => {
  afterEach(() => vi.unstubAllGlobals())

  const wire = (archivedAt: string | null) => ({
    id: 'project-a',
    organizationId: 'organization-a',
    key: 'A',
    name: 'Project A',
    role: 'owner',
    version: archivedAt ? 2 : 1,
    archivedAt,
    capabilities: {},
    navigation: { location: { kind: 'project_overview' }, version: 1 },
  })
  const json = (body: unknown, status = 200) =>
    Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }))
  // The archive's answer was lost: the retry finds the space closed.
  const closed = () => json({ outcome: 'not_found', message: 'That space' }, 404)

  it('takes a retry that finds the space already archived as done', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) =>
        String(input).endsWith('/ops/space_archive') ? closed() : json(wire('2026-11-21T21:00:00Z')),
      ),
    )
    const project = await archiveProject('project-a', 1)
    expect(project.archivedAt).toBe('2026-11-21T21:00:00Z')
  })

  it('still fails when the space is not archived', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) =>
        String(input).endsWith('/ops/space_archive') ? closed() : json(wire(null)),
      ),
    )
    await expect(archiveProject('project-a', 1)).rejects.toMatchObject({ code: 'not_found' })
  })
})

describe('createProject', () => {
  afterEach(() => vi.unstubAllGlobals())

  const json = (body: unknown, status = 200) =>
    Promise.resolve(new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }))
  const input = { organizationId: 'organization-a', key: 'NEW', name: 'New', idempotencyKey: 'create-1' }

  it('says a create held for review waits, without reading a project', async () => {
    const fetchMock = vi.fn(() => json({ outcome: 'ok', value: { status: 'pending_review', requestId: 'review-1' } }))
    vi.stubGlobal('fetch', fetchMock)
    await expect(createProject(input)).rejects.toThrow(/waits for a reviewer/)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('keeps a made space when the read after it fails', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((url: RequestInfo | URL) =>
        String(url).endsWith('/ops/space_create')
          ? json({ outcome: 'ok', value: { id: 'space-new', key: 'NEW', name: 'New', version: 1 } })
          : json({ error: { code: 'internal_error', message: 'Down' } }, 500),
      ),
    )
    const project = await createProject(input)
    expect(project).toMatchObject({ id: 'space-new', organizationId: 'organization-a', key: 'NEW' })
  })
})
