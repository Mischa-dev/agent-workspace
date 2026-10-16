import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'

import { applyMove, dropAnchor, fieldMove, lanesFor, runOptimisticMove } from './boardModel'
import { changeSentence, formatChanges } from './historyFormat'
import type { WorkItemBoardResponse, WorkItemCard, WorkItemWorkflow } from './types'
import { addStatus, draftFromWorkflow, draftProblem, removeStatus, setAllowAny, togglePair, workflowUpdate } from './workflowModel'

function card(id: string): WorkItemCard {
  return {
    id,
    projectId: 'p',
    key: `AW-${id}`,
    number: 1,
    title: id,
    itemType: { id: 't', name: 'Task' },
    status: { id: 'todo', name: 'Todo', semanticCategory: 'unstarted' },
    priority: 'medium',
    assignee: null,
    version: 1,
    archivedAt: null,
    updatedAt: '',
    createdAt: '',
    labels: [{ id: 'bug', name: 'Bug', color: '#f00' }],
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
  }
}

function column(key: string, ids: string[]) {
  return { key, label: key, status: { id: key, name: key, semanticCategory: 'unstarted' }, items: ids.map(card), hasMore: false, total: ids.length, nextCursor: null, filter: { statusId: key } }
}

const board: WorkItemBoardResponse = { columnsBy: 'status', columns: [column('todo', ['1', '2', '3']), column('doing', ['4'])] }
const ids = (result: WorkItemBoardResponse) => result.columns.map((entry) => entry.items.map((item) => item.id).join(','))

describe('history changes', () => {
  it('reads each field as "Label from → to" with names, dates, units and empty values', () => {
    const lines = formatChanges(
      {
        dueDate: { from: null, to: '2026-11-03' },
        priority: { from: 'high', to: 'urgent' },
        assignee: { from: 'Ada', to: null },
        estimate: { from: 1, to: 3 },
        parent: { from: null, to: 'AW-7' },
        title: { from: 'Same', to: 'Same' },
      },
      'hours',
    ).map(changeSentence)
    expect(lines).toEqual(['Priority High → Urgent', 'Assignee Ada → Unassigned', 'Parent No parent → AW-7', 'Due date No date → Nov 3, 2026', 'Estimate 1 hour → 3 hours'])
  })

  it('turns a description diff into added, removed and unchanged lines', () => {
    const [change] = formatChanges({ description: { diff: ' keep\n-old\n+new\n', truncated: true } })
    expect(change).toEqual({
      kind: 'description',
      field: 'description',
      label: 'Description',
      truncated: true,
      lines: [
        { kind: 'same', text: 'keep' },
        { kind: 'removed', text: 'old' },
        { kind: 'added', text: 'new' },
      ],
    })
  })
})

describe('board moves', () => {
  it('anchors a drop to the card it lands above, or below the last card', () => {
    const items = board.columns[0].items
    expect(dropAnchor(items, '3', 0)).toEqual({ beforeId: '1' })
    expect(dropAnchor(items, '1', 2)).toEqual({ afterId: '3' })
    expect(dropAnchor([items[0]], '1', 0)).toEqual({})
  })

  it('reorders within a column and moves across columns with the right totals', () => {
    expect(ids(applyMove(board, '3', 'todo', 0))).toEqual(['3,1,2', '4'])
    const moved = applyMove(board, '1', 'doing', null)
    expect(ids(moved)).toEqual(['2,3', '4,1'])
    expect(moved.columns.map((entry) => entry.total)).toEqual([2, 2])
    expect(applyMove(board, 'missing', 'doing', 0)).toBe(board)
  })

  it('moves a label card between label columns by swapping just those labels', () => {
    const move = fieldMove('label', 'bug', 'ui', 'UI')
    expect(move.patch).toEqual({ addLabelIds: ['ui'], removeLabelIds: ['bug'] })
    expect(move.apply(card('1')).labels.map((label) => label.id)).toEqual(['ui'])
    expect(fieldMove('assignee', 'a1', 'none', 'Unassigned').patch).toEqual({ assigneeActorId: null })
    const cleared = fieldMove('label', 'bug', 'none', 'No label', undefined, ['bug', 'ui'])
    expect(cleared.patch).toEqual({ removeLabelIds: ['bug', 'ui'] })
  })

  it('offers a lane for every known value, used or not, with none last', () => {
    const lanes = lanesFor([], 'milestone', [
      { key: 'none', label: 'No milestone' },
      { key: 'm2', label: 'v2' },
      { key: 'm1', label: 'v1' },
    ])
    expect(lanes.map((lane) => lane.key)).toEqual(['m1', 'm2', 'none'])
  })

  it('shows the move at once and puts the board back when the server refuses', async () => {
    const queryClient = new QueryClient()
    const key = ['project', 'p', 'work-items', 'board']
    queryClient.setQueryData(key, board)
    let seenDuringSend: WorkItemBoardResponse | undefined
    await expect(
      runOptimisticMove(queryClient, key, (current) => applyMove(current, '1', 'doing', 0), async () => {
        seenDuringSend = queryClient.getQueryData(key)
        throw new Error('transition not allowed')
      }),
    ).rejects.toThrow('transition not allowed')
    expect(ids(seenDuringSend!)).toEqual(['2,3', '1,4'])
    expect(queryClient.getQueryData(key)).toEqual(board)

    await runOptimisticMove(queryClient, key, (current) => applyMove(current, '1', 'doing', 0), async () => 'ok')
    expect(ids(queryClient.getQueryData<WorkItemBoardResponse>(key)!)).toEqual(['2,3', '1,4'])
  })
})

