import { act, cleanup, render } from '@testing-library/react'
import { QueryClient, QueryClientProvider, QueryObserver } from '@tanstack/react-query'
import { useEffect } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RealtimeProvider } from '../RealtimeProvider'
import { useLiveControls, useTyping, useWatchDocument, type DocumentFrame, type LiveControls } from '../realtimeContext'

// W1.6: what the page sends over protocol 2 (hello, counted watches,
// throttled typing) and the ephemeral frames it keeps (typing, documents,
// access).

class StubSocket extends EventTarget {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSING = 2
  static readonly CLOSED = 3
  static all: StubSocket[] = []
  readyState = StubSocket.CONNECTING
  sent: Array<Record<string, unknown>> = []
  constructor(public readonly url: string) {
    super()
    StubSocket.all.push(this)
  }
  open() {
    this.readyState = StubSocket.OPEN
    this.dispatchEvent(new Event('open'))
  }
  send(data: string) {
    this.sent.push(JSON.parse(data) as Record<string, unknown>)
  }
  close() {
    if (this.readyState === StubSocket.CLOSED) return
    this.readyState = StubSocket.CLOSED
    this.dispatchEvent(new Event('close'))
  }
  receive(frame: unknown) {
    this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(frame) }))
  }
}

const CONVERSATION = '0193a0aa-0000-7000-8000-000000000003'
const ACCOUNT = '0192a0aa-0000-7000-8000-000000000005'
const SPACE = '0193a0aa-0000-7000-8000-000000000009'
const PAGE = { kind: 'page', id: '0193a1f0-0000-7000-8000-000000000007' } as const
const latest = () => StubSocket.all[StubSocket.all.length - 1]
const seen: { controls: LiveControls | null; typing: Record<string, number> } = { controls: null, typing: {} }

function Probe() {
  const controls = useLiveControls()
  const typing = useTyping(CONVERSATION)
  useEffect(() => {
    seen.controls = controls
    seen.typing = typing
  })
  return null
}
const controls = () => seen.controls as LiveControls

function start() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <RealtimeProvider>
        <Probe />
      </RealtimeProvider>
    </QueryClientProvider>,
  )
  act(() => latest().open())
  return queryClient
}

