import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { App } from './App'
import { coalesceDelta } from './streaming'

const bootstrap = {
  projectId: '019fb600-0000-7000-8000-000000000001',
  channelId: '019fb600-0000-7000-8000-000000000002',
  threadId: '019fb600-0000-7000-8000-000000000003',
}

const timeline = {
  conversation: {
    id: bootstrap.threadId,
    projectId: bootstrap.projectId,
    kind: 'thread',
    parentChannelId: bootstrap.channelId,
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
        content: { version: 1 },
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
    },
  ],
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
      headers: { 'content-type': 'application/json' },
    }),
  )
}

describe('M0 Thread route', () => {
  beforeEach(() => {
    WebSocketStub.instances = []
    vi.stubGlobal('WebSocket', WebSocketStub)
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input)
        if (path.endsWith('/api/v1/dev/bootstrap')) return jsonResponse(bootstrap)
        if (path.includes('/timeline')) return jsonResponse(timeline)
        if (path.includes('/respond')) {
          return jsonResponse({ ...timeline.approvals[0], status: 'resolved' })
        }
        return jsonResponse({})
      }),
    )
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('renders the four-region shell and durable timeline', async () => {
    render(<App />)

    expect(screen.getByRole('navigation', { name: 'Projects' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Agent Workspace' })).toBeVisible()
    expect(
      await screen.findByRole('heading', { name: 'M0 walking skeleton' }),
    ).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Project host' })).toBeVisible()
    expect(screen.getByText('The run is waiting for an approval.')).toBeVisible()
  })

  it('escapes hostile provider output as text', async () => {
    const { container } = render(<App />)

    expect(await screen.findByText('<img src=x onerror=alert(1)>')).toBeVisible()
    expect(container.querySelector('img')).toBeNull()
  })

  it('submits an approval response and refetches durable state', async () => {
    render(<App />)
    const button = await screen.findByRole('button', { name: 'Allow once' })
    fireEvent.click(button)

    await waitFor(() => {
      expect(fetch).toHaveBeenCalledWith(
        '/api/v1/agent-approvals/approval-1/respond',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({ decision: 'approve' }),
        }),
      )
    })
  })

  it('refetches after the realtime connection opens', async () => {
    render(<App />)
    await screen.findByText('<img src=x onerror=alert(1)>')
    expect(WebSocketStub.instances).toHaveLength(1)

    WebSocketStub.instances[0].dispatchEvent(new Event('open'))
    await waitFor(() => {
      const fetchMock = vi.mocked(fetch)
      expect(
        fetchMock.mock.calls.filter(([path]) => String(path).includes('/timeline')),
      ).toHaveLength(2)
    })
  })
})

describe('streamed worker message coalescing', () => {
  it('keeps one in-memory message while appending deltas', () => {
    const first = coalesceDelta({}, 'message-1', 'hello ')
    const second = coalesceDelta(first, 'message-1', 'world')

    expect(second).toEqual({ 'message-1': 'hello world' })
  })
})
