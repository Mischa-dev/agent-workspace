import type { QueryClient, QueryKey } from '@tanstack/react-query'

import { priorityLabel } from './constants'
import type { FilterValues, LaneBy } from './listModel'
import type { BoardColumn, BoardColumnsBy, WorkItemBoardResponse, WorkItemCard, WorkItemPatch, WorkItemPriority } from './types'

/**
 * Board moves: where a dropped card lands, what changes on it, and the
 * optimistic update that shows the move at once and puts it back if the
 * server says no.
 */

/** Anchors for POST /move: put the card above the card now at `index`, or below the last one. */
export function dropAnchor(items: readonly WorkItemCard[], draggedId: string, index: number): { beforeId?: string; afterId?: string } {
  const others = items.filter((item) => item.id !== draggedId)
  if (others.length === 0) return {}
  const clamped = Math.max(0, Math.min(index, others.length))
  if (clamped < others.length) return { beforeId: others[clamped].id }
  return { afterId: others[others.length - 1].id }
}

/** Where the card would land among the column's other cards, counting it out. */
export function indexWithout(items: readonly WorkItemCard[], draggedId: string, index: number) {
  const from = items.findIndex((item) => item.id === draggedId)
  return from >= 0 && from < index ? index - 1 : index
}

export type FieldMove = {
  /** PATCH body fields, without expectedVersion. Empty when the field doesn't change. */
  patch: Omit<WorkItemPatch, 'expectedVersion'>
  apply: (card: WorkItemCard) => WorkItemCard
}

type Field = BoardColumnsBy | Exclude<LaneBy, 'none'>

/** The change that moving a card from group `fromKey` to group `toKey` of `field` means. */
/** `labelIds` are the card's labels now; a drop on "No label" removes them all. */
export function fieldMove(
  field: Field,
  fromKey: string,
  toKey: string,
  toLabel: string,
  status?: BoardColumn['status'],
  labelIds: string[] = [],
): FieldMove {
  const none = toKey === 'none'
  switch (field) {
    case 'status':
      return {
        patch: {},
        apply: (card) => (status ? { ...card, status } : card),
      }
    case 'assignee':
      return {
        patch: { assigneeActorId: none ? null : toKey },
        apply: (card) => ({
          ...card,
          assignee: none ? null : { actorId: toKey, displayName: toLabel, handle: '', kind: card.assignee?.kind ?? 'human', active: true },
        }),
      }
    case 'priority':
      return {
        patch: { priority: (none ? 'no_priority' : toKey) as WorkItemPriority },
        apply: (card) => ({ ...card, priority: (none ? 'no_priority' : toKey) as WorkItemPriority }),
      }
    case 'milestone':
      return {
        patch: { milestoneId: none ? null : toKey },
        apply: (card) => ({ ...card, milestone: none ? null : { id: toKey, name: toLabel, targetDate: card.milestone?.targetDate ?? null } }),
      }
    case 'type':
      return {
        patch: { typeId: toKey },
        apply: (card) => ({ ...card, itemType: { id: toKey, name: toLabel } }),
      }
    case 'label': {
      const remove = none ? labelIds : fromKey !== 'none' && fromKey !== toKey ? [fromKey] : []
      const add = !none && fromKey !== toKey ? [toKey] : []
      return {
        patch: { ...(add.length ? { addLabelIds: add } : {}), ...(remove.length ? { removeLabelIds: remove } : {}) },
        apply: (card) => ({
          ...card,
          labels: [
            ...card.labels.filter((label) => !remove.includes(label.id)),
            ...(add.length && !card.labels.some((label) => label.id === toKey) ? [{ id: toKey, name: toLabel, color: '#8b8d98' }] : []),
          ],
        }),
      }
    }
  }
}

