import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { queryKeys } from '../api/queryKeys'
import { resetQueryScope } from '../api/queryScope'
import type { WorkItemCard, WorkItemListResponse } from '../work-items/types'
import { applyWorkItemChange, patchList, workItemChange } from './workItemPatches'

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
    createdAt: '2026-10-16T09:00:00Z',
    updatedAt: '2026-10-16T10:00:00Z',
    labels: [],
    capabilities: { view: true, edit: true, archive: true },
    milestone: null,
    startDate: null,
    dueDate: null,
    estimate: null,
    ready: false,
    parentId: null,
    blockedReason: null,
    openBlockerCount: 0,
    subtaskCount: 0,
    subtasksDone: 0,
    claimedBy: null,
    boardOrder: '',
  }
}

/** The detail endpoint's answer for a card. */
function detail(n: number, status = todo, version = 1) {
  return { ...card(n, status, version), description: '' }
}

function respond(body: unknown) {
  return new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
}

const page = (items: WorkItemCard[], nextCursor: string | null = null): WorkItemListResponse => ({
  items,
  nextCursor,
  total: items.length,
})

const numbers = (data: WorkItemListResponse | undefined) => data?.items.map((item) => item.number)

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

  it('drops a card that no longer matches the filters, and keeps newer copies', () => {
    const archived = { ...card(2, todo, 2), archivedAt: '2026-10-16T11:00:00Z' }
    const list = page([card(3), card(2), card(1)])
    const removed = patchList(list, archived, archived.id, {})
    expect(removed !== 'refetch' && removed !== 'unchanged' && numbers(removed.data)).toEqual([3, 1])
    expect(removed !== 'refetch' && removed !== 'unchanged' && removed.data.total).toBe(2)
    expect(patchList(list, card(2, todo, 0), id(2), {})).toBe('unchanged')
    const added = patchList(page([card(3), card(1)]), card(2), id(2), {})
    expect(added !== 'refetch' && added !== 'unchanged' && numbers(added.data)).toEqual([3, 2, 1])
  })

  it('matches filters with several values and "is not" on the card itself', () => {
    const ana = { actorId: id(50), displayName: 'Ana', handle: 'ana', kind: 'human' } as unknown as WorkItemCard['assignee']
    const matches = (item: WorkItemCard, filters: Parameters<typeof patchList>[3]) => {
      const patched = patchList(page([]), item, item.id, filters)
      return patched !== 'refetch' && patched !== 'unchanged' && patched.data.items.length === 1
    }
    expect(matches(card(2, doing), { statusId: `${todo.id},${doing.id}` })).toBe(true)
    expect(matches(card(2, doing), { notStatusId: doing.id })).toBe(false)
    expect(matches(card(2), { notSemanticCategory: 'started,completed' })).toBe(true)
    // "Is not" Ana keeps unassigned tasks unless "none" is left out too.
    expect(matches(card(2), { notAssigneeActorId: id(50) })).toBe(true)
    expect(matches(card(2), { notAssigneeActorId: `${id(50)},none` })).toBe(false)
    expect(matches({ ...card(2), assignee: ana }, { assigneeActorId: `${id(50)},none` })).toBe(true)
    expect(matches({ ...card(2), assignee: ana }, { unassigned: true })).toBe(false)
    expect(matches(card(2), { priority: 'high,urgent' })).toBe(false)
    expect(patchList(page([card(2)]), card(2, todo, 2), id(2), { anyLabelIds: 'none' })).toBe('refetch')
  })

  it('refetches a long list instead of leaving it a card short', () => {
    const archived = { ...card(2, todo, 2), archivedAt: '2026-10-16T11:00:00Z' }
    expect(patchList(page([card(3), card(2)], 'next'), archived, id(2), {})).toBe('refetch')
  })

  it('refetches a long page that gains a card', () => {
    expect(patchList(page([card(3), card(1)], 'next'), card(2), id(2), {})).toBe('refetch')
    expect(patchList(page([card(5), card(4)], 'next'), card(2), id(2), {})).toBe('unchanged')
  })

  it('refetches a wildcard search, which only the server can match', () => {
    expect(patchList(page([card(2)]), card(2, todo, 2), id(2), { q: '100%' })).toBe('refetch')
  })

  it('refetches a list filtered or sorted in a way only the server knows', () => {
    expect(patchList(page([card(2)]), card(2, todo, 2), id(2), { milestoneId: id(9) })).toBe('refetch')
    expect(patchList(page([card(2)]), card(2, todo, 2), id(2), { sort: 'title' })).toBe('refetch')
  })

  it('keeps the newer detail when an older fetch answers last', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const detailKey = queryKeys.workItem(projectId, id(1))
    const listKey = queryKeys.workItems(projectId, {})
    queryClient.setQueryData(listKey, page([card(2), card(1)]))
    queryClient.setQueryData(detailKey, detail(1))
    const answers: Array<(version: number) => void> = []
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((resolve) => {
      answers.push((version) => resolve(respond(detail(1, todo, version))))
    })))
    const older = applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 2, placement: null })
    const newer = applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 3, placement: null })
    await vi.waitFor(() => expect(answers).toHaveLength(2))
    answers[1](3)
    await newer
    answers[0](2)
    await older

    expect(queryClient.getQueryData(detailKey)).toMatchObject({ version: 3 })
    expect(queryClient.getQueryData<WorkItemListResponse>(listKey)!.items[1].version).toBe(3)
  })

  it('patches a plain list in place and refetches the task list page and board', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const listKey = queryKeys.workItems(projectId, {})
    const pagesKey = queryKeys.workItemPages(projectId, {})
    const boardKey = queryKeys.workItemBoard(projectId, { columnsBy: 'status' })
    const ordered = { ...card(1), boardOrder: '0001' }
    queryClient.setQueryData(listKey, page([card(2), ordered]))
    queryClient.setQueryData(pagesKey, { pages: [page([card(2), ordered])], pageParams: [''] })
    queryClient.setQueryData(boardKey, { columnsBy: 'status', columns: [] })
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      expect(String(input)).toBe(`/api/v1/projects/${projectId}/work-items/${id(1)}`)
      return respond({ ...detail(1, doing, 2), title: 'Renamed' })
    })
    vi.stubGlobal('fetch', fetchMock)

    await applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 2, placement: null })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const patched = queryClient.getQueryData<WorkItemListResponse>(listKey)!.items[1]
    expect(patched).toMatchObject({ title: 'Renamed', version: 2, boardOrder: '0001' })
    expect(queryClient.getQueryState(listKey)?.isInvalidated).toBe(false)
    expect(queryClient.getQueryState(pagesKey)?.isInvalidated).toBe(true)
    expect(queryClient.getQueryState(boardKey)?.isInvalidated).toBe(true)
    expect(queryClient.getQueryData(queryKeys.workItem(projectId, id(1)))).toMatchObject({ version: 2 })
  })

  it('writes nothing once the session ends mid-fetch', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    queryClient.setQueryData(queryKeys.workItems(projectId, {}), page([card(1)]))
    let answer: (() => void) | undefined
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((resolve) => {
      answer = () => resolve(respond(detail(1, doing, 2)))
    })))
    const applied = applyWorkItemChange(queryClient, projectId, { workItemId: id(1), version: 2, placement: null })
    await vi.waitFor(() => expect(answer).toBeDefined())
    queryClient.clear()
    resetQueryScope()
    answer!()
    await applied

    expect(queryClient.getQueryCache().getAll()).toHaveLength(0)
  })
})
