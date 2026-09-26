import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import axe from 'axe-core'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type {
  OrganizationSummary,
  SessionResponse,
} from '../auth/session'
import { App } from './App'
import { coalesceDelta, coalesceDeltaBatch } from './streaming'

const organization: OrganizationSummary = {
  id: '019fb600-0000-7000-8000-000000000000',
  name: 'Mischa',
  slug: 'mischa',
  role: 'owner',
}
const projectAId = '019fb600-0000-7000-8000-000000000001'
const projectBId = '019fb600-0000-7000-8000-000000000002'
const threadId = '019fb600-0000-7000-8000-000000000003'
const adminOrganizationId = '019fb600-0000-7000-8000-000000000007'
const ownerOrganizationId = '019fb600-0000-7000-8000-000000000008'
const memberOrganizationId = '019fb600-0000-7000-8000-000000000009'
const user = {
  id: '019fb600-0000-7000-8000-000000000004',
  actorId: '019fb600-0000-7000-8000-000000000005',
  actorKind: 'human',
  loginName: 'mischa',
  displayName: 'Mischa Nelson',
  capabilities: { createProject: true },
}
const authenticatedSession = {
  authenticated: true,
  user,
  organizations: [organization],
}
const signedOutSession = {
  authenticated: false,
  user: null,
  organizations: [],
}

function project(
  id: string,
  key: string,
  name: string,
  overrides: Record<string, unknown> = {},
) {
  return {
    id,
    organizationId: organization.id,
    key,
    name,
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
      lastOpenedAt: '2026-07-31T05:00:00Z',
      version: 1,
    },
    ...overrides,
  }
}

const projectA = project(projectAId, 'AW', 'Agent Workspace', {
  attentionCount: 3,
  navigation: {
    location: { kind: 'thread', conversationId: threadId },
    href: `/projects/${projectAId}/threads/${threadId}`,
    pinned: true,
    lastOpenedAt: '2026-07-31T06:00:00Z',
    version: 2,
  },
})
const projectB = project(projectBId, 'BETA', 'Beta Project')

const timeline = {
  conversation: {
    id: threadId,
    projectId: projectAId,
    kind: 'thread',
    parentChannelId: 'channel-1',
    name: 'M0 walking skeleton',
  },
  entries: [
    {
      id: 'entry-1',
      sequence: 1,
      kind: 'message',
      createdAt: '2026-07-31T04:00:00Z',
      message: {
        id: 'message-1',
        author: {
          id: 'actor-1',
          kind: 'human',
          displayName: 'Mischa Nelson',
          handle: 'mischa',
        },
        content: {
          version: 1,
          nodes: [
            { type: 'text', text: '<img src=x onerror=alert(1)>' },
          ],
        },
        plainText: '<img src=x onerror=alert(1)>',
      },
      workerEvent: null,
    },
    {
      id: 'entry-2',
      sequence: 2,
      kind: 'worker_event',
      createdAt: '2026-07-31T04:01:00Z',
      message: null,
      workerEvent: {
        id: 'event-1',
        agentRunId: 'run-1',
        kind: 'approval_requested',
        summary: 'The run is waiting for an approval.',
        payload: {},
      },
    },
  ],
  activeRun: {
    id: 'run-1',
    state: 'blocked',
    repositoryId: 'repo-1',
    repositoryWorkspaceId: 'workspace-1',
    providerSessionId: null,
    failureReason: null,
    createdAt: '2026-07-31T04:00:00Z',
    updatedAt: '2026-07-31T04:01:00Z',
    capabilities: { interrupt: true },
  },
  approvals: [
    {
      id: 'approval-1',
      agentRunId: 'run-1',
      kind: 'approval',
      summary: 'Allow the isolated file change?',
      options: [
        { id: 'approve', label: 'Allow once' },
        { id: 'deny', label: 'Deny' },
      ],
      status: 'pending',
      response: null,
      createdAt: '2026-07-31T04:01:00Z',
      capabilities: { respond: true },
    },
  ],
  capabilities: { workerInvoke: true },
}

class WebSocketStub extends EventTarget {
  static instances: WebSocketStub[] = []
  close = vi.fn()

  constructor(public readonly url: string) {
    super()
    WebSocketStub.instances.push(this)
  }
}

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: {
        'content-type': 'application/json',
        'x-request-id': 'request-test',
      },
    }),
  )
}

function noContentResponse() {
  return Promise.resolve(new Response(null, { status: 204 }))
}

function installStandardApi(
  session: SessionResponse = authenticatedSession,
) {
  let currentSession = session
  const fetchMock = vi.fn(
    (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const path = String(input)
      const method = init?.method ?? 'GET'
      if (path.endsWith('/api/v1/auth/session')) {
        return jsonResponse(currentSession)
      }
      if (path.endsWith('/api/v1/auth/login')) {
        currentSession = authenticatedSession
        return jsonResponse(currentSession)
      }
      if (path.endsWith('/api/v1/auth/logout')) return noContentResponse()
      if (path.startsWith('/api/v1/projects?') && method === 'GET') {
        return jsonResponse({ projects: [projectA, projectB] })
      }
      if (path.endsWith(`/projects/${projectAId}`)) {
        return jsonResponse(projectA)
      }
      if (path.endsWith(`/projects/${projectBId}`)) {
        return jsonResponse(projectB)
      }
      if (path.endsWith(`/projects/${projectAId}/navigation`)) {
        return jsonResponse(projectA.navigation)
      }
      if (path.endsWith(`/projects/${projectBId}/navigation`)) {
        return jsonResponse(projectB.navigation)
      }
      if (path.endsWith(`/projects/${projectAId}/switch`)) {
        return jsonResponse({
          project: projectA,
          navigation: projectA.navigation,
          href: projectA.navigation.href,
        })
      }
      if (path.endsWith(`/projects/${projectBId}/switch`)) {
        return jsonResponse({
          project: projectB,
          navigation: projectB.navigation,
          href: projectB.navigation.href,
        })
      }
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(timeline)
      }
      if (path.includes('/agent-approvals/approval-1/respond')) {
        return jsonResponse({ ...timeline.approvals[0], status: 'resolved' })
      }
      throw new Error(`Unexpected request: ${method} ${path}`)
    },
  )
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

