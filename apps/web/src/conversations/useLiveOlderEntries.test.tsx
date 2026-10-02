import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { RealtimeContext, type RealtimeContextValue } from '../realtime/realtimeContext'
import { normalizeTimelinePage } from './api'
import type { MessageUpdates } from './liveMessages'
import type { ConversationTimelineEntry } from './types'
import { useLiveOlderEntries, type OlderPage } from './useLiveOlderEntries'

const projectId = '019fb600-0000-7000-8000-000000000001'
const conversationId = '019fb600-0000-7000-8000-000000000002'

function messageId(sequence: number) {
  return `019fb601-0000-7000-8000-${String(sequence).padStart(12, '0')}`
}

function wireEntry(sequence: number, text: string, extra: Record<string, unknown> = {}) {
  return {
    id: `019fb602-0000-7000-8000-${String(sequence).padStart(12, '0')}`,
    sequence,
    kind: 'message',
    createdAt: '2026-10-01T10:00:00Z',
    message: {
      id: messageId(sequence),
      author: { id: projectId, kind: 'human', displayName: 'Bob', handle: 'bob', avatarUrl: null },
      content: { version: 1, nodes: [{ type: 'text', text }] },
      plainText: text,
      mentionedActors: [],
      referencedWorkItems: [],
      createdAt: '2026-10-01T10:00:00Z',
      ...extra,
    },
    workerEvent: null,
  }
}

function entries(...list: unknown[]): ConversationTimelineEntry[] {
  return normalizeTimelinePage({
    conversation: { id: conversationId, projectId, kind: 'channel' },
    entries: list,
  }).entries
}

let realtime: RealtimeContextValue = {
  connection: 'live',
  streams: {},
  messageUpdates: {},
  resyncGeneration: 1,
}

function wrapper({ children }: { children: ReactNode }) {
  return <RealtimeContext.Provider value={realtime}>{children}</RealtimeContext.Provider>
}

function setRealtime(change: Partial<RealtimeContextValue>) {
  realtime = { ...realtime, ...change }
}

const newest = entries(wireEntry(3, 'Newest'))

