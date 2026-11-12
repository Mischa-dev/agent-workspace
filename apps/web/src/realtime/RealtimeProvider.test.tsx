import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider, QueryObserver } from '@tanstack/react-query'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RealtimeProvider } from './RealtimeProvider'
import { useConversationRealtime } from './realtimeContext'

const projectId = '019fb600-0000-7000-8000-000000000001'
const conversationId = '019fb600-0000-7000-8000-000000000002'
const eventId = '019fb600-0000-7000-8000-000000000003'
const messageStreamId = '019fb600-0000-7000-8000-000000000006'
const runId = '019fb600-0000-7000-8000-000000000008'
const secondMessageStreamId = '019fb600-0000-7000-8000-00000000000f'
const secondRunId = '019fb600-0000-7000-8000-000000000010'

class WebSocketStub extends EventTarget {
  static instances: WebSocketStub[] = []
  close = vi.fn()

  constructor(public readonly url: string) {
    super()
    WebSocketStub.instances.push(this)
  }

  message(value: unknown) {
    this.dispatchEvent(
      new MessageEvent('message', { data: JSON.stringify(value) }),
    )
  }
}

function Probe() {
  const realtime = useConversationRealtime(conversationId)
  return (
    <output>
      {realtime.connection}:{Object.values(realtime.streaming).join('|')}
    </output>
  )
}

