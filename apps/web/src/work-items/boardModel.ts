import type { QueryClient, QueryKey } from '@tanstack/react-query'

import { priorityLabel } from './constants'
import type { LaneBy } from './listModel'
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
 * Where a card lands among `items` on a board not grouped by status. Those
 * boards keep manual order (status, then rank), and a field change doesn't
 * touch it, so the card goes where that order puts it, not where it was dropped.
 */
export function orderedIndex(items: WorkItemCard[], card: WorkItemCard): number {
  return items.filter((item) => item.id !== card.id && item.boardOrder < card.boardOrder).length
}