describe('M1 authenticated shell', () => {
  beforeEach(() => {
    WebSocketStub.instances = []
    vi.stubGlobal('WebSocket', WebSocketStub)
    window.history.replaceState({}, '', '/home')
    document.cookie =
      'agent_workspace_csrf=test-csrf; Path=/; SameSite=Strict'
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    document.cookie = 'agent_workspace_csrf=; Max-Age=0; Path=/'
    window.history.replaceState({}, '', '/')
  })

  it('redirects a signed-out protected route to the sign-in boundary', async () => {
    installStandardApi(signedOutSession)
    const { container } = render(<App />)

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    expect(
      screen.getByText(/Create the first account on the server/i),
    ).toBeVisible()
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
  })

  it('signs in without writing credentials or tokens to browser storage', async () => {
    window.history.replaceState({}, '', '/sign-in?returnTo=%2Fhome')
    const storageSpy = vi.spyOn(Storage.prototype, 'setItem')
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        if (path.endsWith('/auth/session')) return jsonResponse(signedOutSession)
        if (path.endsWith('/auth/login')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?')) {
          return jsonResponse({ projects: [projectA] })
        }
        throw new Error(`Unexpected request: ${String(init?.method)} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(await screen.findByLabelText('Login name'), {
      target: { value: 'mischa' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByRole('heading', { name: /Welcome back/ }),
    ).toBeVisible()
    const loginCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith('/auth/login'),
    )
    expect(loginCall?.[1]).toEqual(
      expect.objectContaining({
        credentials: 'same-origin',
        method: 'POST',
      }),
    )
    expect(storageSpy).not.toHaveBeenCalled()
    storageSpy.mockRestore()
  })

  it('signs out through the CSRF-protected session action', async () => {
    const fetchMock = installStandardApi()
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Account: Mischa Nelson' }),
    )
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    const logoutCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith('/auth/logout'),
    )
    expect(logoutCall?.[1]).toEqual(
      expect.objectContaining({
        credentials: 'same-origin',
        method: 'POST',
      }),
    )
    expect(new Headers(logoutCall?.[1]?.headers).get('x-csrf-token')).toBe(
      'test-csrf',
    )
  })

  it('drops prior Actor caches after an automatic 401 before another login', async () => {
    const nextActorSession: SessionResponse = {
      ...authenticatedSession,
      user: {
        ...user,
        id: '019fb600-0000-7000-8000-00000000000a',
        actorId: '019fb600-0000-7000-8000-00000000000b',
        loginName: 'next',
        displayName: 'Next Actor',
      },
    }
    let nextActor = false
    let projectListReads = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.endsWith('/auth/login')) {
            nextActor = true
            return jsonResponse(nextActorSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            projectListReads += 1
            return jsonResponse({
              projects: nextActor ? [projectB] : [projectA],
            })
          }
          if (
            path.endsWith(`/projects/${projectAId}/navigation`) &&
            method === 'GET'
          ) {
            return jsonResponse(
              {
                error: {
                  code: 'authentication_required',
                  message: 'Sign in to continue.',
                },
              },
              401,
            )
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()

    fireEvent.change(screen.getByLabelText('Login name'), {
      target: { value: 'next' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByRole('heading', { name: /Welcome back, Next Actor/ }),
    ).toBeVisible()
    expect(
      await within(await screen.findByRole('navigation', { name: 'Projects' })).findByRole('link', {
        name: 'Beta Project',
      }),
    ).toBeVisible()
    expect(
      screen.queryByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    ).not.toBeInTheDocument()
    expect(projectListReads).toBeGreaterThanOrEqual(2)
  })

  it('renders server-backed rail, attention labels, search, and accessible drawers', async () => {
    installStandardApi()
    const { container } = render(<App />)

    expect(
      await screen.findByRole('heading', { name: /Welcome back/ }),
    ).toBeVisible()
    expect(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    ).toBeVisible()
    const searchButton = screen.getByRole('button', { name: 'Search Projects' })
    fireEvent.click(searchButton)
    const search = screen.getByRole('searchbox', { name: 'Search Projects' })
    expect(search).toHaveFocus()
    expect(search.closest('label')).toHaveClass('aw-switcher__search')
    fireEvent.change(search, { target: { value: 'beta' } })
    const dialog = screen.getByRole('dialog')
    expect(within(dialog).getByRole('link', { name: /Beta Project/ })).toBeVisible()
    fireEvent.keyDown(dialog, { key: 'Escape' })
    await waitFor(() => expect(searchButton).toHaveFocus())

    const projectsButton = screen.getByRole('button', { name: 'Projects' })
    fireEvent.click(projectsButton)
    expect(screen.getByRole('navigation', { name: 'Projects' })).toHaveAttribute(
      'data-mobile-open',
      'true',
    )
    fireEvent.keyDown(document, { key: 'Escape' })
    await waitFor(() => expect(projectsButton).toHaveFocus())
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
  })

  it('finishes the Project rail when a hard-route location sync cancels its cold read', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000006'
    const postgresTimestampProjects = [
      {
        ...projectA,
        navigation: {
          ...projectA.navigation,
          lastOpenedAt: '2026-07-31 06:00:00.123456+00',
        },
      },
      {
        ...projectB,
        navigation: {
          ...projectB.navigation,
          lastOpenedAt: '2026-07-31 05:00:00.123456+00',
        },
      },
      project(projectCId, 'GAMMA', 'Gamma Project', {
        navigation: {
          location: { kind: 'project_overview' },
          href: `/projects/${projectCId}`,
          pinned: false,
          lastOpenedAt: '2026-07-31 04:00:00.123456+00',
          version: 1,
        },
      }),
    ]
    window.history.replaceState({}, '', `/projects/${projectBId}`)
    let projectListReads = 0
    let releaseInitialList: ((response: Response) => void) | undefined
    const initialListRead = new Promise<Response>((resolve) => {
      releaseInitialList = resolve
    })
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            projectListReads += 1
            if (projectListReads === 1) return initialListRead
            releaseInitialList?.(
              new Response(JSON.stringify({ projects: postgresTimestampProjects }), {
                headers: { 'content-type': 'application/json' },
              }),
            )
            return jsonResponse({ projects: postgresTimestampProjects })
          }
          if (path.endsWith(`/projects/${projectBId}`)) {
            return jsonResponse(projectB)
          }
          if (
            path.endsWith(`/projects/${projectBId}/navigation`) &&
            method === 'GET'
          ) {
            return jsonResponse(projectB.navigation)
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'Beta Project' }),
    ).toBeVisible()
    expect(
      await screen.findByRole('link', { name: 'Gamma Project' }),
    ).toBeVisible()
    expect(screen.queryByLabelText('Loading Projects')).not.toBeInTheDocument()
    expect(projectListReads).toBe(2)
  })

  it('restores independent A to B to A locations after a client reload', async () => {
    installStandardApi()
    const firstRender = render(<App />)
    const projectALink = await screen.findByRole('link', {
      name: 'Agent Workspace, 3 items need attention',
    })
    fireEvent.click(projectALink)
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(
      `/projects/${projectAId}/threads/${threadId}`,
    )

    fireEvent.click(screen.getByRole('link', { name: 'Home' }))
    fireEvent.click(
      await within(screen.getByRole('navigation', { name: 'Projects' })).findByRole('link', {
        name: 'Beta Project',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Beta Project' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(`/projects/${projectBId}`)

    fireEvent.click(screen.getByRole('link', { name: 'Home' }))
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()

    firstRender.unmount()
    installStandardApi(signedOutSession)
    window.history.replaceState({}, '', '/sign-in')
    render(<App />)
    fireEvent.change(await screen.findByLabelText('Login name'), {
      target: { value: 'mischa' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'correct horse battery staple' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))
    expect(
      await screen.findByRole('heading', { name: /Welcome back/ }),
    ).toBeVisible()
    fireEvent.click(
      await screen.findByRole('link', {
        name: 'Agent Workspace, 3 items need attention',
      }),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(window.location.pathname).toBe(
      `/projects/${projectAId}/threads/${threadId}`,
    )
  })

  it('keeps one idempotency key while retrying Project creation', async () => {
    window.history.replaceState({}, '', '/projects/new')
    let createAttempts = 0
    const idempotencyKeys: Array<string | null> = []
    const created = project(
      '019fb600-0000-7000-8000-000000000099',
      'TRI',
      'Trident',
    )
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({ projects: [] })
        }
        if (path.endsWith('/projects') && method === 'POST') {
          idempotencyKeys.push(
            new Headers(init?.headers).get('idempotency-key'),
          )
          createAttempts += 1
          return createAttempts === 1
            ? jsonResponse(
                {
                  error: {
                    code: 'internal_error',
                    message: 'The request could not be completed.',
                  },
                },
                500,
              )
            : jsonResponse(created, 201)
        }
        if (path.endsWith(`/${created.id}/switch`)) {
          return jsonResponse({
            project: created,
            navigation: created.navigation,
            href: `/projects/${created.id}`,
          })
        }
        if (path.endsWith(`/${created.id}`)) return jsonResponse(created)
        if (path.endsWith(`/${created.id}/navigation`)) {
          return jsonResponse(created.navigation)
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(await screen.findByLabelText('Project name'), {
      target: { value: 'Trident' },
    })
    fireEvent.change(screen.getByLabelText('Project key'), {
      target: { value: 'tri' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Project' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'The request could not be completed.',
    )
    fireEvent.click(screen.getByRole('button', { name: 'Create Project' }))

    await waitFor(() => expect(createAttempts).toBe(2))
    expect(idempotencyKeys[0]).toBeTruthy()
    expect(idempotencyKeys[1]).toBe(idempotencyKeys[0])
  })

  it('offers Project creation only in Owner and Admin Organizations', async () => {
    window.history.replaceState({}, '', '/projects/new')
    const sessionWithMixedRoles: SessionResponse = {
      ...authenticatedSession,
      organizations: [
        {
          id: memberOrganizationId,
          name: 'Member Organization',
          slug: 'member',
          role: 'member',
        },
        {
          id: adminOrganizationId,
          name: 'Admin Organization',
          slug: 'admin',
          role: 'admin',
        },
        {
          id: ownerOrganizationId,
          name: 'Owner Organization',
          slug: 'owner',
          role: 'owner',
        },
      ],
    }
    installStandardApi(sessionWithMixedRoles)
    render(<App />)

    const selector = await screen.findByLabelText('Organization')
    expect(selector).toHaveValue(adminOrganizationId)
    expect(
      within(selector).queryByRole('option', {
        name: 'Member Organization',
      }),
    ).not.toBeInTheDocument()
    expect(
      within(selector).getByRole('option', { name: 'Admin Organization' }),
    ).toBeVisible()
    expect(
      within(selector).getByRole('option', { name: 'Owner Organization' }),
    ).toBeVisible()
  })

  it('blocks direct Project creation when no eligible Organization exists', async () => {
    window.history.replaceState({}, '', '/projects/new')
    installStandardApi({
      ...authenticatedSession,
      organizations: [
        {
          id: memberOrganizationId,
          name: 'Member Organization',
          slug: 'member',
          role: 'member',
        },
      ],
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', {
        name: 'Project creation unavailable',
      }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Create Project' }),
    ).not.toBeInTheDocument()
  })

  it('requires the exact Project key and archives with its expected version', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectBId}/settings`,
    )
    let archived = false
    const fetchMock = installStandardApi()
    fetchMock.mockImplementation(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) return jsonResponse(authenticatedSession)
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({
            projects: archived ? [projectA] : [projectA, projectB],
          })
        }
        if (path.endsWith(`/projects/${projectBId}`)) {
          return jsonResponse(projectB)
        }
        if (path.endsWith(`/projects/${projectBId}/navigation`)) {
          return jsonResponse({
            ...projectB.navigation,
            location: { kind: 'settings' },
            href: `/projects/${projectBId}/settings`,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/archive`) &&
          method === 'POST'
        ) {
          archived = true
          return jsonResponse({
            ...projectB,
            archivedAt: '2026-07-31T07:00:00Z',
            version: 2,
          })
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    render(<App />)

    const archiveButton = await screen.findByRole('button', {
      name: 'Archive Project',
    })
    expect(archiveButton).toBeDisabled()
    fireEvent.change(screen.getByLabelText(/Type BETA to confirm/), {
      target: { value: 'BETA' },
    })
    fireEvent.click(archiveButton)

    expect(
      await screen.findByText(/Beta Project was archived/),
    ).toBeVisible()
    const archiveCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith(`/${projectBId}/archive`),
    )
    expect(archiveCall?.[1]?.body).toBe(JSON.stringify({ expectedVersion: 1 }))
    expect(new Headers(archiveCall?.[1]?.headers).get('x-csrf-token')).toBe(
      'test-csrf',
    )
    expect(
      screen.queryByRole('link', { name: 'Beta Project' }),
    ).not.toBeInTheDocument()
  })

  it.each([
    {
      code: 'version_conflict',
      message: 'Project changed. Reload before archiving.',
      details: { currentVersion: 2 },
    },
    {
      code: 'project_has_active_runs',
      message: 'Interrupt active runs before archiving.',
    },
  ])('keeps archive conflicts recoverable for $code', async ({
    code,
    details,
    message,
  }) => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectBId}/settings`,
    )
    let projectReads = 0
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/auth/session')) {
          return jsonResponse(authenticatedSession)
        }
        if (path.startsWith('/api/v1/projects?') && method === 'GET') {
          return jsonResponse({ projects: [projectA, projectB] })
        }
        if (path.endsWith(`/projects/${projectBId}`)) {
          projectReads += 1
          return jsonResponse({
            ...projectB,
            version:
              code === 'version_conflict' && projectReads > 1 ? 2 : 1,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/navigation`) &&
          method === 'GET'
        ) {
          return jsonResponse({
            ...projectB.navigation,
            location: { kind: 'settings' },
            href: `/projects/${projectBId}/settings`,
          })
        }
        if (
          path.endsWith(`/projects/${projectBId}/archive`) &&
          method === 'POST'
        ) {
          return jsonResponse({ error: { code, details, message } }, 409)
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    render(<App />)

    fireEvent.change(
      await screen.findByLabelText(/Type BETA to confirm/),
      { target: { value: 'BETA' } },
    )
    fireEvent.click(screen.getByRole('button', { name: 'Archive Project' }))

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent(
      code === 'version_conflict' ? 'Someone changed this Project' : message,
    )
    await waitFor(() => expect(alert).toHaveFocus())
    expect(window.location.pathname).toBe(
      `/projects/${projectBId}/settings`,
    )
    expect(
      screen.getByRole('link', { name: 'Beta Project' }),
    ).toBeVisible()
    const archiveCall = fetchMock.mock.calls.find(([path]) =>
      String(path).endsWith(`/${projectBId}/archive`),
    )
    expect(archiveCall?.[1]?.body).toBe(
      JSON.stringify({ expectedVersion: 1 }),
    )
    if (code === 'version_conflict') {
      fireEvent.click(
        screen.getByRole('button', { name: 'Archive Project' }),
      )
      await waitFor(() =>
        expect(
          fetchMock.mock.calls.filter(([path]) =>
            String(path).endsWith(`/${projectBId}/archive`),
          ),
        ).toHaveLength(2),
      )
      const archiveCalls = fetchMock.mock.calls.filter(([path]) =>
        String(path).endsWith(`/${projectBId}/archive`),
      )
      expect(archiveCalls[1][1]?.body).toBe(
        JSON.stringify({ expectedVersion: 2 }),
      )
    }
  })

  it('serializes a navigation conflict retry with the newest expected version', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000088'
    const projectC = project(projectCId, 'GAMMA', 'Gamma Project')
    window.history.replaceState({}, '', `/projects/${projectCId}/settings`)
    let navigationReads = 0
    const navigationWrites: Array<Record<string, unknown>> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            return jsonResponse({ projects: [projectC] })
          }
          if (path.endsWith(`/projects/${projectCId}`)) {
            return jsonResponse(projectC)
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'GET'
          ) {
            navigationReads += 1
            return jsonResponse({
              ...projectC.navigation,
              version: navigationReads,
            })
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'PUT'
          ) {
            const body = JSON.parse(String(init?.body)) as Record<string, unknown>
            navigationWrites.push(body)
            if (navigationWrites.length === 1) {
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
            return jsonResponse({
              ...projectC.navigation,
              location: { kind: 'settings' },
              href: `/projects/${projectCId}/settings`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { name: 'Project settings' }),
    ).toBeVisible()
    await waitFor(() => expect(navigationWrites).toHaveLength(2))
    expect(navigationWrites.map((write) => write.expectedVersion)).toEqual([
      1, 2,
    ])
    expect(navigationWrites[1]).toMatchObject({
      location: { kind: 'settings' },
    })
    expect(navigationWrites[1]).not.toHaveProperty('actorId')
  })

  it('announces a terminal remembered-location failure and offers retry', async () => {
    const projectCId = '019fb600-0000-7000-8000-000000000089'
    const projectC = project(projectCId, 'DELTA', 'Delta Project')
    window.history.replaceState({}, '', `/projects/${projectCId}/settings`)
    let navigationReads = 0
    let navigationWrites = 0
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            return jsonResponse({ projects: [projectC] })
          }
          if (path.endsWith(`/projects/${projectCId}`)) {
            return jsonResponse(projectC)
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'GET'
          ) {
            navigationReads += 1
            return jsonResponse({
              ...projectC.navigation,
              version: navigationReads === 1 ? 1 : 2,
            })
          }
          if (
            path.endsWith(`/projects/${projectCId}/navigation`) &&
            method === 'PUT'
          ) {
            navigationWrites += 1
            if (navigationWrites === 1) {
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
            if (navigationWrites === 2) {
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
            return jsonResponse({
              ...projectC.navigation,
              location: { kind: 'settings' },
              href: `/projects/${projectCId}/settings`,
              version: 3,
            })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('Navigation could not be saved.')
    expect(navigationWrites).toBe(2)
    fireEvent.click(
      within(alert).getByRole('button', { name: 'Try saving again' }),
    )
    await waitFor(() => expect(navigationWrites).toBe(3))
    await waitFor(() =>
      expect(
        screen.queryByText('Navigation could not be saved.'),
      ).not.toBeInTheDocument(),
    )
  })

  it('keeps Viewer Project controls read-only', async () => {
    const viewerProject = {
      ...projectB,
      role: 'viewer',
      capabilities: {
        view: true,
        manage: false,
        archive: false,
        navigationWrite: true,
      },
      navigation: {
        ...projectB.navigation,
        location: { kind: 'settings' },
        href: `/projects/${projectBId}/settings`,
      },
    }
    window.history.replaceState({}, '', `/projects/${projectBId}/settings`)
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
          const path = String(input)
          const method = init?.method ?? 'GET'
          if (path.endsWith('/auth/session')) {
            return jsonResponse(authenticatedSession)
          }
          if (path.startsWith('/api/v1/projects?') && method === 'GET') {
            return jsonResponse({ projects: [viewerProject] })
          }
          if (path.endsWith(`/projects/${projectBId}`)) {
            return jsonResponse(viewerProject)
          }
          if (path.endsWith(`/projects/${projectBId}/navigation`)) {
            return jsonResponse(viewerProject.navigation)
          }
          if (
            path.endsWith(`/projects/${projectBId}/repositories`) &&
            method === 'GET'
          ) {
            return jsonResponse({ canManage: false, repositories: [] })
          }
          throw new Error(`Unexpected request: ${method} ${path}`)
        },
      ),
    )
    render(<App />)

    expect(
      await screen.findByRole('heading', { name: 'Project settings' }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Archive Project' }),
    ).not.toBeInTheDocument()
    expect(screen.getByText('Viewer')).toBeVisible()
    expect(screen.getByRole('note')).toHaveTextContent('Viewer access is read-only')
    const sections = screen.getByRole('navigation', { name: 'Settings sections' })
    expect(within(sections).queryByRole('link', { name: 'Codex' })).not.toBeInTheDocument()
    expect(within(sections).queryByRole('link', { name: 'GitHub' })).not.toBeInTheDocument()
    expect(within(sections).queryByRole('link', { name: 'Members' })).not.toBeInTheDocument()
    fireEvent.click(within(sections).getByRole('link', { name: 'Repositories' }))
    expect(await screen.findByRole('heading', { name: 'Repositories' })).toBeVisible()
    expect(
      screen.queryByRole('heading', { name: 'Codex account' }),
    ).not.toBeInTheDocument()
  })

  it('preserves the hosted Thread timeline, hostile-output escaping, and approval flow', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const { container } = render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: 'Show details' }))
    expect(await screen.findByRole('heading', { name: 'Details' })).toBeVisible()
    expect(
      screen.getByText('<img src=x onerror=alert(1)>'),
    ).toBeVisible()
    expect(container.querySelector('img')).toBeNull()
    expect(
      screen.getByRole('button', { name: 'Stop run' }),
    ).toBeVisible()
    expect(
      (
        await axe.run(container, {
          rules: { 'color-contrast': { enabled: false } },
        })
      ).violations,
    ).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }))
    await waitFor(() => {
      expect(
        fetchMock.mock.calls.some(([path]) =>
          String(path).includes('/agent-approvals/approval-1/respond'),
        ),
      ).toBe(true)
    })
  })

  it('submits a bounded provider question answer instead of an approval decision', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const questionTimeline = {
      ...timeline,
      entries: timeline.entries.map((entry) =>
        entry.workerEvent?.kind === 'approval_requested'
          ? {
              ...entry,
              workerEvent: {
                ...entry.workerEvent,
                kind: 'question_requested',
                summary: 'The Worker needs a release name.',
              },
            }
          : entry,
      ),
      approvals: [
        {
          ...timeline.approvals[0],
          kind: 'question' as const,
          summary: 'What should the release be called?',
          options: [],
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(questionTimeline)
      }
      if (path.includes('/agent-approvals/approval-1/respond')) {
        return jsonResponse({
          ...questionTimeline.approvals[0],
          status: 'resolved',
        })
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { name: 'Codex has a question' }),
    ).toBeVisible()
    fireEvent.change(screen.getByLabelText('Your answer'), {
      target: { value: 'Aurora' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send answer' }))
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-approvals/approval-1/respond'),
      )
      expect(request).toBeDefined()
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({ answer: 'Aurora' })
    })
    expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull()
  })

  it('retries the latest terminal run with a stable explicit command', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalRun = {
      ...timeline.activeRun,
      state: 'failed',
      failureReason: 'The service restarted; the worktree was preserved.',
      capabilities: { interrupt: false, retry: true },
    }
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: terminalRun,
      approvals: [],
      entries: [
        ...timeline.entries,
        {
          id: 'entry-failed',
          sequence: 3,
          kind: 'worker_event',
          createdAt: '2026-07-31T04:02:00Z',
          message: null,
          workerEvent: {
            id: 'event-failed',
            agentRunId: 'run-1',
            kind: 'run_failed',
            summary: 'The run failed and its worktree was preserved.',
            payload: {},
          },
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (path.includes('/agent-runs/run-1/retry')) {
        return jsonResponse({
          runId: 'run-2',
          state: 'queued',
          repositoryId: 'repo-1',
          repositoryWorkspaceId: null,
          branch: null,
          baseCommit: null,
          queuePosition: 1,
        }, 202)
      }
      if (path.includes('/agent-runs/run-1/acknowledge-failure')) {
        return jsonResponse({})
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Dismiss failure' }),
    )
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-runs/run-1/acknowledge-failure'),
      )
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({
        eventId: 'event-failed',
      })
    })
    expect(
      screen.queryByRole('button', { name: 'Dismiss failure' }),
    ).toBeNull()
    fireEvent.click(
      screen.getByRole('button', { name: 'Run again' }),
    )
    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes('/agent-runs/run-1/retry'),
      )
      expect(new Headers(request?.[1]?.headers).get('idempotency-key')).toBe(
        'retry-run-1',
      )
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })

  it('queues publication for the exact terminal run through the Project-scoped boundary', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        capabilities: { interrupt: false, retry: false, publish: true },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/agent-runs/run-1/publish`,
        )
      ) {
        return jsonResponse({ jobId: 'publish-job-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Publish branch' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/agent-runs/run-1/publish`,
        ),
      )
      expect(request).toBeDefined()
      expect(request?.[1]?.method).toBe('POST')
      expect(
        new Headers(request?.[1]?.headers).get('idempotency-key'),
      ).toMatch(/^[0-9a-f-]{36}$/i)
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })

  it('creates a pull request only from the exact published run and explicit Work Item links', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const terminalTimeline = {
      ...timeline,
      linkedWorkItems: [
        {
          id: '019fbc00-0000-7000-8000-000000000001',
          key: 'AW-42',
          title: 'Ship exact publication',
          statusCategory: 'in_progress',
          archivedAt: null,
          href: `/projects/${projectAId}/work-items/AW-42`,
        },
      ],
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        publishedCommit: '0123456789012345678901234567890123456789',
        capabilities: {
          interrupt: false,
          retry: false,
          publish: false,
          createPullRequest: true,
        },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/agent-runs/run-1/pull-request`,
        )
      ) {
        return jsonResponse({ requestId: 'pull-request-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Create pull request' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/agent-runs/run-1/pull-request`,
        ),
      )
      expect(request).toBeDefined()
      expect(request?.[1]?.method).toBe('POST')
      expect(
        new Headers(request?.[1]?.headers).get('idempotency-key'),
      ).toMatch(/^[0-9a-f-]{36}$/i)
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({
        title: 'AW-42: Ship exact publication',
        body: 'Created from an explicitly published Agent Workspace run.',
        workItemIds: ['019fbc00-0000-7000-8000-000000000001'],
      })
    })
  })

  it('renders canonical GitHub state and queues an explicit pull request refresh', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const pullRequestId = '019fbc00-0000-7000-8000-000000000099'
    const terminalTimeline = {
      ...timeline,
      activeRun: null,
      latestTerminalRun: {
        ...timeline.activeRun,
        state: 'completed',
        pullRequestId,
        pullRequestNumber: 7,
        pullRequestUrl:
          'https://github.com/Mischa-dev/agent-workspace-m0-spike/pull/7',
        pullRequestTitle: 'Canonical review state',
        pullRequestState: 'open',
        pullRequestDraft: false,
        pullRequestChecksState: 'success',
        pullRequestReviewState: 'approved',
        pullRequestMergeabilityState: 'ready',
        pullRequestSyncState: 'fresh',
        pullRequestSyncFailureCategory: null,
        capabilities: {
          interrupt: false,
          retry: false,
          publish: false,
          createPullRequest: false,
          refreshPullRequest: true,
        },
      },
      approvals: [],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(terminalTimeline)
      }
      if (
        path.includes(
          `/projects/${projectAId}/pull-requests/${pullRequestId}/refresh`,
        )
      ) {
        return jsonResponse({ jobId: 'refresh-1', state: 'queued' }, 202)
      }
      return standardImplementation?.(input, init) ?? Promise.reject()
    })
    render(<App />)

    expect(await screen.findByText('Checks passed')).toBeVisible()
    expect(screen.getByText(/review approved · ready/)).toBeVisible()
    fireEvent.click(
      screen.getByRole('button', { name: 'Refresh GitHub status' }),
    )

    await waitFor(() => {
      const request = fetchMock.mock.calls.find(([path]) =>
        String(path).includes(
          `/projects/${projectAId}/pull-requests/${pullRequestId}/refresh`,
        ),
      )
      expect(request?.[1]?.method).toBe('POST')
      expect(JSON.parse(String(request?.[1]?.body))).toEqual({})
    })
  })

  it('does not remember an active child while browsing archived-parent history', async () => {
    const archivedChildId = '019fb600-0000-7000-8000-000000000014'
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${archivedChildId}?includeArchived=true`,
    )
    let navigationWrites = 0
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${archivedChildId}/timeline`)) {
        return jsonResponse({
          ...timeline,
          conversation: {
            ...timeline.conversation,
            id: archivedChildId,
            archivedAt: null,
          },
        })
      }
      if (
        path.endsWith(`/projects/${projectAId}/navigation`) &&
        init?.method === 'PUT'
      ) {
        navigationWrites += 1
        return jsonResponse(projectA.navigation)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([path]) =>
          String(path).includes(`/conversations/${archivedChildId}/timeline`),
        ),
      ).toBe(true),
    )
    expect(navigationWrites).toBe(0)
  })

  it('rechecks an expired session when the realtime connection closes', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    let expired = false
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (
        expired &&
        String(input).includes(`/conversations/${threadId}/timeline`)
      ) {
        return jsonResponse(
          {
            error: {
              code: 'authentication_required',
              message: 'Sign in to continue.',
            },
          },
          401,
        )
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(WebSocketStub.instances).toHaveLength(1)
    expired = true
    WebSocketStub.instances[0].dispatchEvent(new Event('close'))

    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeVisible()
    expect(
      screen.queryByText('<img src=x onerror=alert(1)>'),
    ).not.toBeInTheDocument()
  })

  it('redirects a cross-Project Thread response without exposing its state or actions', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const mismatchedTimeline = {
      ...timeline,
      conversation: {
        ...timeline.conversation,
        projectId: projectBId,
        name: 'Private Project B Thread',
      },
      entries: [
        {
          ...timeline.entries[0],
          message: {
            ...timeline.entries[0].message,
            plainText: 'Project B private content',
          },
        },
      ],
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(mismatchedTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
    expect(screen.queryByText('Private Project B Thread')).not.toBeInTheDocument()
    expect(screen.queryByText('Project B private content')).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Stop run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for Codex')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run with Codex' })).not.toBeInTheDocument()
    // Realtime is authenticated at the browser-session boundary, not scoped
    // to a resource that a mismatched response could cause us to subscribe to.
    expect(WebSocketStub.instances).toHaveLength(1)
    expect(
      fetchMock.mock.calls.some(
        ([path]) =>
          String(path).includes('/agent-runs') ||
          String(path).includes('/agent-approvals'),
      ),
    ).toBe(false)
  })

  it.each([
    {
      label: 'a different Conversation ID',
      conversation: {
        ...timeline.conversation,
        id: '019fb600-0000-7000-8000-00000000000c',
      },
    },
    {
      label: 'a Channel kind at the Thread route',
      conversation: { ...timeline.conversation, kind: 'channel' },
    },
  ])('rejects $label even inside the selected Project', async ({ conversation }) => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse({ ...timeline, conversation })
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
    expect(screen.queryByText('M0 walking skeleton')).not.toBeInTheDocument()
  })

  it('returns a remotely unavailable Thread to its last verified parent Channel', async () => {
    const parentChannelId = '019fb600-0000-7000-8000-00000000000d'
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    let timelineReads = 0
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        timelineReads += 1
        if (timelineReads > 1) {
          return jsonResponse(
            {
              error: {
                code: 'not_found',
                message: 'Conversation not found.',
              },
            },
            404,
          )
        }
        return jsonResponse({
          ...timeline,
          conversation: {
            ...timeline.conversation,
            parentChannelId,
          },
        })
      }
      if (path.includes(`/conversations/${parentChannelId}/timeline`)) {
        return new Promise<Response>(() => {})
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    WebSocketStub.instances[0].dispatchEvent(
      new MessageEvent('message', {
        data: JSON.stringify({
          event_id: '019fb600-0000-7000-8000-00000000000e',
          event_cursor: 14,
          version: 1,
          project_id: projectAId,
          type: 'conversation.archived',
          occurred_at: '2026-07-31T15:02:00Z',
          payload: { conversationId: threadId, version: 2 },
        }),
      }),
    )

    await waitFor(() =>
      expect(window.location.pathname).toBe(
        `/projects/${projectAId}/channels/${parentChannelId}`,
      ),
    )
  })

  it('returns an initially unavailable Thread to the Project overview', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(
          {
            error: {
              code: 'not_found',
              message: 'Conversation not found.',
            },
          },
          404,
        )
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectAId}`),
    )
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Agent Workspace',
      }),
    ).toBeVisible()
  })

  it('keeps Viewer execution controls read-only when capabilities are missing', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const readOnlyTimeline = {
      ...timeline,
      capabilities: {},
      activeRun: timeline.activeRun
        ? { ...timeline.activeRun, capabilities: {} }
        : null,
      approvals: timeline.approvals.map((approval) => ({
        ...approval,
        capabilities: {},
      })),
    }
    const viewerThreadProject = {
      ...projectA,
      role: 'viewer',
      capabilities: {
        view: true,
        manage: false,
        archive: false,
        navigationWrite: false,
      },
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      const path = String(input)
      const method = init?.method ?? 'GET'
      if (path.includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(readOnlyTimeline)
      }
      if (path.startsWith('/api/v1/projects?') && method === 'GET') {
        return jsonResponse({ projects: [viewerThreadProject] })
      }
      if (path.endsWith(`/projects/${projectAId}`)) {
        return jsonResponse(viewerThreadProject)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(screen.getByText('Allow the isolated file change?')).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Deny' }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: 'Stop run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for Codex')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run with Codex' })).not.toBeInTheDocument()
  })

  it('fails Worker invocation closed when its timeline capability is absent', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const idleReadOnlyTimeline = {
      ...timeline,
      activeRun: null,
      approvals: [],
      capabilities: {},
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(idleReadOnlyTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    expect(
      await screen.findByRole('heading', { level: 1, name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(
      screen.queryByRole('button', { name: 'Start run' }),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Describe the change for Codex')).not.toBeInTheDocument()
    expect(screen.queryByRole('radio', { name: 'Run with Codex' })).not.toBeInTheDocument()
  })

  it('shows Worker invocation only when the server explicitly grants it', async () => {
    window.history.replaceState(
      {},
      '',
      `/projects/${projectAId}/threads/${threadId}`,
    )
    const invokableTimeline = {
      ...timeline,
      activeRun: null,
      approvals: [],
      capabilities: { workerInvoke: true },
    }
    const fetchMock = installStandardApi()
    const standardImplementation = fetchMock.getMockImplementation()
    fetchMock.mockImplementation((input, init) => {
      if (String(input).includes(`/conversations/${threadId}/timeline`)) {
        return jsonResponse(invokableTimeline)
      }
      if (!standardImplementation) {
        throw new Error('Standard API implementation unavailable.')
      }
      return standardImplementation(input, init)
    })
    render(<App />)

    // Without message access the thread opens straight into the run composer.
    expect(
      await screen.findByLabelText('Describe the change for Codex'),
    ).toBeVisible()
    expect(screen.queryByRole('radio', { name: 'Run with Codex' })).not.toBeInTheDocument()
    expect(
      screen.getByRole('button', { name: 'Start run' }),
    ).toBeDisabled()
  })
})

describe('streamed worker message coalescing', () => {
  it('keeps one in-memory message while appending deltas', () => {
    const first = coalesceDelta({}, 'message-1', 'hello ')
    const second = coalesceDelta(first, 'message-1', 'world')
    expect(second).toEqual({ 'message-1': 'hello world' })
  })

  it('coalesces a frame of deltas into one state update', () => {
    expect(
      coalesceDeltaBatch(
        { 'message-1': 'hello ' },
        { 'message-1': 'world', 'message-2': 'second' },
      ),
    ).toEqual({
      'message-1': 'hello world',
      'message-2': 'second',
    })
  })

  it('keeps the consolidated provider stream inside the Message boundary', () => {
    const exact = { 'message-1': '🙂'.repeat(20_000) }
    expect(coalesceDelta(exact, 'message-1', '🙂')).toBe(exact)
    expect(coalesceDelta(exact, 'message-1', '\u0000')).toBe(exact)
    expect(
      coalesceDeltaBatch(exact, { 'message-1': '🙂', 'message-2': 'safe' }),
    ).toEqual({ ...exact, 'message-2': 'safe' })
  })
})
