import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { App } from '../app/App'

const projectId = '019fb600-0000-7000-8000-000000000001'
const channelId = '019fb600-0000-7000-8000-000000000002'
const humanId = '019fb600-0000-7000-8000-000000000003'
const userId = '019fb600-0000-7000-8000-000000000004'
const organizationId = '019fb600-0000-7000-8000-000000000005'
const linkedThreadId = '019fb600-0000-7000-8000-000000000006'
const createdThreadId = '019fb600-0000-7000-8000-000000000007'

class WebSocketStub extends EventTarget {
  static instances: WebSocketStub[] = []
  close = vi.fn()

  constructor() {
    super()
    WebSocketStub.instances.push(this)
  }

  message(value: unknown) {
    this.dispatchEvent(
      new MessageEvent('message', { data: JSON.stringify(value) }),
    )
  }
}

function response(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: {
        'content-type': 'application/json',
        'x-request-id': 'channel-test',
      },
    }),
  )
}

function message(sequence: number, text: string) {
  return {
    id: `019fb600-0000-7000-8000-${String(sequence).padStart(12, '0')}`,
    sequence,
    kind: 'message',
    createdAt: '2026-07-31T15:00:00Z',
    message: {
      id: `019fb601-0000-7000-8000-${String(sequence).padStart(12, '0')}`,
      author: {
        id: humanId,
        kind: 'human',
        displayName: 'Mischa <script>',
        handle: 'mischa',
        avatarUrl: null,
      },
      content: {
        version: 1,
        nodes: [
          { type: 'text', text },
          { type: 'mention', actorId: humanId },
        ],
      },
      plainText: `${text}@mischa`,
      mentionedActors: [
        {
          id: humanId,
          kind: 'human',
          displayName: 'Mischa <script>',
          handle: 'mischa',
          avatarUrl: null,
        },
      ],
      createdAt: '2026-07-31T15:00:00Z',
    },
    workerEvent: null,
  }
}

function messageId(sequence: number) {
  return `019fb601-0000-7000-8000-${String(sequence).padStart(12, '0')}`
}

