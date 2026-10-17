import { describe, expect, it } from 'vitest'

import { doneCountFromGroups, doneCountQuery, orderStatusColumns, statusBoardColumns, tucksDone } from './doneModel'
import { readListState, rememberedState } from './listModel'
import { withDoneRows, type ListRow } from './listRows'
import type { WorkItemCard, WorkItemWorkflow } from './types'

function workflow(statuses: Array<[id: string, category: string, position: number]>): WorkItemWorkflow {
  return {
    types: [],
    statuses: statuses.map(([id, semanticCategory, position]) => ({ id, name: id, semanticCategory, position, isDefault: false, taskCount: 0 })),
    transitions: [],
    canEdit: true,
    version: 1,
    estimateUnit: 'points',
  }
}

const column = (key: string, total = 0) => ({ key, status: null, total })

describe('done work behind its toggle', () => {
  it('is collapsed by default and only opens from the URL', () => {
    expect(readListState(new URLSearchParams('')).showDone).toBe(false)
    expect(readListState(new URLSearchParams('showDone=true')).showDone).toBe(true)
    expect(rememberedState(readListState(new URLSearchParams('view=board&showDone=true')))).toEqual({ view: 'board', showDone: 'true' })
  })

  it('tucks done work away unless a status filter or a hide-done view says otherwise', () => {
    expect(tucksDone({})).toBe(true)
    expect(tucksDone({ priority: 'high' })).toBe(true)
    expect(tucksDone({ statusId: 's1' })).toBe(false)
    expect(tucksDone({ semanticCategory: 'completed' })).toBe(false)
    expect(tucksDone({ includeDone: 'false' })).toBe(false)
  })

  it('counts done and canceled tasks across the matching filters', () => {
    const flow = workflow([
      ['todo', 'unstarted', 1],
      ['done', 'completed', 2],
      ['dropped', 'canceled', 3],
    ])
    const groups = [
      { key: 'todo', label: 'Todo', count: 4 },
      { key: 'done', label: 'Done', count: 2 },
      { key: 'dropped', label: 'Dropped', count: 1 },
    ]
    expect(doneCountFromGroups(groups, flow)).toBe(3)
    expect(doneCountFromGroups(undefined, flow)).toBe(0)
    expect(doneCountQuery({ priority: 'high', includeDone: 'false' })).toEqual({ priority: 'high', groupBy: 'status' })
  })

  it('adds a collapsed toggle row after the list, and the tasks only once it opens', () => {
    const card = { id: 'd1', group: undefined } as unknown as WorkItemCard
    const open: ListRow[] = [{ kind: 'task', key: 'a', groupKey: null, item: { ...card, id: 'a' } }]
    expect(withDoneRows(open, { count: 0, open: false, items: [], hasMore: false })).toBe(open)
    const collapsed = withDoneRows(open, { count: 5, open: false, items: [card], hasMore: true })
    expect(collapsed.map((row) => row.key)).toEqual(['a', 'group:__done'])
    expect(collapsed[1]).toMatchObject({ kind: 'group', collapsed: true, count: 5 })
    const expanded = withDoneRows(open, { count: 5, open: true, items: [card, card], hasMore: true })
    expect(expanded.map((row) => row.key)).toEqual(['a', 'group:__done', '__done:d1', '__loading:done'])
  })
})

describe('board columns', () => {
  it('follow workflow position, then status group where positions tie', () => {
    const flow = workflow([
      ['review', 'review', 3],
      ['canceled', 'canceled', 2],
      ['doing', 'started', 2],
      ['backlog', 'backlog', 0],
      ['todo', 'unstarted', 1],
    ])
    const columns = ['canceled', 'review', 'unknown', 'todo', 'doing', 'backlog'].map((key) => column(key))
    expect(orderStatusColumns(columns, flow).map((entry) => entry.key)).toEqual(['backlog', 'todo', 'doing', 'canceled', 'review', 'unknown'])
  })

  it('fold done and canceled columns away until shown, counting their tasks', () => {
    const flow = workflow([
      ['todo', 'unstarted', 1],
      ['done', 'completed', 2],
      ['doing', 'started', 3],
      ['dropped', 'canceled', 4],
    ])
    const columns = [column('dropped', 1), column('done', 4), column('doing', 2), column('todo', 3)]
    const tucked = statusBoardColumns(columns, flow, { tucked: true, showDone: false, hideDone: false })
    expect(tucked.visible.map((entry) => entry.key)).toEqual(['todo', 'doing'])
    expect(tucked.doneTotal).toBe(5)
    const shown = statusBoardColumns(columns, flow, { tucked: true, showDone: true, hideDone: false })
    expect(shown.visible.map((entry) => entry.key)).toEqual(['todo', 'done', 'doing', 'dropped'])
    // A status filter shows what it picked; a hide-done view hides done columns outright.
    expect(statusBoardColumns(columns, flow, { tucked: false, showDone: false, hideDone: false }).visible).toHaveLength(4)
    expect(statusBoardColumns(columns, flow, { tucked: false, showDone: true, hideDone: true }).visible).toHaveLength(2)
  })
})