describe('live older timeline pages', () => {
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    realtime = { connection: 'live', streams: {}, messageUpdates: {}, resyncGeneration: 1 }
  })

  it('blanks a removal at once, refetches that message, and refetches every page on reconnect', async () => {
    const pages: Record<string, OlderPage> = {
      'cursor-a': { entries: entries(wireEntry(2, 'ghp_secret')), nextCursor: 'cursor-b' },
      'cursor-b': { entries: entries(wireEntry(1, 'First')), nextCursor: null },
    }
    const loadPage = vi.fn(async (cursor: string) => pages[cursor])
    const fetchMock = vi.fn(async () =>
      new Response(
        JSON.stringify({
          entry: wireEntry(2, '', {
            content: { version: 1, nodes: [] },
            removed: { removedAt: '2026-10-01T10:05:00Z', removedBy: { id: conversationId, kind: 'human', displayName: 'Mara', handle: 'mara', avatarUrl: null } },
          }),
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    vi.stubGlobal('fetch', fetchMock)

    const { result, rerender } = renderHook(
      () => useLiveOlderEntries({ conversationId, loadPage, newestCursor: 'cursor-a', newestEntries: newest }),
      { wrapper },
    )
    expect(result.current.cursor).toBe('cursor-a')
    await act(() => result.current.loadOlder())
    await act(() => result.current.loadOlder())
    expect(result.current.entries.map((entry) => entry.sequence)).toEqual([1, 2, 3])
    expect(result.current.cursor).toBeNull()

    const updates: MessageUpdates = {
      [messageId(2)]: { conversationId, messageId: messageId(2), revision: 1, removed: true },
    }
    setRealtime({ messageUpdates: updates })
    rerender()
    // Gone in the same render, before the refetch answers.
    expect(JSON.stringify(result.current.entries)).not.toContain('ghp_secret')
    expect(result.current.entries[1].message?.removed).not.toBeNull()
    await waitFor(() => expect(result.current.entries[1].message?.removed?.removedBy?.displayName).toBe('Mara'))
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(String((fetchMock.mock.calls[0] as unknown[])[0])).toBe(
      `/api/v1/conversations/${conversationId}/messages/${messageId(2)}`,
    )

    // Reconnecting refetches each older page by the cursor that loaded it.
    pages['cursor-b'] = { entries: entries(wireEntry(1, 'First, edited', { revision: 2 })), nextCursor: null }
    loadPage.mockClear()
    setRealtime({ resyncGeneration: 2 })
    rerender()
    await waitFor(() => expect(result.current.entries[0].message?.plainText).toBe('First, edited'))
    expect(loadPage.mock.calls.map(([cursor]) => cursor).sort()).toEqual(['cursor-a', 'cursor-b'])
  })

  it('drops older pages on leaving, so a reconnect missed elsewhere never shows them on return', async () => {
    const otherId = '019fb600-0000-7000-8000-000000000009'
    const loadPage = vi.fn(async () => ({ entries: entries(wireEntry(1, 'ghp_secret')), nextCursor: null }))
    const { result, rerender } = renderHook(
      ({ id }: { id: string }) =>
        useLiveOlderEntries({
          conversationId: id,
          loadPage,
          newestCursor: id === conversationId ? 'cursor-a' : null,
          newestEntries: id === conversationId ? newest : [],
        }),
      { wrapper, initialProps: { id: conversationId } },
    )
    await act(() => result.current.loadOlder())
    expect(JSON.stringify(result.current.entries)).toContain('ghp_secret')

    // Elsewhere, the socket drops and reconnects; the removal was never seen.
    rerender({ id: otherId })
    setRealtime({ resyncGeneration: 2 })
    rerender({ id: otherId })
    expect(result.current.entries).toEqual([])

    rerender({ id: conversationId })
    expect(result.current.entries.map((entry) => entry.sequence)).toEqual([3])
    expect(JSON.stringify(result.current.entries)).not.toContain('ghp_secret')
    expect(result.current.cursor).toBe('cursor-a')
  })

  it('never keeps a page load that answers after leaving, even once back', async () => {
    const otherId = '019fb600-0000-7000-8000-000000000009'
    let answer: (page: OlderPage) => void = () => undefined
    const loadPage = vi.fn(() => new Promise<OlderPage>((resolve) => (answer = resolve)))
    const { result, rerender } = renderHook(
      ({ id }: { id: string }) =>
        useLiveOlderEntries({ conversationId: id, loadPage, newestCursor: 'cursor-a', newestEntries: newest }),
      { wrapper, initialProps: { id: conversationId } },
    )
    let pending: Promise<void> = Promise.resolve()
    act(() => {
      pending = result.current.loadOlder()
    })
    expect(result.current.loading).toBe(true)
    rerender({ id: otherId })
    expect(result.current.loading).toBe(false)
    rerender({ id: conversationId })

    await act(async () => {
      answer({ entries: entries(wireEntry(1, 'ghp_secret')), nextCursor: null })
      await pending
    })
    expect(result.current.entries.map((entry) => entry.sequence)).toEqual([3])
    expect(result.current.cursor).toBe('cursor-a')
    expect(result.current.loading).toBe(false)
  })

  it('drops older pages when the reconnect refetch fails', async () => {
    const loadPage = vi.fn(async () => ({ entries: entries(wireEntry(1, 'Older')), nextCursor: null }))
    const { result, rerender } = renderHook(
      () => useLiveOlderEntries({ conversationId, loadPage, newestCursor: 'cursor-a', newestEntries: newest }),
      { wrapper },
    )
    await act(() => result.current.loadOlder())
    expect(result.current.entries).toHaveLength(2)

    loadPage.mockRejectedValueOnce(new Error('offline'))
    setRealtime({ resyncGeneration: 2 })
    rerender()
    await waitFor(() => expect(result.current.entries).toHaveLength(1))
    expect(result.current.cursor).toBe('cursor-a')
  })
})
