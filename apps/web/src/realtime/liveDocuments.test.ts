import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { collectionChange, documentChanged, lastHeardVersion, onPageChange, onWhiteboardChange, pageChange, whiteboardChange } from './liveDocuments'
import type { RealtimeEnvelope } from './realtimeModel'

const PROJECT = '0190f5a4-0000-7000-8000-000000000001'
const PAGE = '0190f5a4-0000-7000-8000-000000000002'
const BOARD = '0190f5a4-0000-7000-8000-000000000003'
const COLLECTION = '0190f5a4-0000-7000-8000-000000000004'

function envelope(type: string, payload: Record<string, unknown>): RealtimeEnvelope {
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
    ])
  })
})
