import { describe, expect, it } from 'vitest'

import { listQuery, parametersFromRemembered, readListState, rememberedState, withFilter, writeListState } from './listModel'
import { buildListRows, visibleTaskIds, type ListRow } from './listRows'
import { emptySelection, keepLoaded, MAX_BULK, pick, pickerCurrent, pickerTargets, selectAll } from './selectionModel'
import type { WorkItemCard } from './types'

function card(id: string, group?: string): WorkItemCard {
  return {
    id,
    projectId: 'p',
    key: `AW-${id}`,
    number: 1,
    title: id,
    itemType: { id: 't', name: 'Task' },
    status: { id: 's', name: 'Todo', semanticCategory: 'unstarted' },
    priority: 'medium',
    assignee: null,
    version: 1,
    archivedAt: null,
    updatedAt: '',
    createdAt: '',
    labels: [],
    capabilities: { view: true, edit: true, archive: true },
    milestone: null,
    startDate: null,
    dueDate: null,
    estimate: null,
    ready: true,
    parentId: null,
    blockedReason: null,
    openBlockerCount: 0,
    subtaskCount: 0,
    subtasksDone: 0,
    claimedBy: null,
    boardOrder: '0000000001:00000000000000000000',
    ...(group ? { group: { key: group, label: group.toUpperCase() } } : {}),
  }
}

const kinds = (rows: ListRow[]) => rows.map((row) => (row.kind === 'task' ? `task:${row.item.id}` : row.kind === 'group' ? `group:${row.groupKey}` : 'loading'))

describe('list state in the URL', () => {
  it('round-trips every part of the view and leaves defaults out', () => {
    const query = 'view=board&statusId=s1&labelIds=a%2Cb&milestoneId=none&blocked=false&ready=true&includeDone=false&dueBefore=2026-10-31&q=login+bug&sort=due&groupBy=milestone&columnsBy=assignee&lanes=priority&task=t1'
    const state = readListState(new URLSearchParams(query))
    expect(state).toMatchObject({
      view: 'board',
      sort: 'due',
      groupBy: 'milestone',
      columnsBy: 'assignee',
      lanes: 'priority',
      task: 't1',
      filters: { statusId: 's1', labelIds: 'a,b', milestoneId: 'none', blocked: 'false', ready: true, includeDone: 'false', dueBefore: '2026-10-31', q: 'login bug' },
    })
    expect(readListState(writeListState(state))).toEqual(state)
    expect(writeListState(readListState(new URLSearchParams('sort=newest&view=list&lanes=none'))).toString()).toBe('')
  })

  it('drops values that are not valid instead of sending them', () => {
    const state = readListState(new URLSearchParams('sort=sideways&groupBy=mood&dueBefore=tomorrow&priority=whenever&statusId=%3Cscript%3E'))
    expect(state.sort).toBe('newest')
    expect(state.groupBy).toBeNull()
    expect(state.filters).toEqual({})
  })

  it('reads older links as the server does: "unassigned" is one more assignee, "no labels" one more label choice', () => {
    expect(readListState(new URLSearchParams('assigneeActorId=a1&unassigned=true')).filters).toEqual({ assigneeActorId: 'a1,none' })
    expect(readListState(new URLSearchParams('unassigned=true')).filters).toEqual({ assigneeActorId: 'none' })
    expect(readListState(new URLSearchParams('labelIds=none')).filters).toEqual({ anyLabelIds: 'none' })
    expect(writeListState(readListState(new URLSearchParams('unassigned=true'))).toString()).toBe('assigneeActorId=none')
  })

  it('keeps each filter to one spelling and drops lists the server would refuse', () => {
    const state = readListState(new URLSearchParams('statusId=s2,s1,s2&notAssigneeActorId=none,a1&priority=urgent,high&anyLabelIds=l1'))
    expect(state.filters).toEqual({ statusId: 's1,s2', notAssigneeActorId: 'a1,none', priority: 'high,urgent', anyLabelIds: 'l1' })
    expect(withFilter(state, 'statusId', undefined).filters).not.toHaveProperty('statusId')
    for (const query of ['statusId=none', 'priority=high,whenever', 'labelIds=none,l1', `statusId=${Array.from({ length: 21 }, (_, n) => `s${n}`).join(',')}`]) {
      expect(readListState(new URLSearchParams(query)).filters).toEqual({})
    }
  })

  it('sends grouping and sort with the filters, and remembers everything but the open task', () => {
    const state = readListState(new URLSearchParams('priority=high&groupBy=label&sort=manual&task=t9'))
    expect(listQuery(state)).toEqual({ priority: 'high', groupBy: 'label', sort: 'manual' })
    const remembered = rememberedState(state)
    expect(remembered).toEqual({ priority: 'high', sort: 'manual', groupBy: 'label' })
    expect(parametersFromRemembered({ ...remembered, groupBy: 'nonsense', ready: true }).toString()).toBe('priority=high&ready=true&sort=manual')
  })
})