describe('M2 Channel workspace', () => {
  beforeEach(() => {
    WebSocketStub.instances = []
    vi.stubGlobal('WebSocket', WebSocketStub)
    window.history.replaceState(
      {},
      '',
      `/projects/${projectId}/channels/${channelId}`,
    )
    document.cookie = 'agent_workspace_csrf=test-csrf; Path=/; SameSite=Strict'
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it('paginates safe AST output, marks rendered state seen, and retries one idempotent send', async () => {
    let sendAttempts = 0
    const sentKeys: string[] = []
    const threadCommands: unknown[] = []
    const fetchMock = vi.fn(
      (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const path = String(input)
        const method = init?.method ?? 'GET'
        if (path.endsWith('/api/v1/auth/session')) {
          return response({
            authenticated: true,
            user: {
              id: userId,
              actorId: humanId,
              actorKind: 'human',
              loginName: 'mischa',
              displayName: 'Mischa',
              capabilities: { createProject: true },
            },
            organizations: [
              {
                id: organizationId,
                name: 'Mischa',
                slug: 'mischa',
                role: 'owner',
              },
            ],
          })
        }
        const project = {
          id: projectId,
          organizationId,
          key: 'AW',
          name: 'Agent Workspace',
          archivedAt: null,
          role: 'owner',
          version: 1,
          attentionCount: 1,
          capabilities: {
            view: true,
            manage: true,
            archive: true,
            navigationWrite: true,
          },
          navigation: {
            location: { kind: 'channel', conversationId: channelId },
            href: `/projects/${projectId}/channels/${channelId}`,
            pinned: false,
            lastOpenedAt: '2026-07-31T15:00:00Z',
            version: 1,
          },
        }
        if (path.startsWith('/api/v1/projects?')) {
          return response({ projects: [project] })
        }
        if (path.endsWith(`/projects/${projectId}`)) return response(project)
        if (path.endsWith(`/projects/${projectId}/navigation`)) {
          return response({
            ...project.navigation,
            version: project.navigation.version + 1,
          })
        }
        if (path.includes('/conversation-sidebar')) {
          return response({
            projectId,
            channels: [
              {
                id: channelId,
                name: 'General',
                archivedAt: null,
                highestSequence: 3,
                highestSeenSequence: 0,
                unread: true,
                threads: [
                  {
                    id: linkedThreadId,
                    parentChannelId: channelId,
                    originMessageId: messageId(2),
                    name: 'Focused reply',
                    archivedAt: null,
                    parentArchivedAt: null,
                    highestSequence: 1,
                    highestSeenSequence: 1,
                    attention: { kind: 'settled', label: 'Seen' },
                  },
                ],
                hasMoreThreads: false,
              },
            ],
            capabilities: { createChannel: true },
          })
        }
        if (path.includes(`/conversations/${channelId}/timeline`)) {
          const older = path.includes('cursor=opaque-older')
          return response({
            conversation: {
              id: channelId,
              projectId,
              kind: 'channel',
              parentChannelId: null,
              originMessageId: null,
              name: 'General',
              archivedAt: null,
              version: 1,
            },
            entries: older
              ? [message(1, 'Older <script>alert(1)</script> ')]
              : [
                  message(2, 'Hello '),
                  message(3, 'Latest '),
                  {
                    id: '019fb600-0000-7000-8000-000000000099',
                    sequence: 4,
                    kind: 'system',
                    createdAt: '2026-07-31T15:01:00Z',
                    message: null,
                    workerEvent: null,
                  },
                ],
            nextCursor: older ? null : 'opaque-older',
            highestSequence: 4,
            readState: { highestSeenSequence: 0, version: 0 },
            capabilities: {
              view: true,
              sendMessage: true,
              archive: true,
              markSeen: true,
            },
          })
        }
        if (path.endsWith(`/conversations/${channelId}/mark-seen`)) {
          return response({ highestSeenSequence: 4, version: 1 })
        }
        if (path.includes(`/channels/${channelId}/messages`)) {
          sentKeys.push(new Headers(init?.headers).get('idempotency-key') ?? '')
          sendAttempts += 1
          return sendAttempts === 1
            ? response(
                {
                  error: {
                    code: 'temporarily_unavailable',
                    message: 'Try the same Message again.',
                  },
                },
                503,
              )
            : response({ message: message(4, 'Retry me').message }, 201)
        }
        if (path.endsWith(`/channels/${channelId}/threads`)) {
          threadCommands.push(JSON.parse(String(init?.body)))
          return response(
            {
              conversation: {
                id: createdThreadId,
                name: 'Latest follow-up',
              },
            },
            201,
          )
        }
        if (path.includes(`/conversations/${createdThreadId}/timeline`)) {
          return response({
            conversation: {
              id: createdThreadId,
              projectId,
              kind: 'thread',
              parentChannelId: channelId,
              originMessageId: messageId(3),
              name: 'Latest follow-up',
              archivedAt: null,
              version: 1,
            },
            entries: [],
            nextCursor: null,
            highestSequence: 0,
            readState: { highestSeenSequence: 0, version: 0 },
            activeRun: null,
            approvals: [],
            capabilities: {
              view: true,
              sendMessage: true,
              archive: true,
              markSeen: true,
              workerInvoke: false,
            },
          })
        }
        throw new Error(`Unexpected request: ${method} ${path}`)
      },
    )
    vi.stubGlobal('fetch', fetchMock)
    const { container } = render(<App />)

    expect(await screen.findByRole('heading', { name: 'General' })).toBeVisible()
    expect(screen.getByText('Latest')).toBeVisible()
    expect(screen.getByText('System activity')).toBeVisible()
    expect(
      screen.getByText('A system change was recorded in this conversation.'),
    ).toBeVisible()
    expect(screen.getAllByLabelText('Mischa <script>, @mischa')).toHaveLength(2)
    expect(container.querySelector('script')).toBeNull()
    expect(
      screen.getByRole('link', {
        name: 'Open linked Thread: Focused reply',
      }),
    ).toHaveAttribute(
      'href',
      `/projects/${projectId}/threads/${linkedThreadId}`,
    )
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([path, init]) =>
          String(path).endsWith(`/conversations/${channelId}/mark-seen`) &&
          init?.method === 'POST',
        ),
      ).toBe(true),
    )

    fireEvent.click(screen.getByRole('button', { name: 'Load older Messages' }))
    expect(
      await screen.findByText('Older <script>alert(1)</script>'),
    ).toBeVisible()
    expect(container.querySelector('script')).toBeNull()

    fireEvent.change(screen.getByLabelText('Message'), {
      target: { value: 'Retry me' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Try the same Message again.',
    )
    expect(screen.getByLabelText('Message')).toHaveValue('Retry me')
    fireEvent.click(screen.getByRole('button', { name: 'Send Message' }))
    await waitFor(() => expect(sendAttempts).toBe(2))
    expect(sentKeys[0]).toBeTruthy()
    expect(sentKeys[1]).toBe(sentKeys[0])

    const latestMessage = screen.getByText('Latest').closest('article')
    expect(latestMessage).not.toBeNull()
    const startLinkedThread = within(
      latestMessage as HTMLElement,
    ).getByText('Start a linked Thread')
    fireEvent.click(startLinkedThread)
    const linkedThreadForm = startLinkedThread.closest('details')
    expect(linkedThreadForm).not.toBeNull()
    fireEvent.change(
      within(linkedThreadForm as HTMLElement).getByLabelText('Thread title'),
      { target: { value: 'Latest follow-up' } },
    )
    fireEvent.click(
      within(linkedThreadForm as HTMLElement).getByRole('button', {
        name: 'Create Thread',
      }),
    )
    await waitFor(() => expect(threadCommands).toHaveLength(1))
    expect(threadCommands[0]).toEqual({
      name: 'Latest follow-up',
      originMessageId: messageId(3),
    })
    await waitFor(() =>
      expect(window.location.pathname).toBe(
        `/projects/${projectId}/threads/${createdThreadId}`,
      ),
    )
  })

  it('falls back to the Project overview when a durable refetch returns 404', async () => {
    let timelineReads = 0
    const project = {
      id: projectId,
      organizationId,
      key: 'AW',
      name: 'Agent Workspace',
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
        location: { kind: 'channel', conversationId: channelId },
        href: `/projects/${projectId}/channels/${channelId}`,
        pinned: false,
        lastOpenedAt: '2026-07-31T15:00:00Z',
        version: 1,
      },
    }
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input)
        if (path.endsWith('/api/v1/auth/session')) {
          return response({
            authenticated: true,
            user: {
              id: userId,
              actorId: humanId,
              actorKind: 'human',
              loginName: 'mischa',
              displayName: 'Mischa',
              capabilities: { createProject: true },
            },
            organizations: [
              {
                id: organizationId,
                name: 'Mischa',
                slug: 'mischa',
                role: 'owner',
              },
            ],
          })
        }
        if (path.startsWith('/api/v1/projects?')) {
          return response({ projects: [project] })
        }
        if (path.endsWith(`/projects/${projectId}`)) return response(project)
        if (path.endsWith(`/projects/${projectId}/navigation`)) {
          return response(project.navigation)
        }
        if (path.includes('/conversation-sidebar')) {
          return response({
            projectId,
            channels: [],
            capabilities: { createChannel: false },
          })
        }
        if (path.includes(`/conversations/${channelId}/timeline`)) {
          timelineReads += 1
          if (timelineReads > 1) {
            return response(
              {
                error: {
                  code: 'not_found',
                  message: 'Conversation not found.',
                },
              },
              404,
            )
          }
          return response({
            conversation: {
              id: channelId,
              projectId,
              kind: 'channel',
              parentChannelId: null,
              originMessageId: null,
              name: 'General',
              archivedAt: null,
              version: 1,
            },
            entries: [message(1, 'Before archive')],
            nextCursor: null,
            highestSequence: 1,
            readState: { highestSeenSequence: 1, version: 1 },
            capabilities: {
              view: true,
              sendMessage: true,
              archive: true,
              markSeen: true,
            },
          })
        }
        throw new Error(`Unexpected request: ${init?.method ?? 'GET'} ${path}`)
      }),
    )
    render(<App />)

    expect(await screen.findByRole('heading', { name: 'General' })).toBeVisible()
    WebSocketStub.instances[0].message({
      event_id: '019fb600-0000-7000-8000-000000000008',
      event_cursor: 8,
      version: 1,
      project_id: projectId,
      type: 'conversation.archived',
      occurred_at: '2026-07-31T15:01:00Z',
      payload: { conversationId: channelId, version: 2 },
    })

    await waitFor(() =>
      expect(window.location.pathname).toBe(`/projects/${projectId}`),
    )
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Agent Workspace' }),
    ).toBeVisible()
  })
})
