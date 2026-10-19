import { cleanup } from '@testing-library/react'
import { afterEach, beforeEach, vi } from 'vitest'

import type {
  OrganizationSummary,
  SessionResponse,
} from '../auth/session'

export const organization: OrganizationSummary = {
  id: '019fb600-0000-7000-8000-000000000000',
  name: 'Mischa',
  slug: 'mischa',
  role: 'owner',
}
export const projectAId = '019fb600-0000-7000-8000-000000000001'
export const projectBId = '019fb600-0000-7000-8000-000000000002'
export const threadId = '019fb600-0000-7000-8000-000000000003'
export const adminOrganizationId = '019fb600-0000-7000-8000-000000000007'
export const ownerOrganizationId = '019fb600-0000-7000-8000-000000000008'
export const memberOrganizationId = '019fb600-0000-7000-8000-000000000009'
export const user = {
  id: '019fb600-0000-7000-8000-000000000004',
  actorId: '019fb600-0000-7000-8000-000000000005',
  actorKind: 'human',
  loginName: 'mischa',
  displayName: 'Mischa Nelson',
  capabilities: { createProject: true },
}
export const authenticatedSession = {
  authenticated: true,
  user,
  organizations: [organization],
}
export const signedOutSession = {
  authenticated: false,
  user: null,
  organizations: [],
}

export function project(
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

export const projectA = project(projectAId, 'AW', 'Agent Workspace', {
  attentionCount: 3,
  navigation: {
    location: { kind: 'thread', conversationId: threadId },
    href: `/projects/${projectAId}/threads/${threadId}`,
    pinned: true,
    lastOpenedAt: '2026-07-31T06:00:00Z',
    version: 2,
  },
})
export const projectB = project(projectBId, 'BETA', 'Beta Project')

export const timeline = {
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
    config: { harness: 'codex' },
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

export class WebSocketStub extends EventTarget {
  static instances: WebSocketStub[] = []
  close = vi.fn()

  constructor(public readonly url: string) {
    super()
    WebSocketStub.instances.push(this)
  }
}

export function jsonResponse(body: unknown, status = 200) {
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

export function noContentResponse() {
  return Promise.resolve(new Response(null, { status: 204 }))
}

export function installStandardApi(
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
      // Home's sections, quiet by default.
      if (path === '/api/v1/home') {
        return jsonResponse({ working: [], workingTotal: 0, queuedLaunches: 0 })
      }
      if (path.startsWith('/api/v1/inbox?')) {
        return jsonResponse({ notifications: [], nextCursor: null, unreadCount: 0 })
      }
      if (path === '/api/v1/inbox/summary') return jsonResponse({ unreadCount: 0 })
      if (path === '/api/v1/auth/workos') {
        return jsonResponse({ enabled: false, signupEnabled: false, googleEnabled: false, linked: false, email: null })
      }
      if (path === '/api/v1/usage') return jsonResponse({ accounts: [] })
      if (path === '/api/v1/my-work') {
        const empty = { items: [], total: 0 }
        return jsonResponse({ assigned: empty, claimed: empty, review: empty })
      }
      if (path.endsWith('/onboarding') && method === 'GET') {
        return jsonResponse({
          organizationId: organization.id,
          organizationName: organization.name,
          projectId: projectAId,
          projectName: 'Project A',
          steps: [],
          nextStep: null,
          complete: true,
          dismissed: false,
          canManageWorkspace: true,
        })
      }
      throw new Error(`Unexpected request: ${method} ${path}`)
    },
  )
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

/** Registers the shared App shell beforeEach/afterEach hooks inside the calling describe block. */
export function installAppShellLifecycle() {
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
}
