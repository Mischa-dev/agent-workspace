import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import type { WorkItemBoardResponse, WorkItemCard, WorkItemListResponse } from '../work-items/types'
import { applyWorkItemChange, patchBoard, patchList, workItemChange } from './workItemPatches'

const projectId = '019fb600-0000-7000-8000-000000000002'
const todo = { id: '019fb600-0000-7000-8000-0000000000a1', name: 'Todo', semanticCategory: 'unstarted' }
const doing = { id: '019fb600-0000-7000-8000-0000000000a2', name: 'Doing', semanticCategory: 'started' }
const id = (n: number) => `019fb600-0000-7000-8000-0000000001${String(n).padStart(2, '0')}`

function card(n: number, status = todo, version = 1): WorkItemCard {
  return {
    id: id(n),
    projectId,
    key: `AWD-${n}`,
    number: n,
    title: `Task ${n}`,
    itemType: { id: 'type', name: 'Task' },
    status,
    priority: 'no_priority',
    assignee: null,
    version,
    archivedAt: null,
    updatedAt: '2026-10-16T10:00:00Z',
    labels: [],
    capabilities: { view: true, edit: true, archive: true },
  }
}

function board(): WorkItemBoardResponse {
  return {
    columns: [
      { status: todo, items: [card(1), card(2), card(3)], hasMore: false },
      { status: doing, items: [card(4, doing), card(5, doing)], hasMore: false },
    ],
  }
}

const ids = (data: WorkItemBoardResponse) => data.columns.map((column) => column.items.map((item) => item.number))

describe('work item live patches', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('reads the placement and ignores a malformed one', () => {
    expect(workItemChange({
      type: 'work_item.updated',
      payload: { workItemId: id(2), version: 3, placement: { statusId: doing.id, archived: false, afterId: id(4) } },
    })).toEqual({
      workItemId: id(2),
      version: 3,
      placement: { statusId: doing.id, archived: false, afterId: id(4), beforeId: null },
    })
    expect(workItemChange({
      type: 'work_item.updated',
      payload: { workItemId: id(2), version: 3, placement: { statusId: 'nope', archived: false } },
    })?.placement).toBeNull()
    expect(workItemChange({ type: 'conversation.message.created', payload: {} })).toBeNull()
  })

  it('moves a card between columns beside its new neighbours', () => {
    const moved = card(2, doing, 2)
    const result = patchBoard(board(), moved, moved.id, {
      statusId: doing.id, archived: false, afterId: id(4), beforeId: id(5),
    }, {})
    expect(result !== 'refetch' && result !== 'unchanged' && ids(result.data)).toEqual([[1, 3], [4, 2, 5]])
  })

  it('refetches a board only when no neighbour is on screen', () => {
    const moved = card(2, doing, 2)
    expect(patchBoard(board(), moved, moved.id, {
      statusId: doing.id, archived: false, afterId: id(90), beforeId: id(91),
    }, {})).toBe('refetch')
  })

  it('drops a card that no longer matches the filters, and keeps newer copies', () => {
    const archived = { ...card(2, todo, 2), archivedAt: '2026-10-16T11:00:00Z' }
    const list: WorkItemListResponse = { items: [card(3), card(2), card(1)], nextCursor: null }
    const removed = patchList(list, archived, archived.id, {})
    expect(removed !== 'refetch' && removed !== 'unchanged' && removed.data.items.map((item) => item.number)).toEqual([3, 1])
    expect(patchList(list, card(2, todo, 0), id(2), {})).toBe('unchanged')
    const added = patchList({ items: [card(3), card(1)], nextCursor: null }, card(2), id(2), {})
    expect(added !== 'refetch' && added !== 'unchanged' && added.data.items.map((item) => item.number)).toEqual([3, 2, 1])
  })

  it('keeps the newer detail when an older fetch answers last', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const detailKey = queryKeys.workItem(projectId, id(1))
    const boardKey = [...queryKeys.workItems(projectId, { cursor: undefined }), 'board']
    queryClient.setQueryData(boardKey, board())
    queryClient.setQueryData(detailKey, { ...card(1), description: '', createdAt: '2026-10-16T09:00:00Z' })
    const answers: Array<(version: number) => void> = []
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((resolve) => {
      answers.push((version) => resolve(new Response(
        JSON.stringify({ ...card(1, todo, version), description: '', createdAt: '2026-10-16T09:00:00Z' }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )))
    })))
    const placement = { statusId: todo.id, archived: false, afterId: null, beforeId: id(2) }
    const older = applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 2, placement })
    const newer = applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 3, placement })
    await vi.waitFor(() => expect(answers).toHaveLength(2))
    answers[1](3)
    await newer
    answers[0](2)
    await older

    expect(queryClient.getQueryData(detailKey)).toMatchObject({ version: 3 })
    const shown = queryClient.getQueryData<WorkItemBoardResponse>(boardKey)!.columns[0].items[0]
    expect(shown.version).toBe(3)
  })

  it('leaves the card where a newer event put it', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const detailKey = queryKeys.workItem(projectId, id(1))
    const boardKey = [...queryKeys.workItems(projectId, { cursor: undefined }), 'board']
    // Version 3 already placed it after task 3; version 2's event said before task 2.
    queryClient.setQueryData(boardKey, {
      columns: [{ status: todo, items: [card(2), card(3), card(1, todo, 3)], hasMore: false }],
    })
    queryClient.setQueryData(detailKey, { ...card(1, todo, 3), description: '', createdAt: '2026-10-16T09:00:00Z' })
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    await applyWorkItemChange(queryClient, projectId, {
      workItemId: id(1),
      version: 2,
      placement: { statusId: todo.id, archived: false, afterId: null, beforeId: id(2) },
    })

    expect(fetchMock).not.toHaveBeenCalled()
    expect(ids(queryClient.getQueryData<WorkItemBoardResponse>(boardKey)!)).toEqual([[2, 3, 1]])
  })

  it('refetches a long list or column instead of leaving it a card short', () => {
    const archived = { ...card(2, todo, 2), archivedAt: '2026-10-16T11:00:00Z' }
    expect(patchList({ items: [card(3), card(2)], nextCursor: 'next' }, archived, id(2), {})).toBe('refetch')
    const long = board()
    long.columns[0].hasMore = true
    expect(patchBoard(long, archived, id(2), null, {})).toBe('refetch')
    expect(patchBoard(long, card(2, doing, 2), id(2), {
      statusId: doing.id, archived: false, afterId: id(4), beforeId: id(5),
    }, {})).toBe('refetch')
  })

  it('moves a card on an open board without refetching the board', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const boardKey = [...queryKeys.workItems(projectId, { cursor: undefined }), 'board']
    queryClient.setQueryData(boardKey, board())
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      expect(String(input)).toBe(`/api/v1/projects/${projectId}/work-items/${id(1)}`)
      return new Response(JSON.stringify({ ...card(1, doing, 2), description: '', createdAt: '2026-10-16T09:00:00Z' }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    })
    vi.stubGlobal('fetch', fetchMock)

    await applyWorkItemChange(queryClient, projectId, {
      workItemId: id(1),
      version: 2,
      placement: { statusId: doing.id, archived: false, afterId: null, beforeId: id(4) },
    })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(ids(queryClient.getQueryData<WorkItemBoardResponse>(boardKey)!)).toEqual([[2, 3], [1, 4, 5]])
    expect(queryClient.getQueryState(boardKey)?.isInvalidated).toBe(false)
    expect(queryClient.getQueryData(queryKeys.workItem(projectId, id(1)))).toMatchObject({ version: 2 })
  })
})
