import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { resetQueryScope } from '../api/queryScope'
import {
  enqueueProjectNavigationUpdate,
  enqueueProjectSwitch,
} from './navigationQueue'
import type { ProjectNavigation } from './types'

const projectId = 'project-a'

function navigation(
  version: number,
  overrides: Partial<ProjectNavigation> = {},
): ProjectNavigation {
  return {
    location: { kind: 'project_overview' },
    href: `/projects/${projectId}`,
    pinned: false,
    lastOpenedAt: null,
    version,
    ...overrides,
  }
}

function projectWire(current: ProjectNavigation) {
  return {
    id: projectId,
    organizationId: 'organization-a',
    key: 'A',
    name: 'Project A',
    role: 'owner',
    version: 1,
    attentionCount: 0,
    archivedAt: null,
    capabilities: {
      view: true,
      manage: true,
      archive: true,
      navigationWrite: true,
    },
    navigation: current,
  }
}

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' },
    }),
  )
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('serialized Project navigation queue', () => {
  it('rebases route, pin, and switch intents in one versioned sequence', async () => {
    const queryClient = new QueryClient()
    let current = navigation(1)
    queryClient.setQueryData(['projects', 'search', 'project'], {
      projects: [projectWire(current)],
    })
    const writes: Array<{
      kind: 'route' | 'pin' | 'switch'
      body: Record<string, unknown>
    }> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith(`/${projectId}/navigation`) && method === 'GET') {
            return jsonResponse(current)
          }
          if (path.endsWith(`/${projectId}/navigation`) && method === 'PUT') {
            const body = JSON.parse(String(init?.body)) as Record<string, unknown>
            writes.push({
              kind: 'pinned' in body ? 'pin' : 'route',
              body,
            })
            current = {
              ...current,
              location:
                (body.location as ProjectNavigation['location']) ??
                current.location,
              href:
                (body.location as ProjectNavigation['location'])?.kind ===
                'settings'
                  ? `/projects/${projectId}/settings`
                  : current.href,
              pinned:
                typeof body.pinned === 'boolean'
                  ? body.pinned
                  : current.pinned,
              version: current.version + 1,
            }
            return jsonResponse(current)
          }
          if (path.endsWith(`/${projectId}/switch`) && method === 'POST') {
            const body = JSON.parse(String(init?.body)) as Record<string, unknown>
            writes.push({ kind: 'switch', body })
            current = {
              ...current,
              lastOpenedAt: '2026-07-31T08:00:00Z',
              version: current.version + 1,
            }
            return jsonResponse({
              project: projectWire(current),
              navigation: current,
              href: current.href,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )

    const route = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      location: { kind: 'settings' },
    })
    const pin = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      pinned: true,
    })
    const switched = enqueueProjectSwitch(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
    })

    await Promise.all([route, pin, switched])

    expect(writes.map(({ kind }) => kind)).toEqual([
      'route',
      'pin',
      'switch',
    ])
    expect(writes.map(({ body }) => body.expectedVersion)).toEqual([1, 2, 3])
    expect(writes[1].body).toMatchObject({
      location: { kind: 'settings' },
      pinned: true,
    })
    expect(
      queryClient.getQueryData<ProjectNavigation>([
        'project',
        projectId,
        'navigation',
      ]),
    ).toMatchObject({
      location: { kind: 'settings' },
      pinned: true,
      version: 4,
    })
    expect(
      queryClient.getQueryData<{
        projects: Array<{ navigation: ProjectNavigation }>
      }>(['projects', 'search', 'project'])?.projects[0].navigation,
    ).toMatchObject({
      location: { kind: 'settings' },
      pinned: true,
      version: 4,
    })
  })

  it('retries one conflict, rejects terminal failure, then runs the next intent', async () => {
    const queryClient = new QueryClient()
    let current = navigation(1)
    let navigationReads = 0
    const writeVersions: number[] = []
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith(`/${projectId}/navigation`) && method === 'GET') {
            navigationReads += 1
            if (navigationReads === 2) current = navigation(2)
            return jsonResponse(current)
          }
          if (path.endsWith(`/${projectId}/navigation`) && method === 'PUT') {
            const body = JSON.parse(String(init?.body)) as {
              expectedVersion: number
              pinned?: boolean
            }
            writeVersions.push(body.expectedVersion)
            if (writeVersions.length === 1) {
              return jsonResponse(
                {
                  error: {
                    code: 'version_conflict',
                    details: { currentVersion: 2 },
                    message: 'Navigation changed.',
                  },
                },
                409,
              )
            }
            if (writeVersions.length === 2) {
              return jsonResponse(
                {
                  error: {
                    code: 'internal_error',
                    message: 'Navigation could not be saved.',
                  },
                },
                500,
              )
            }
            current = navigation(3, { pinned: body.pinned ?? false })
            return jsonResponse(current)
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )

    const failedRoute = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      location: { kind: 'settings' },
    })
    const followingPin = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      pinned: true,
    })

    await expect(failedRoute).rejects.toMatchObject({
      status: 500,
      code: 'internal_error',
    })
    await expect(followingPin).resolves.toMatchObject({
      pinned: true,
      version: 3,
    })
    expect(writeVersions).toEqual([1, 2, 2])
  })

  it('cancels queued old-Actor intents when the query scope resets', async () => {
    const queryClient = new QueryClient()
    const current = navigation(1)
    let releaseWrite: (() => void) | undefined
    let announceWrite: (() => void) | undefined
    const writeStarted = new Promise<void>((resolve) => {
      announceWrite = resolve
    })
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith(`/${projectId}/navigation`) && method === 'GET') {
          return jsonResponse(current)
        }
        if (path.endsWith(`/${projectId}/navigation`) && method === 'PUT') {
          announceWrite?.()
          return new Promise((resolve) => {
            releaseWrite = () =>
              resolve(
                new Response(
                  JSON.stringify(navigation(2, { pinned: true })),
                  { headers: { 'content-type': 'application/json' } },
                ),
              )
          })
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)

    const active = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      pinned: true,
    })
    const queued = enqueueProjectNavigationUpdate(queryClient, {
      projectId,
      expectedVersion: 1,
      navigation: current,
      location: { kind: 'settings' },
    })
    const activeResult = expect(active).rejects.toThrow(/session changed/)
    const queuedResult = expect(queued).rejects.toThrow(/session changed/)

    await writeStarted
    resetQueryScope()
    releaseWrite?.()

    await activeResult
    await queuedResult
    expect(
      fetchMock.mock.calls.filter(
        ([path, init]) =>
          String(path).endsWith(`/${projectId}/navigation`) &&
          init?.method === 'PUT',
      ),
    ).toHaveLength(1)
  })

  it('does not wait for a stalled active search before switching Projects', async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    })
    const current = navigation(1)
    let railReads = 0
    let searchReads = 0
    let releaseSearch: (() => void) | undefined
    const stalledSearch = new Promise<{ projects: ReturnType<typeof projectWire>[] }>(
      (resolve) => {
        releaseSearch = () => resolve({ projects: [projectWire(current)] })
      },
    )
    const railObserver = new QueryObserver(queryClient, {
      queryKey: ['projects', 'rail'],
      queryFn: async () => {
        railReads += 1
        return { projects: [projectWire(current)] }
      },
      initialData: { projects: [projectWire(current)] },
    })
    const searchObserver = new QueryObserver(queryClient, {
      queryKey: ['projects', 'search', 'project'],
      queryFn: () => {
        searchReads += 1
        return stalledSearch
      },
      initialData: { projects: [projectWire(current)] },
    })
    const unsubscribeRail = railObserver.subscribe(() => undefined)
    const unsubscribeSearch = searchObserver.subscribe(() => undefined)
    let announceSwitch: (() => void) | undefined
    const switchStarted = new Promise<void>((resolve) => {
      announceSwitch = resolve
    })
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith(`/${projectId}/navigation`) && method === 'GET') {
            return jsonResponse(current)
          }
          if (path.endsWith(`/${projectId}/switch`) && method === 'POST') {
            announceSwitch?.()
            const switchedNavigation = navigation(2, {
              lastOpenedAt: '2026-07-31T08:00:00Z',
            })
            return jsonResponse({
              project: projectWire(switchedNavigation),
              navigation: switchedNavigation,
              href: switchedNavigation.href,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )

    const switched = enqueueProjectSwitch(queryClient, {
      projectId,
      expectedVersion: current.version,
      navigation: current,
    })
    await expect(
      Promise.race([
        switchStarted.then(() => 'started'),
        new Promise<string>((_, reject) => {
          window.setTimeout(
            () => reject(new Error('Project switch waited for stalled search')),
            250,
          )
        }),
      ]),
    ).resolves.toBe('started')
    await expect(switched).resolves.toMatchObject({
      navigation: { version: 2 },
    })
    expect(railReads).toBe(1)
    expect(searchReads).toBe(1)

    releaseSearch?.()
    await expect
      .poll(
        () =>
          queryClient.getQueryData<{
            projects: Array<{ navigation: ProjectNavigation }>
          }>(['projects', 'search', 'project'])?.projects[0].navigation.version,
      )
      .toBe(2)
    expect(
      queryClient.getQueryData<{
        projects: Array<{ navigation: ProjectNavigation }>
      }>(['projects', 'rail'])?.projects[0].navigation.version,
    ).toBe(2)
    unsubscribeRail()
    unsubscribeSearch()
    await queryClient.cancelQueries({ queryKey: ['projects'] })
  })
})
