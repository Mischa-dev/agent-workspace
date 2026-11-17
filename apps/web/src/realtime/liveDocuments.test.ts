import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { collectionChange, sheetChange, documentChanged, lastHeardVersion, onPageChange, onWhiteboardChange, pageChange, whiteboardChange } from './liveDocuments'
import type { RealtimeEnvelope } from './realtimeModel'
import { queryKeys } from '../api/queryKeys'
import { collectionKeys } from '../collections/keys'
import { memoryKeys } from '../memory/keys'
import { sheetKeys } from '../sheets/keys'
import { whiteboardKeys } from '../whiteboards/keys'

const PROJECT = '0190f5a4-0000-7000-8000-000000000001'
const PAGE = '0190f5a4-0000-7000-8000-000000000002'
const BOARD = '0190f5a4-0000-7000-8000-000000000003'
const COLLECTION = '0190f5a4-0000-7000-8000-000000000004'

function envelope(type: string, payload: Record<string, unknown>): RealtimeEnvelope {
  return base(type, payload)
}

function base(type: string, payload: Record<string, unknown>): RealtimeEnvelope {
  return {
    eventId: '0190f5a4-0000-7000-8000-0000000000ff',
    eventCursor: 1,
    version: 1,
    projectId: PROJECT,
    conversationId: null,
    type,
    occurredAt: '2026-11-11T00:00:00Z',
    payload,
  }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('live documents from the realtime socket', () => {
  it('reads page and board changes, and nothing malformed', () => {
    expect(pageChange(envelope('page.updated', { pageId: PAGE, version: 4 }))).toEqual({ pageId: PAGE, version: 4 })
    expect(pageChange(envelope('page.updated', { pageId: 'nope', version: 4 }))).toBeNull()
    expect(pageChange(envelope('page.updated', { pageId: PAGE, version: 0 }))).toBeNull()
    expect(pageChange(envelope('whiteboard.updated', { pageId: PAGE, version: 4 }))).toBeNull()
    expect(whiteboardChange(envelope('whiteboard.updated', { whiteboardId: BOARD, sceneSeq: 2, version: 3 }))).toEqual({
      whiteboardId: BOARD,
      sceneSeq: 2,
      version: 3,
    })
  })

  it('tells the open page, then refreshes its details once while someone types', () => {
    vi.useFakeTimers()
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue()
    const heard: number[] = []
    const stop = onPageChange(PAGE, (change) => heard.push(change.version))
    const board = vi.fn()
    const stopBoard = onWhiteboardChange(BOARD, board)
    documentChanged(queryClient, envelope('page.updated', { pageId: PAGE, version: 2 }))
    documentChanged(queryClient, envelope('page.updated', { pageId: PAGE, version: 3 }))
    expect(heard).toEqual([2, 3])
    // A page that opens next hears where the page got to.
    expect(lastHeardVersion(PAGE)).toBe(3)
    documentChanged(queryClient, envelope('page.updated', { pageId: PAGE, version: 2 }))
    expect(lastHeardVersion(PAGE)).toBe(3)
    expect(board).not.toHaveBeenCalled()
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(3_000)
    expect(invalidate).toHaveBeenCalledTimes(1)
    stop()
    stopBoard()
    documentChanged(queryClient, envelope('page.updated', { pageId: PAGE, version: 4 }))
    expect(heard).toEqual([2, 3, 2])
  })

  it('refetches what is open of a changed collection once for a burst of changes', () => {
    vi.useFakeTimers()
    expect(collectionChange(envelope('collection.updated', { collectionId: COLLECTION, recordIds: [] }))).toEqual({ collectionId: COLLECTION })
    expect(collectionChange(envelope('collection.updated', { collectionId: 'nope' }))).toBeNull()
    expect(collectionChange(envelope('page.updated', { collectionId: COLLECTION }))).toBeNull()
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue()
    documentChanged(queryClient, envelope('collection.updated', { collectionId: COLLECTION, recordIds: [PAGE] }))
    documentChanged(queryClient, envelope('collection.updated', { collectionId: COLLECTION, recordIds: [BOARD] }))
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(invalidate.mock.calls.map(([filters]) => filters?.queryKey)).toEqual([
      ['project', PROJECT, 'collections', COLLECTION],
      ['project', PROJECT, 'collections', 'list'],
      // Open dashboards, by a predicate over their op keys.
      undefined,
    ])
    const dashboards = invalidate.mock.calls[2][0]?.predicate
    expect(dashboards?.({ queryKey: ['op', null, 'dashboard_get', { dashboard: PAGE }] } as never)).toBe(true)
    expect(dashboards?.({ queryKey: ['op', null, 'dashboard_list', { spaceId: PROJECT }] } as never)).toBe(false)
  })

  it('refetches open dashboards and their lists once when one changes', () => {
    vi.useFakeTimers()
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue()
    documentChanged(queryClient, envelope('dashboard.updated', { dashboardId: PAGE, version: 2 }))
    documentChanged(queryClient, envelope('dashboard.updated', { dashboardId: PAGE, version: 3 }))
    vi.advanceTimersByTime(300)
    expect(invalidate).toHaveBeenCalledTimes(1)
    const matches = invalidate.mock.calls[0][0]?.predicate
    expect(matches?.({ queryKey: ['op', null, 'dashboard_list', { spaceId: PROJECT }] } as never)).toBe(true)
    expect(matches?.({ queryKey: ['project', PROJECT, 'collections', 'list'] } as never)).toBe(false)
  })

  it('refetches an open sheet and the list of sheets once for a burst of changes', () => {
    vi.useFakeTimers()
    expect(sheetChange(envelope('sheet.updated', { sheetId: COLLECTION, revision: 3 }))).toEqual({ sheetId: COLLECTION })
    expect(sheetChange(envelope('sheet.updated', { sheetId: 'nope' }))).toBeNull()
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue()
    documentChanged(queryClient, envelope('sheet.updated', { sheetId: COLLECTION, revision: 3 }))
    documentChanged(queryClient, envelope('sheet.updated', { sheetId: COLLECTION, revision: 4 }))
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(300)
    expect(invalidate.mock.calls.map(([filters]) => filters?.queryKey)).toEqual([
      ['project', PROJECT, 'sheets', COLLECTION],
      ['project', PROJECT, 'sheets', 'list'],
      undefined,
    ])
    // The third refetches this sheet's discussion summaries, and no other's.
    const summaries = invalidate.mock.calls[2]?.[0]?.predicate
    const query = (queryKey: unknown[]) => ({ queryKey }) as unknown as Parameters<NonNullable<typeof summaries>>[0]
    expect(summaries?.(query(['op', null, 'comment_list', { thing: { kind: 'sheet', id: COLLECTION } }]))).toBe(true)
    expect(summaries?.(query(['op', null, 'comment_list', { thing: { kind: 'sheet', id: 'other' } }]))).toBe(false)
    expect(summaries?.(query(['op', null, 'page_list', {}]))).toBe(false)
  })

  // The pages cache under their features' key factories; the live connection
  // names the same factories (W1.2), so a key that changes shape in one place
  // still reaches what is on screen. Seeded with real queries, not literals.
  it('refreshes what the pages cached, by their own key factories', () => {
    vi.useFakeTimers()
    // Fresh ids: an earlier test's pending refresh for the same thing would swallow these.
    const [PROJECT, PAGE, BOARD, COLLECTION] = ['11', '12', '13', '14'].map((n) => `0190f5a4-0000-7000-8000-0000000000${n}`)
    const envelope = (type: string, payload: Record<string, unknown>) => ({ ...base(type, payload), projectId: PROJECT })
    const queryClient = new QueryClient()
    const seeded = {
      records: collectionKeys.records(PROJECT, COLLECTION, 'view'),
      collections: collectionKeys.list(PROJECT, false),
      sheetTab: sheetKeys.tab(PROJECT, COLLECTION, 'tab-1'),
      sheets: sheetKeys.list(PROJECT),
      discussion: memoryKeys.discussion(PROJECT, PAGE),
      page: memoryKeys.page(PROJECT, PAGE),
      pageThing: queryKeys.thing('page', PAGE),
      boards: whiteboardKeys.list({}),
      boardThing: queryKeys.thing('whiteboard', BOARD),
      unrelated: memoryKeys.page(PROJECT, BOARD).slice(0, 3).concat(['decisions']),
    }
    for (const key of Object.values(seeded)) queryClient.setQueryData(key, { seeded: true })
    documentChanged(queryClient, envelope('collection.updated', { collectionId: COLLECTION, recordIds: [] }))
    documentChanged(queryClient, envelope('sheet.updated', { sheetId: COLLECTION, revision: 3 }))
    documentChanged(queryClient, envelope('page.discussion.updated', { pageId: PAGE }))
    documentChanged(queryClient, envelope('page.updated', { pageId: PAGE, version: 9 }))
    documentChanged(queryClient, envelope('whiteboard.updated', { whiteboardId: BOARD, sceneSeq: 2, version: 2 }))
    vi.advanceTimersByTime(5_000)
    const invalidated = (key: readonly unknown[]) => queryClient.getQueryState(key)?.isInvalidated
    const { unrelated, ...touched } = seeded
    for (const [name, key] of Object.entries(touched)) expect(invalidated(key), name).toBe(true)
    expect(invalidated(unrelated)).toBe(false)
  })
})