describe('list rows from keyset pages', () => {
  it('puts the loading row where the next page lands and shows groups that have not loaded yet', () => {
    const groups = [
      { key: 'm1', label: 'M1', count: 2 },
      { key: 'm2', label: 'M2', count: 3 },
      { key: 'none', label: 'No milestone', count: 1 },
    ]
    const rows = buildListRows({ items: [card('1', 'm1'), card('2', 'm1'), card('3', 'm2')], grouped: true, groups, collapsed: new Set(), hasMore: true })
    expect(kinds(rows)).toEqual(['group:m1', 'task:1', 'task:2', 'group:m2', 'task:3', 'loading', 'group:none'])
    const m2 = rows.find((row) => row.kind === 'group' && row.groupKey === 'm2')
    expect(m2).toMatchObject({ count: 3, loaded: 1 })
  })

  it('shows a task once per label group but selects and moves through it once', () => {
    const items = [card('1', 'bug'), card('2', 'bug'), card('1', 'ui')]
    const rows = buildListRows({ items, grouped: true, groups: undefined, collapsed: new Set(), hasMore: false })
    expect(kinds(rows)).toEqual(['group:bug', 'task:1', 'task:2', 'group:ui', 'task:1'])
    expect(new Set(rows.map((row) => row.key)).size).toBe(rows.length)
    expect(visibleTaskIds(rows)).toEqual(['1', '2'])
  })

  it('hides the tasks of a collapsed group but keeps its header and count', () => {
    const groups = [{ key: 'a', label: 'A', count: 1 }, { key: 'b', label: 'B', count: 1 }]
    const rows = buildListRows({ items: [card('1', 'a'), card('2', 'b')], grouped: true, groups, collapsed: new Set(['a']), hasMore: false })
    expect(kinds(rows)).toEqual(['group:a', 'group:b', 'task:2'])
  })

  it('starts loading at the top before the first page arrives, and dedupes ungrouped pages', () => {
    const groups = [{ key: 'a', label: 'A', count: 4 }]
    expect(kinds(buildListRows({ items: [], grouped: true, groups, collapsed: new Set(), hasMore: true }))).toEqual(['loading', 'group:a'])
    expect(kinds(buildListRows({ items: [card('1'), card('1'), card('2')], grouped: false, groups: undefined, collapsed: new Set(), hasMore: true }))).toEqual([
      'task:1',
      'task:2',
      'loading',
    ])
  })
})

describe('bulk selection', () => {
  const order = ['a', 'b', 'c', 'd', 'e']

  it('toggles one, then extends a range from the anchor with Shift', () => {
    let selection = pick(emptySelection, 'b', order, { toggle: true })
    selection = pick(selection, 'd', order, { shift: true })
    expect([...selection.ids]).toEqual(['b', 'c', 'd'])
    selection = pick(selection, 'c', order, { toggle: true })
    expect([...selection.ids].sort()).toEqual(['b', 'd'])
    selection = pick(selection, 'a', order, { shift: true })
    expect([...selection.ids].sort()).toEqual(['a', 'b', 'c', 'd'])
  })

  it('selects everything shown and forgets tasks that are no longer loaded', () => {
    const all = selectAll(order)
    expect(all.ids.size).toBe(5)
    const kept = keepLoaded(all, new Set(['a', 'b']))
    expect([...kept.ids]).toEqual(['a', 'b'])
    expect(kept.anchor).toBeNull()
    expect(keepLoaded(kept, new Set(order))).toBe(kept)
    const many = Array.from({ length: MAX_BULK + 50 }, (_, index) => `t${index}`)
    expect(selectAll(many).ids.size).toBe(MAX_BULK)
  })

  it('changes the selection from a selected card, only that card from another, and never what can’t change', () => {
    const [a, b, c] = [card('a'), card('b'), card('c')]
    const locked = { ...card('d'), capabilities: { view: true, edit: false, archive: false } }
    const archived = { ...card('e'), archivedAt: '2026-10-19T00:00:00Z' }
    const ids = (cards: WorkItemCard[]) => cards.map((entry) => entry.id)
    expect(ids(pickerTargets([a, b, locked, archived], a))).toEqual(['a', 'b'])
    expect(ids(pickerTargets([a, b], c))).toEqual(['c'])
    expect(ids(pickerTargets([a, b]))).toEqual(['a', 'b'])
    expect(pickerTargets([], locked)).toEqual([])
  })

  it('shows the values of one task, and only the labels every selected task has', () => {
    const red = { id: 'red', name: 'Red', color: '#f00' }
    const blue = { id: 'blue', name: 'Blue', color: '#00f' }
    const a = { ...card('a'), labels: [red, blue] }
    const b = { ...card('b'), labels: [blue] }
    expect(pickerCurrent([a, b])).toEqual({ labelIds: ['blue'] })
    expect(pickerCurrent([a])).toMatchObject({ statusId: 's', priority: 'medium', assigneeId: null, labelIds: ['red', 'blue'] })
  })
})
