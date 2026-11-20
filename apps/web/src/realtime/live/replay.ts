import { act, cleanup, render } from '@testing-library/react'
import { QueryClient, QueryClientProvider, QueryObserver, type QueryKey } from '@tanstack/react-query'
import { createElement } from 'react'
import { vi } from 'vitest'

import { aiKeys } from '../../ai/keys'
import { queryKeys } from '../../api/queryKeys'
import { collectionKeys } from '../../collections/keys'
import { threadsQueryKey } from '../../conversations/threadModel'
import { memoryKeys } from '../../memory/keys'
import { runKeys } from '../../runs/keys'
import { sheetKeys } from '../../sheets/keys'
import { RealtimeProvider } from '../RealtimeProvider'
import { FIXTURE_IDS, type FixtureIdName } from './fixtureIds'

// W1.7: replays a recorded stream of server frames through the real live
// connection into a real query cache, with a stub socket and a counting
// fetch, and says what was fetched and what went stale. Test-only.

/**
 * Seeds name keys by factory, so a renamed factory fails here as it would in
 * the app: `["timeline", "$project", "$channel"]` is
 * `queryKeys.timeline(project, channel)`. `$name` is a FIXTURE_IDS id; any
 * other argument is passed as it is.
 */
const FACTORIES: Record<string, (...args: never[]) => QueryKey> = {
  rail: () => queryKeys.projects,
  project: queryKeys.project,
  direct: () => queryKeys.direct,
  inbox: () => queryKeys.inbox,
  myWork: () => queryKeys.myWork,
  timeline: queryKeys.timeline,
  sidebar: queryKeys.conversationSidebar,
  threads: threadsQueryKey,
  thing: queryKeys.thing,
  workItems: queryKeys.workItems,
  workItemBoard: queryKeys.workItemBoard,
  workItem: queryKeys.workItem,
  workItemTimeline: queryKeys.workItemTimeline,
  planning: aiKeys.suggestions,
  pages: memoryKeys.pages,
  page: memoryKeys.page,
  pins: memoryKeys.pins,
  sheets: sheetKeys.list,
  collections: collectionKeys.lists,
  runs: (() => runKeys.lists) as never,
  members: queryKeys.members,
}

export type SeedEntry = { name: string; key: [string, ...unknown[]]; data?: unknown }
/** Queries on screen (they refetch when stale) and queries only cached. */
export type Seed = { open?: SeedEntry[]; cached?: SeedEntry[] }
/** A recorded frame, or a socket event the recorder marked. */
export type Frame = Record<string, unknown> | { $: 'close' } | { $: 'open' }

export type ReplayRun = {
  /** Open queries by name, and how many times each was fetched. */
  fetches: Record<string, number>
  /** Seeded queries, by name, that went stale. */
  invalidated: string[]
  /** What the client sent once the stream started, frame by frame (W1.6). */
  sent: unknown[]
}

export function seedKey([factory, ...args]: SeedEntry['key']): QueryKey {
  const build = FACTORIES[factory]
  if (!build) throw new Error(`replay: no key factory named ${factory}`)
  const resolved = args.map((arg) => (typeof arg === 'string' && arg.startsWith('$') ? FIXTURE_IDS[arg.slice(1) as FixtureIdName] ?? arg : arg))
  return (build as (...values: unknown[]) => QueryKey)(...resolved)
}

/** JSON Lines, a frame a line. */
export function parseFrames(text: string): Frame[] {
  return text
    .split('\n')
    .filter((line) => line.trim())
    .map((line) => JSON.parse(line) as Frame)
}

class ReplaySocket extends EventTarget {
  static readonly CONNECTING = 0
  static readonly OPEN = 1
  static readonly CLOSING = 2
  static readonly CLOSED = 3
  static instances: ReplaySocket[] = []
  readyState = ReplaySocket.CONNECTING
  sent: string[] = []

  constructor(public readonly url: string) {
    super()
    ReplaySocket.instances.push(this)
  }

  open() {
    this.readyState = ReplaySocket.OPEN
    this.dispatchEvent(new Event('open'))
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    if (this.readyState === ReplaySocket.CLOSED) return
    this.readyState = ReplaySocket.CLOSED
    this.dispatchEvent(new Event('close'))
  }

  receive(frame: unknown) {
    this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(frame) }))
  }
}

const settle = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

/**
 * Plays `frames` into a connection that has just opened. `settleMs` is how
 * long to wait afterwards for delayed refetches (a burst's 40 ms, a page's
 * details' 3 s).
 */
export async function replay(frames: Frame[], seed: Seed, { settleMs = 200 } = {}): Promise<ReplayRun> {
  ReplaySocket.instances = []
  vi.stubGlobal('WebSocket', ReplaySocket)
  // Anything that does reach the network is a fetch the stream caused.
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => undefined)))
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } })
  const fetches: Record<string, number> = {}
  const unsubscribes: Array<() => void> = []
  for (const entry of [...(seed.open ?? []), ...(seed.cached ?? [])]) queryClient.setQueryData(seedKey(entry.key), entry.data ?? {})
  for (const entry of seed.open ?? []) {
    fetches[entry.name] = 0
    const observer = new QueryObserver(queryClient, {
      queryKey: seedKey(entry.key),
      queryFn: () => {
        fetches[entry.name] += 1
        return entry.data ?? {}
      },
      staleTime: Infinity,
    })
    unsubscribes.push(observer.subscribe(() => undefined))
  }
  try {
    render(createElement(QueryClientProvider, { client: queryClient }, createElement(RealtimeProvider, null, null)))
    const socket = () => ReplaySocket.instances[ReplaySocket.instances.length - 1]
    // The stream starts on an open connection: what opening refetches isn't the stream's.
    await act(async () => {
      socket().open()
      await settle(50)
    })
    for (const entry of seed.open ?? []) fetches[entry.name] = 0
    const started = ReplaySocket.instances.length - 1
    socket().sent = []
    for (const query of queryClient.getQueryCache().getAll()) query.reset()
    for (const entry of [...(seed.open ?? []), ...(seed.cached ?? [])]) queryClient.setQueryData(seedKey(entry.key), entry.data ?? {})
    await act(async () => {
      for (const frame of frames) {
        if ('$' in frame && frame.$ === 'close') socket().close()
        else if ('$' in frame && frame.$ === 'open') {
          // The browser came back online: the connection reconnects at once.
          window.dispatchEvent(new Event('online'))
          socket().open()
        } else socket().receive(frame)
      }
      await settle(settleMs)
    })
    const invalidated = [...(seed.open ?? []), ...(seed.cached ?? [])]
      .filter((entry) => queryClient.getQueryState(seedKey(entry.key))?.isInvalidated || fetches[entry.name] > 0)
      .map((entry) => entry.name)
      .sort()
    const sent = ReplaySocket.instances.slice(started).flatMap((each) => each.sent.map((frame) => JSON.parse(frame) as unknown))
    return { fetches, invalidated, sent }
  } finally {
    for (const unsubscribe of unsubscribes) unsubscribe()
    cleanup()
    queryClient.clear()
    vi.unstubAllGlobals()
  }
}