/** The lane a card sits in when the board is split by `lanes`. */
export function laneOf(card: WorkItemCard, lanes: LaneBy): { key: string; label: string } {
  switch (lanes) {
    case 'assignee':
      return card.assignee ? { key: card.assignee.actorId, label: card.assignee.displayName } : { key: 'none', label: 'Unassigned' }
    case 'priority':
      return { key: card.priority, label: priorityLabel(card.priority) }
    case 'milestone':
      return card.milestone ? { key: card.milestone.id, label: card.milestone.name } : { key: 'none', label: 'No milestone' }
    case 'type':
      return { key: card.itemType.id, label: card.itemType.name }
    default:
      return { key: 'all', label: '' }
  }
}

export const PRIORITY_ORDER: WorkItemPriority[] = ['urgent', 'high', 'medium', 'low', 'no_priority']

/**
 * The swimlanes: every `known` value a card can be dropped on (the field's
 * definitions), plus any other value a loaded card carries, in a stable
 * order with "none" last.
 */
export function lanesFor(columns: readonly BoardColumn[], lanes: LaneBy, known: ReadonlyArray<{ key: string; label: string }> = []) {
  if (lanes === 'none') return [{ key: 'all', label: '' }]
  const found = new Map<string, string>(known.map((lane) => [lane.key, lane.label]))
  for (const column of columns) for (const card of column.items) {
    const lane = laneOf(card, lanes)
    found.set(lane.key, lane.label)
  }
  const entries = [...found.entries()].map(([key, label]) => ({ key, label }))
  return entries.sort((a, b) => {
    if (a.key === 'none') return 1
    if (b.key === 'none') return -1
    if (lanes === 'priority') return PRIORITY_ORDER.indexOf(a.key as WorkItemPriority) - PRIORITY_ORDER.indexOf(b.key as WorkItemPriority)
    return a.label.localeCompare(b.label)
  })
}

/**
 * The board after moving a card to `toIndex` among the cards of `toColumn`
 * (an index into that column's list without the card). `toIndex` of null
 * appends.
 */
export function applyMove(
  board: WorkItemBoardResponse,
  cardId: string,
  toColumnKey: string,
  toIndex: number | null,
  change: (card: WorkItemCard) => WorkItemCard = (card) => card,
  fromColumnKey?: string,
): WorkItemBoardResponse {
  let moved: WorkItemCard | null = null
  let fromKey: string | null = null
  for (const column of board.columns) {
    if (fromColumnKey !== undefined && column.key !== fromColumnKey) continue
    const found = column.items.find((item) => item.id === cardId)
    if (found) {
      moved = found
      fromKey = column.key
      break
    }
  }
  if (!moved || !board.columns.some((column) => column.key === toColumnKey)) return board
  const card = change(moved)
  return {
    ...board,
    columns: board.columns.map((column) => {
      let items = column.items
      let total = column.total
      if (column.key === fromKey) {
        items = items.filter((item) => item.id !== cardId)
        if (column.key !== toColumnKey) total = Math.max(0, total - 1)
      } else if (column.key !== toColumnKey && board.columnsBy === 'label' && !inLabelColumn(card, column.key) && items.some((item) => item.id === cardId)) {
        // A task leaves every other label column whose label it lost, such as when its labels are cleared.
        items = items.filter((item) => item.id !== cardId)
        total = Math.max(0, total - 1)
      }
      if (column.key === toColumnKey) {
        // On a label board the task may already be in the target column; it moves there, not twice.
        const already = column.key !== fromKey && items.some((item) => item.id === cardId)
        if (already) items = items.filter((item) => item.id !== cardId)
        const at = toIndex === null ? items.length : Math.max(0, Math.min(toIndex, items.length))
        items = [...items.slice(0, at), card, ...items.slice(at)]
        if (column.key !== fromKey && !already) total += 1
      }
      return items === column.items ? column : { ...column, items, total }
    }),
  }
}

function inLabelColumn(card: WorkItemCard, key: string): boolean {
  return key === 'none' ? card.labels.length === 0 : card.labels.some((label) => label.id === key)
}

/**
 * Shows a change in the cached board at once, sends it, and restores the board
 * as it was if sending fails. Either way the board is refetched afterwards.
 */