describe('workflow drafts', () => {
  const saved: WorkItemWorkflow = {
    canEdit: true,
    version: 4,
    estimateUnit: 'points',
    types: [{ id: 'task', name: 'Task', position: 1, isDefault: true, taskCount: 3 }],
    statuses: [
      { id: 'todo', name: 'Todo', semanticCategory: 'unstarted', position: 1, isDefault: true, taskCount: 0 },
      { id: 'qa', name: 'QA', semanticCategory: 'review', position: 2, isDefault: false, taskCount: 4 },
      { id: 'done', name: 'Done', semanticCategory: 'completed', position: 3, isDefault: false, taskCount: 9 },
    ],
    transitions: [],
  }

  it('reads an empty transition list as no changes allowed and keeps it', () => {
    const draft = draftFromWorkflow(saved)
    expect(draft.allowAny).toBe(false)
    expect(workflowUpdate(draft, saved)).toMatchObject({ expectedVersion: 4, transitions: [] })
  })

  it('reads every pair as any change allowed and saves it as "all"', () => {
    const ids = saved.statuses.map((status) => status.id)
    const every = ids.flatMap((from) => ids.filter((to) => to !== from).map((to) => ({ fromStatusId: from, toStatusId: to })))
    const draft = draftFromWorkflow({ ...saved, transitions: every })
    expect(draft.allowAny).toBe(true)
    expect(workflowUpdate(draft, saved)).toMatchObject({ transitions: 'all' })
  })

  it('requires a home for the tasks of a removed status, then sends it as a replacement', () => {
    let draft = removeStatus(draftFromWorkflow(saved), 'qa')
    expect(draftProblem(draft, saved)).toBe('Choose where the 4 tasks in QA go.')
    draft = { ...draft, replacements: { qa: 'todo' } }
    expect(draftProblem(draft, saved)).toBeNull()
    const update = workflowUpdate(draft, saved)
    expect(update.statuses.map((status) => status.id)).toEqual(['todo', 'done'])
    expect(update.replacements).toEqual({ qa: 'todo' })
  })

  it('sends new statuses by key and limited changes as pairs', () => {
    let draft = setAllowAny(draftFromWorkflow(saved), false)
    draft = addStatus(draft, { ref: 'new-1', name: 'Blocked', semanticCategory: 'started', isDefault: false, taskCount: 0 })
    draft = togglePair(draft, 'done', 'todo')
    const update = workflowUpdate(draft, saved)
    expect(update.statuses.at(-1)).toEqual({ key: 'new-1', name: 'Blocked', semanticCategory: 'started', isDefault: false })
    expect(update.transitions).toContainEqual({ from: 'todo', to: 'new-1' })
    expect(update.transitions).not.toContainEqual({ from: 'done', to: 'todo' })
  })

  it('refuses two statuses with one name', () => {
    const draft = addStatus(draftFromWorkflow(saved), { ref: 'new-2', name: ' todo ', semanticCategory: 'started', isDefault: false, taskCount: 0 })
    expect(draftProblem(draft, saved)).toBe('Two statuses are called “Todo”.')
  })
})