describe('the live connection on protocol 2', () => {
  beforeEach(() => {
    StubSocket.all = []
    vi.stubGlobal('WebSocket', StubSocket)
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => undefined)))
  })
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('asks for protocol 2 and says hello first', () => {
    start()
    expect(latest().url).toMatch(/\/api\/v1\/realtime\?v=2$/)
    expect(latest().sent[0]).toEqual({ type: 'hello', protocol: 2, client: 'web' })
  })

  it('refreshes what is shown once a fresh start’s ready says where the server began', () => {
    const queryClient = start()
    const key = ['project', SPACE, 'work-items', 'detail', 'task-1']
    // Read on open, before the server fixed its starting position: a change
    // between the two would be missed, so the refresh waits for `ready`.
    queryClient.setQueryData(key, { title: 'Plan' })
    expect(queryClient.getQueryState(key)?.isInvalidated).toBe(false)
    act(() => latest().receive({ type: 'ready', protocol: 2, position: '7', resumed: false, replayed: 0 }))
    expect(queryClient.getQueryState(key)?.isInvalidated).toBe(true)
  })

  it('watches a document once however many views show it, and again after a reconnect', () => {
    start()
    const watched = () => latest().sent.filter((frame) => frame.type === 'watch' || frame.type === 'unwatch')
    let stopFirst = () => undefined as void
    let stopSecond = () => undefined as void
    act(() => {
      stopFirst = controls().watch(PAGE)
      stopSecond = controls().watch(PAGE)
    })
    expect(watched()).toEqual([{ type: 'watch', document: PAGE }])
    act(() => {
      latest().receive({ type: 'ready', protocol: 2, position: '7', resumed: false, replayed: 0 })
      latest().close()
      window.dispatchEvent(new Event('online'))
      latest().open()
    })
    // Hello resumes from the last position, then the watch again.
    expect(latest().sent.slice(0, 2)).toEqual([
      { type: 'hello', protocol: 2, resumeFrom: '7', client: 'web' },
      { type: 'watch', document: PAGE },
    ])
    act(() => stopFirst())
    expect(watched()).toEqual([{ type: 'watch', document: PAGE }])
    act(() => stopSecond())
    expect(watched()).toEqual([{ type: 'watch', document: PAGE }, { type: 'unwatch', document: PAGE }])
  })

  it('watches a document whose page mounts with the connection', () => {
    function PageProbe() {
      useWatchDocument(PAGE)
      return null
    }
    render(
      <QueryClientProvider client={new QueryClient()}>
        <RealtimeProvider>
          <PageProbe />
        </RealtimeProvider>
      </QueryClientProvider>,
    )
    act(() => latest().open())
    expect(latest().sent.filter((frame) => frame.type === 'watch')).toEqual([{ type: 'watch', document: PAGE }])
  })

  it('passes a watched document’s changes and cursors to its listener', () => {
    start()
    const frames: DocumentFrame[] = []
    act(() => void controls().watch(PAGE, (frame) => frames.push(frame)))
    act(() => {
      latest().receive({ type: 'ephemeral', kind: 'document.update', document: PAGE, generation: '0193a0aa-0000-7000-8000-00000000000b', version: 42, update: 'AQLy' })
      latest().receive({ type: 'ephemeral', kind: 'awareness', document: PAGE, connection: '0193a0aa-0000-7000-8000-00000000000a', accountId: ACCOUNT, state: { anchor: 1 } })
      latest().receive({ type: 'ephemeral', kind: 'document.update', document: { kind: 'sheet', id: PAGE.id }, generation: '0193a0aa-0000-7000-8000-00000000000b', version: 1, update: 'AA' })
    })
    expect(frames.map((frame) => frame.type)).toEqual(['document.update', 'awareness'])
  })

  it('says someone is typing at most every 3 seconds', () => {
    start()
    act(() => {
      controls().typing(CONVERSATION)
      controls().typing(CONVERSATION)
    })
    expect(latest().sent.filter((frame) => frame.type === 'typing')).toEqual([{ type: 'typing', conversationId: CONVERSATION }])
  })

  it('shows who is typing until their message arrives', () => {
    start()
    act(() => latest().receive({ type: 'ephemeral', kind: 'typing', conversationId: CONVERSATION, accountId: ACCOUNT, until: new Date(Date.now() + 6_000).toISOString() }))
    expect(Object.keys(seen.typing)).toEqual([ACCOUNT])
    act(() =>
      latest().receive({
        type: 'event',
        position: '9',
        event: {
          event_id: '0193a0aa-0000-7000-8000-000000000001',
          event_cursor: 9,
          version: 1,
          project_id: '0192a0aa-0000-7000-8000-000000000002',
          conversationId: CONVERSATION,
          type: 'conversation.message.created',
          occurred_at: '2026-11-19T10:02:03Z',
          payload: { conversationId: CONVERSATION, messageId: '0193a0aa-0000-7000-8000-000000000004', sequence: 4, authorActorId: ACCOUNT },
        },
      }),
    )
    expect(seen.typing).toEqual({})
  })

  it('drops a conversation the person lost access to', () => {
    const queryClient = start()
    queryClient.setQueryData(['project', 'space', 'conversation', CONVERSATION, 'timeline', false], { entries: ['secret'] })
    act(() =>
      latest().receive({
        type: 'access',
        added: { workspaces: [], spaces: [], conversations: [] },
        removed: { workspaces: [], spaces: [], conversations: [CONVERSATION] },
      }),
    )
    expect(queryClient.getQueryData(['project', 'space', 'conversation', CONVERSATION, 'timeline', false])).toBeUndefined()
  })

  it('makes a page still showing a space the person left lose it and ask again', async () => {
    const queryClient = start()
    const key = ['project', SPACE, 'work-items', 'detail', 'task-1']
    const asked = vi.fn(() => Promise.reject(new Error('Not found')))
    queryClient.setQueryData(key, { title: 'Private plan' })
    const observer = new QueryObserver(queryClient, { queryKey: key, queryFn: asked, retry: false, staleTime: Infinity })
    const seen: unknown[] = []
    const stop = observer.subscribe((result) => seen.push(result.data))
    act(() =>
      latest().receive({
        type: 'access',
        added: { workspaces: [], spaces: [], conversations: [] },
        removed: { workspaces: [], spaces: [SPACE], conversations: [] },
      }),
    )
    await vi.waitFor(() => expect(asked).toHaveBeenCalled())
    expect(observer.getCurrentResult().data).toBeUndefined()
    expect(queryClient.getQueryData(key)).toBeUndefined()
    stop()
  })
})