describe('authenticated realtime provider', () => {
  beforeEach(() => {
    WebSocketStub.instances = []
    vi.stubGlobal('WebSocket', WebSocketStub)
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it('opens one session socket and ignores duplicate or future durable events', async () => {
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const boardKey = ['project', projectId, 'threads', conversationId]
    queryClient.setQueryData(boardKey, [])
    const cardKey = ['things', 'thread', conversationId]
    queryClient.setQueryData(cardKey, {})
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    expect(WebSocketStub.instances).toHaveLength(1)
    const socket = WebSocketStub.instances[0]
    const event = {
      eventId,
      eventCursor: 1,
      version: 1,
      projectId,
      type: 'conversation.message.created',
      occurredAt: '2026-07-31T15:00:00Z',
      payload: { conversationId },
    }
    socket.message({ ...event, version: 2 })
    socket.message(event)
    socket.message(event)

    await waitFor(() => expect(invalidate).toHaveBeenCalledTimes(5))
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ['projects', 'rail'],
    })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['direct'] })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['inbox'] })
    // Thread boards are project queries, so the project refresh reaches them.
    expect(queryClient.getQueryState(boardKey)?.isInvalidated).toBe(true)
    // So is the conversation's card where a page embeds it.
    expect(queryClient.getQueryState(cardKey)?.isInvalidated).toBe(true)
  })

  it('blanks a removed message in cached timelines and pins before refetching', async () => {
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const removedId = '019fb600-0000-7000-8000-000000000020'
    const timelineKey = ['project', projectId, 'conversation', conversationId, 'timeline', 'active']
    queryClient.setQueryData(timelineKey, {
      entries: [
        {
          id: '019fb600-0000-7000-8000-000000000021',
          sequence: 1,
          kind: 'message',
          createdAt: '2026-10-01T10:00:00Z',
          message: {
            id: removedId,
            author: { id: eventId, kind: 'human', displayName: 'Bob', handle: 'bob', avatarUrl: null },
            content: { version: 1, nodes: [{ type: 'text', text: 'ghp_secret' }] },
            plainText: 'ghp_secret',
            mentionedActors: [],
            referencedWorkItems: [],
            createdAt: '2026-10-01T10:00:00Z',
            via: null,
            revision: 1,
            editedAt: null,
            removed: null,
          },
          workerEvent: null,
        },
      ],
    })
    queryClient.setQueryData(['project', projectId, 'memory', 'pins', conversationId], {
      pins: [{ kind: 'message', id: removedId, title: 'Bob', text: 'ghp_secret' }],
    })
    // A thread named from the message's first line, on a board that's open.
    const threadsKey = ['project', projectId, 'threads', conversationId]
    queryClient.setQueryData(threadsKey, [
      { id: runId, name: 'ghp_secret', status: 'Open', originMessageId: removedId, messageCount: 1 },
    ])
    const unsubscribe = new QueryObserver(queryClient, {
      queryKey: threadsKey,
      // The refetch never answers here, so what's shown is the synchronous change.
      queryFn: () => new Promise<unknown>(() => undefined),
      staleTime: Infinity,
    }).subscribe(() => undefined)
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    WebSocketStub.instances[0].message({
      eventId,
      eventCursor: 4,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.message.updated',
      occurredAt: '2026-10-01T10:01:00Z',
      payload: { conversationId, messageId: removedId, revision: 1, removed: true },
    })

    // Synchronous: no refetch has run yet.
    expect(JSON.stringify(queryClient.getQueryData(timelineKey))).not.toContain('ghp_secret')
    // Pins on no screen are forgotten outright; shown ones lose the message.
    expect(
      JSON.stringify(queryClient.getQueryData(['project', projectId, 'memory', 'pins', conversationId]) ?? null),
    ).not.toContain('ghp_secret')
    // The open board shows the neutral name at once and refetches the real one.
    expect(JSON.stringify(queryClient.getQueryData(threadsKey))).not.toContain('ghp_secret')
    expect(queryClient.getQueryState(threadsKey)?.isInvalidated).toBe(true)
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ['direct'] }))
    unsubscribe()
  })

  it('coalesces bounded legacy provider deltas without creating another socket', async () => {
    const queryClient = new QueryClient()
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    const socket = WebSocketStub.instances[0]
    socket.message({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: messageStreamId, agent_run_id: runId, delta: 'hel' },
    })
    socket.message({
      event_id: '019fb600-0000-7000-8000-000000000004',
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: messageStreamId, agent_run_id: runId, delta: 'lo' },
    })
    expect(await screen.findByText('connecting:hello')).toBeVisible()
    expect(WebSocketStub.instances).toHaveLength(1)
  })

  it('converges an ephemeral delta to its durable outbox Message', async () => {
    const queryClient = new QueryClient()
    let frame: FrameRequestCallback | undefined
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      frame = callback
      return 17
    })
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    const socket = WebSocketStub.instances[0]
    socket.message({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: {
        message_id: messageStreamId,
        agent_run_id: runId,
        delta: 'ephemeral',
      },
    })
    socket.message({
      event_id: '019fb600-0000-7000-8000-000000000005',
      event_cursor: 2,
      version: 1,
      project_id: projectId,
      type: 'conversation.message.created',
      occurred_at: '2026-07-31T15:00:01Z',
      payload: {
        conversationId,
        messageId: messageStreamId,
      },
    })
    frame?.(0)

    await waitFor(() => expect(screen.getByText('connecting:')).toBeVisible())
    expect(screen.queryByText(/ephemeral/)).not.toBeInTheDocument()
  })

  it('keeps a Worker stream through unrelated durable invalidations', async () => {
    const queryClient = new QueryClient()
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    const socket = WebSocketStub.instances[0]
    socket.message({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: messageStreamId, agent_run_id: runId, delta: 'hel' },
    })
    expect(await screen.findByText('connecting:hel')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-000000000007',
      eventCursor: 7,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.agent_run.updated',
      occurredAt: '2026-07-31T15:00:02Z',
      payload: {
        conversationId,
        agentRunId: runId,
        state: 'working',
      },
    })
    socket.message({
      event_id: '019fb600-0000-7000-8000-000000000009',
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: messageStreamId, agent_run_id: runId, delta: 'lo' },
    })
    expect(await screen.findByText('connecting:hello')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-00000000000a',
      eventCursor: 8,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.message.created',
      occurredAt: '2026-07-31T15:00:03Z',
      payload: {
        conversationId,
        messageId: '019fb600-0000-7000-8000-00000000000b',
      },
    })
    expect(screen.getByText('connecting:hello')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-00000000000c',
      eventCursor: 9,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.message.created',
      occurredAt: '2026-07-31T15:00:04Z',
      payload: { conversationId, messageId: messageStreamId },
    })
    await waitFor(() => expect(screen.getByText('connecting:')).toBeVisible())
  })

  it('clears a partial Worker stream when its run terminates without a Message', async () => {
    const queryClient = new QueryClient()
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    const socket = WebSocketStub.instances[0]
    socket.message({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: {
        message_id: messageStreamId,
        agent_run_id: runId,
        delta: 'partial',
      },
    })
    expect(await screen.findByText('connecting:partial')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-00000000000d',
      eventCursor: 10,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.agent_run.updated',
      occurredAt: '2026-07-31T15:00:05Z',
      payload: {
        conversationId,
        agentRunId: runId,
        state: 'interrupted',
      },
    })
    await waitFor(() => expect(screen.getByText('connecting:')).toBeVisible())
  })

  it('does not let a delayed terminal event clear a newer run stream', async () => {
    const queryClient = new QueryClient()
    render(
      <QueryClientProvider client={queryClient}>
        <RealtimeProvider>
          <Probe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    const socket = WebSocketStub.instances[0]
    socket.message({
      event_id: eventId,
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: { message_id: messageStreamId, agent_run_id: runId, delta: 'old' },
    })
    socket.message({
      event_id: '019fb600-0000-7000-8000-000000000011',
      project_id: projectId,
      conversation_id: conversationId,
      kind: 'message.delta',
      payload: {
        message_id: secondMessageStreamId,
        agent_run_id: secondRunId,
        delta: 'new',
      },
    })
    expect(await screen.findByText('connecting:old|new')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-000000000012',
      eventCursor: 11,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.agent_run.updated',
      occurredAt: '2026-07-31T15:00:06Z',
      payload: { conversationId, agentRunId: runId, state: 'failed' },
    })
    expect(await screen.findByText('connecting:new')).toBeVisible()

    socket.message({
      eventId: '019fb600-0000-7000-8000-000000000013',
      eventCursor: 12,
      version: 1,
      projectId,
      conversationId,
      type: 'conversation.agent_run.updated',
      occurredAt: '2026-07-31T15:00:07Z',
      payload: { conversationId, agentRunId: secondRunId, state: 'canceled' },
    })
    await waitFor(() => expect(screen.getByText('connecting:')).toBeVisible())
  })
})