export async function runOptimisticMove<T>(
  queryClient: QueryClient,
  queryKey: QueryKey,
  update: (board: WorkItemBoardResponse) => WorkItemBoardResponse,
  send: () => Promise<T>,
): Promise<T> {
  await queryClient.cancelQueries({ queryKey })
  const previous = queryClient.getQueryData<WorkItemBoardResponse>(queryKey)
  if (previous) queryClient.setQueryData(queryKey, update(previous))
  try {
    return await send()
  } catch (error) {
    if (previous) queryClient.setQueryData(queryKey, previous)
    throw error
  }
}

/**
 * Runs the saves of one card one after another. A move dropped while the
 * card's last move is still saving would send the version the card had before
 * it, so the server would refuse it as a conflict and the card would go back.
 * Each save waits for the one before it and gets the version that one left,
 * or undefined when it failed (the card's own version still holds then).
 */
export function createSaveQueue() {
  const last = new Map<string, Promise<number | undefined>>()
  return <T>(id: string, save: (savedVersion: number | undefined) => Promise<{ result: T; version: number }>): Promise<T> => {
    const run = (last.get(id) ?? Promise.resolve(undefined)).then(save)
    const version = run.then(
      (saved) => saved.version,
      () => undefined,
    )
    last.set(id, version)
    void version.then(() => {
      if (last.get(id) === version) last.delete(id)
    })
    return run.then((saved) => saved.result)
  }
}

/**
 * Where a card lands among `items` on a board not grouped by status. Those
 * boards keep manual order (status, then rank), and a field change doesn't
 * touch it, so the card goes where that order puts it, not where it was dropped.
 */
export function orderedIndex(items: WorkItemCard[], card: WorkItemCard): number {
  return items.filter((item) => item.id !== card.id && item.boardOrder < card.boardOrder).length
}

/** What a task added in a board column starts with. */
export type NewTaskValues = {
  statusId?: string
  priority: WorkItemPriority
  assigneeActorId?: string
  milestoneId?: string
  typeId?: string
  labelIds?: string[]
}

/** The one value a filter names, when it names exactly one a task can have. */
function pinned(value: string | undefined): string | undefined {
  const values = value ? value.split(',') : []
  return values.length === 1 && values[0] !== 'none' && values[0] !== 'me' ? values[0] : undefined
}

/**
 * A task added in a column takes the column's value, and whatever the
 * filters pin to a single value, so it lands where it was typed.
 */
export function newTaskValues(columnsBy: BoardColumnsBy, columnKey: string, filters: FilterValues): NewTaskValues {
  const values: NewTaskValues = { priority: (pinned(filters.priority) as WorkItemPriority | undefined) ?? 'no_priority' }
  const status = pinned(filters.statusId)
  const assignee = pinned(filters.assigneeActorId)
  const milestone = pinned(filters.milestoneId)
  const type = pinned(filters.typeId)
  if (status) values.statusId = status
  if (assignee) values.assigneeActorId = assignee
  if (milestone) values.milestoneId = milestone
  if (type) values.typeId = type
  const labels = new Set(filters.labelIds && filters.labelIds !== 'none' ? filters.labelIds.split(',') : [])
  const anyLabel = pinned(filters.anyLabelIds)
  if (anyLabel) labels.add(anyLabel)
  const none = columnKey === 'none'
  switch (columnsBy) {
    case 'status':
      values.statusId = columnKey
      break
    case 'assignee':
      if (none) delete values.assigneeActorId
      else values.assigneeActorId = columnKey
      break
    case 'priority':
      values.priority = none ? 'no_priority' : (columnKey as WorkItemPriority)
      break
    case 'milestone':
      if (none) delete values.milestoneId
      else values.milestoneId = columnKey
      break
    case 'type':
      values.typeId = columnKey
      break
    case 'label':
      if (none) labels.clear()
      else labels.add(columnKey)
      break
  }
  if (labels.size) values.labelIds = [...labels]
  return values
}
