import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { Button, ErrorState, Icon, IconButton, actorKind, Avatar, countLabel } from '../ui'
import { ASSIGNEE_PAGE, listWorkItems, moveWorkItem, updateWorkItem } from './api'
import { BoardCard } from './BoardCard'
import { pickerShown, useCardFields } from './cardFields'
import { BoardSkeleton } from './BoardSkeleton'
import { ColumnComposer } from './ColumnComposer'
import { statusBoardColumns, tucksDone, withoutDone } from './doneModel'
import { applyMove, createSaveQueue, dropAnchor, fieldMove, laneOf, lanesFor, orderedIndex, PRIORITY_ORDER, runOptimisticMove } from './boardModel'
import { FieldPicker, type PickerField, type PickResult } from './FieldPicker'
import { allowedStatuses } from './pickerModel'
import { boardQuery, type ListState } from './listModel'
import type { Notice } from './notice'
import { priorityLabel } from './constants'
import { useAssignableActors } from './hooks'
import { useBoard, useInvalidateWorkItems, useMilestones } from './planningHooks'
import { SelectionBar } from './SelectionBar'
import { emptySelection, keepLoaded, MAX_BULK, pick, pickerCurrent, pickerTargets, selectAll, toggleOne, type Selection } from './selectionModel'
import { pageHasFocus, useOpenAsYouGo, useTaskShortcuts } from './shortcutKeys'
import { useDoneCount } from './useDoneWork'
import { TaskGlyph } from './TaskGlyph'
import { editErrorMessage, useApplyPick } from './taskEdits'
import type { BoardColumn, WorkItem, WorkItemBoardResponse, WorkItemCard, WorkItemPriority, WorkItemWorkflow } from './types'
import { useBoardDrag, type DropTarget } from './useBoardDrag'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import './board.css'

const PICKER_KEYS: Record<string, PickerField> = { s: 'status', a: 'assignee', l: 'labels', p: 'priority', m: 'milestone' }
const PAGE = 50

type Entry = { card: WorkItemCard; dragId: string } | { slot: true }

const dragIdFor = (columnKey: string, cardId: string) => `${columnKey}|${cardId}`

function splitDragId(dragId: string) {
  const at = dragId.lastIndexOf('|')
  return { columnKey: dragId.slice(0, at), cardId: dragId.slice(at + 1) }
}

/**
 * The board: a column per value of any field, optionally split into
 * swimlanes. Cards move by drag (mouse, or a long press on touch), or with the
 * keyboard: Space picks a card up, arrows move it, Space drops it. Moves show
 * at once and go back if the server refuses them. Like the list, cards can be
 * selected for bulk changes, browsed with J and K, and added in place.
 */
export function WorkItemBoard({
  disabled,
  notify,
  onClosePanel,
  onManageLabels,
  onOpenTask,
  onToggleDone,
  openTaskId,
  projectId,
  state,
  workflow,
}: {
  disabled: boolean
  notify: (notice: Notice) => void
  onClosePanel: () => void
  onManageLabels: () => void
  onOpenTask: (id: string) => void
  onToggleDone: () => void
  openTaskId: string | null
  projectId: string
  state: ListState
  workflow: WorkItemWorkflow
}) {
  // Done and canceled work sits behind the Show done toggle. A status board
  // loads its done columns and folds them away; other boards leave done tasks
  // out of the query until the toggle is open.
  const tucked = tucksDone(state.filters)
  const query = useMemo(
    () => (tucked && !state.showDone && state.columnsBy !== 'status' ? withoutDone(boardQuery(state)) : boardQuery(state)),
    [state, tucked],
  )
  const boardKey = queryKeys.workItemBoard(projectId, query)
  const board = useBoard(projectId, query, true)
  const fields = useCardFields(projectId)
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  const applyPick = useApplyPick()
  const scrollerRef = useRef<HTMLDivElement>(null)
  const ghostRef = useRef<HTMLDivElement>(null)
  const [focusedId, setFocusedId] = useState<string | null>(null)
  const [held, setHeld] = useState<{ dragId: string; target: DropTarget } | null>(null)
  const [announcement, setAnnouncement] = useState('')
  // A card's moves save in order, each with the version the last one left.
  const [queueSave] = useState(() => createSaveQueue())
  const [picker, setPicker] = useState<{ field: PickerField; anchor: HTMLElement; targets: WorkItemCard[] } | null>(null)
  const [loadingColumn, setLoadingColumn] = useState<string | null>(null)
  const [selectionState, setSelection] = useState<Selection>(emptySelection)
  const [composing, setComposing] = useState<string | null>(null)
  // The task just added from a column, until the board shows it (or can't).
  const added = useRef<{ id: string; key: string; columnKey: string; at: number } | null>(null)

  // Filters changed: the selection and the open add box belong to the old board.
  const queryKey = JSON.stringify(query)
  const [shownQuery, setShownQuery] = useState(queryKey)
  if (shownQuery !== queryKey) {
    setShownQuery(queryKey)
    setSelection(emptySelection)
    setComposing(null)
  }

  const columnsBy = board.data?.columnsBy ?? state.columnsBy
  const statusColumns = useMemo(
    () =>
      board.data && columnsBy === 'status'
        ? statusBoardColumns(board.data.columns, workflow, { tucked, showDone: state.showDone, hideDone: state.filters.includeDone === 'false' })
        : null,
    [board.data, columnsBy, state.filters.includeDone, state.showDone, tucked, workflow],
  )
  const columns = useMemo(() => statusColumns?.visible ?? board.data?.columns ?? [], [board.data, statusColumns])
  const otherDoneCount = useDoneCount(projectId, state.filters, workflow, tucked && state.columnsBy !== 'status')
  const doneCount = statusColumns ? statusColumns.doneTotal : otherDoneCount
  const milestones = useMilestones(projectId)
  const assignees = useAssignableActors(projectId, '')
  const knownLanes = useMemo(() => {
    switch (state.lanes) {
      case 'priority':
        return PRIORITY_ORDER.map((priority) => ({ key: priority, label: priorityLabel(priority) }))
      case 'type':
        return workflow.types.map((type) => ({ key: type.id, label: type.name }))
      case 'milestone':
        return [...(milestones.data ?? []).filter((milestone) => !milestone.archived).map((milestone) => ({ key: milestone.id, label: milestone.name })), { key: 'none', label: 'No milestone' }]
      case 'assignee':
        return [...(assignees.data ?? []).map((actor) => ({ key: actor.id, label: actor.displayName })), { key: 'none', label: 'Unassigned' }]
      default:
        return []
    }
  }, [assignees.data, milestones.data, state.lanes, workflow.types])
  const lanes = useMemo(() => lanesFor(columns, state.lanes, knownLanes), [columns, knownLanes, state.lanes])
  const assigneeKinds = useMemo(() => new Map((assignees.data ?? []).map((actor) => [actor.id, actor.kind])), [assignees.data])
  const canMove = !disabled
  // A lane for an archived milestone or a removed member holds its tasks, but nothing new can join it.
  // Only a complete, loaded list of lane values can rule a lane out. The
  // assignee search returns one page, so a full page may be missing active
  // members; then, as while loading, the server decides.
  const lanesKnown =
    state.lanes === 'milestone'
      ? milestones.data !== undefined
      : state.lanes === 'assignee'
        ? assignees.data !== undefined && assignees.data.length < ASSIGNEE_PAGE
        : true
  const laneDroppable = useCallback(
    (laneKey: string) => !lanesKnown || knownLanes.length === 0 || laneKey === 'none' || knownLanes.some((lane) => lane.key === laneKey),
    [knownLanes, lanesKnown],
  )

  const cellCards = useCallback(
    (column: BoardColumn, laneKey: string) =>
      column.items.filter((card) => state.lanes === 'none' || laneOf(card, state.lanes).key === laneKey),
    [state.lanes],
  )

  // Reading order, for J and K and Shift-click ranges: down each column, lane
  // by lane, then the next column.
  const readingOrder = useMemo(
    () => columns.flatMap((column) => lanes.flatMap((lane) => cellCards(column, lane.key).map((card) => ({ card, dragId: dragIdFor(column.key, card.id) })))),
    [cellCards, columns, lanes],
  )
  // A task shown in two label columns is one selection.
  const order = useMemo(() => [...new Set(readingOrder.map((entry) => entry.card.id))], [readingOrder])
  const cardsById = useMemo(() => new Map(readingOrder.map((entry) => [entry.card.id, entry.card])), [readingOrder])
  const selection = useMemo(() => keepLoaded(selectionState, new Set(cardsById.keys())), [cardsById, selectionState])
  const selectedCards = useMemo(() => [...selection.ids].flatMap((id) => cardsById.get(id) ?? []), [cardsById, selection])

  const performMove = useCallback(
    (dragId: string, target: DropTarget) => {
      const data = queryClient.getQueryData<WorkItemBoardResponse>(boardKey)
      if (!data) return
      const { columnKey: fromKey, cardId } = splitDragId(dragId)
      const from = data.columns.find((column) => column.key === fromKey)
      const to = data.columns.find((column) => column.key === target.columnKey)
      const card = from?.items.find((item) => item.id === cardId)
      if (!from || !to || !card) return
      const lane = laneOf(card, state.lanes)
      const laneChanged = state.lanes !== 'none' && lane.key !== target.laneKey
      const sameColumn = from.key === to.key
      const inCell = to.items.filter((item) => state.lanes === 'none' || laneOf(item, state.lanes).key === target.laneKey)
      const currentIndex = inCell.findIndex((item) => item.id === cardId)
      if (sameColumn && !laneChanged && (columnsBy !== 'status' || currentIndex === target.index)) return
      if (columnsBy === 'status' && !sameColumn && !allowedStatuses(workflow, card.status.id).some((status) => status.id === to.key)) {
        notify({ tone: 'info', title: `${card.key} can’t move to ${to.label}`, description: `This workflow doesn’t allow moving from ${card.status.name} to ${to.label}.` })
        return
      }
      if ((!sameColumn && !to.droppable) || (laneChanged && !laneDroppable(target.laneKey))) {
        notify({ tone: 'info', title: `${card.key} can’t move there`, description: 'That column is archived or no longer available, so tasks can’t move into it.' })
        return
      }
      // Anchor on the cards of the target cell; an empty cell goes after the column's last card.
      let anchor = dropAnchor(inCell, cardId, target.index)
      if (!anchor.beforeId && !anchor.afterId) {
        const others = to.items.filter((item) => item.id !== cardId)
        if (others.length) anchor = { afterId: others[others.length - 1].id }
      }
      const others = to.items.filter((item) => item.id !== cardId)
      const columnIndex = columnsBy !== 'status'
        ? orderedIndex(others, card)
        : anchor.beforeId
        ? others.findIndex((item) => item.id === anchor.beforeId)
        : anchor.afterId
          ? others.findIndex((item) => item.id === anchor.afterId) + 1
          : 0
      const labelIds = card.labels.map((label) => label.id)
      const columnChange = sameColumn ? null : fieldMove(columnsBy, from.key, to.key, to.label, to.status, labelIds)
      const laneLabel = lanes.find((entry) => entry.key === target.laneKey)?.label ?? ''
      const laneChange = laneChanged && state.lanes !== 'none' ? fieldMove(state.lanes, lane.key, target.laneKey, laneLabel, undefined, labelIds) : null
      const change = (item: WorkItemCard) => (laneChange ? laneChange.apply(columnChange ? columnChange.apply(item) : item) : columnChange ? columnChange.apply(item) : item)

      // A status move and a lane change are two saves. When the move saves but
      // the lane change doesn't, the move stands and the error comes back as
      // `laneError`, so the person is told what did and didn't happen.
      const send = () =>
        queueSave(card.id, async (savedVersion): Promise<{ result: { laneError?: unknown }; version: number }> => {
          let version = Math.max(card.version, savedVersion ?? 0)
          if (columnsBy === 'status') {
            const moved = await moveWorkItem({
              projectId,
              workItemId: card.id,
              expectedVersion: version,
              targetStatusId: to.key,
              ...(anchor.beforeId ? { beforeId: anchor.beforeId } : anchor.afterId ? { afterId: anchor.afterId } : { targetPosition: 1 }),
              idempotencyKey: crypto.randomUUID(),
            })
            version = moved.version
            if (!laneChange) return { result: {}, version }
            try {
              const updated = await updateWorkItem({ projectId, workItemId: card.id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: version, ...laneChange.patch } })
              version = updated.version
            } catch (laneError) {
              return { result: { laneError }, version }
            }
            return { result: {}, version }
          }
          const patch = { ...(columnChange ? columnChange.patch : {}), ...(laneChange?.patch ?? {}) }
          if (Object.keys(patch).length === 0) return { result: {}, version }
          const updated = await updateWorkItem({ projectId, workItemId: card.id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: version, ...patch } })
          return { result: {}, version: updated.version }
        })

      setAnnouncement(`Moved ${card.key} to ${to.label}${laneChange ? `, ${laneLabel}` : ''}.`)
      runOptimisticMove(
        queryClient,
        boardKey,
        (current) => applyMove(current, cardId, to.key, columnIndex, change, from.key),
        send,
      )
        .then(({ laneError }) => {
          if (laneError === undefined) return
          // The move stands; only the lane goes back, so the cache matches the server until the refetch lands.
          const undoLane = state.lanes === 'none' ? null : fieldMove(state.lanes, target.laneKey, lane.key, lane.label, undefined, labelIds)
          if (undoLane) {
            queryClient.setQueryData<WorkItemBoardResponse>(boardKey, (current) =>
              current
                ? {
                    ...current,
                    columns: current.columns.map((column) =>
                      column.items.some((item) => item.id === cardId)
                        ? { ...column, items: column.items.map((item) => (item.id === cardId ? undoLane.apply(item) : item)) }
                        : column,
                    ),
                  }
                : current,
            )
          }
          setAnnouncement(`Moved ${card.key} to ${to.label}, but its lane didn’t change.`)
          notify({ tone: 'danger', title: `${card.key} moved to ${to.label} but stayed in ${lane.label}`, description: editErrorMessage(laneError, 'The lane change') })
        })
        .catch((error) => {
          setAnnouncement(`${card.key} went back: the move didn’t save.`)
          notify({ tone: 'danger', title: `${card.key} went back to ${from.label}`, description: editErrorMessage(error, 'The move') })
        })
        .finally(() => void invalidate(projectId))
    },
    [boardKey, columnsBy, invalidate, laneDroppable, lanes, notify, projectId, queryClient, queueSave, state.lanes, workflow],
  )

  const drag = useBoardDrag({ enabled: canMove, ghostRef, onDrop: performMove, scrollerRef })
  const activeDrag = useMemo(
    () => (drag.drag ? { dragId: drag.drag.cardId, target: drag.drag.target, keyboard: false } : held ? { ...held, keyboard: true } : null),
    [drag.drag, held],
  )
  const draggedCard = useMemo(() => {
    if (!activeDrag) return null
    const { columnKey, cardId } = splitDragId(activeDrag.dragId)
    return columns.find((column) => column.key === columnKey)?.items.find((item) => item.id === cardId) ?? null
  }, [activeDrag, columns])

  /** The cards of a cell as drawn, with the held card or a drop slot where it would land. */
  const entriesFor = (column: BoardColumn, laneKey: string): Entry[] => {
    const cards: Entry[] = cellCards(column, laneKey).map((card) => ({ card, dragId: dragIdFor(column.key, card.id) }))
    if (!activeDrag) return cards
    const without = cards.filter((entry) => !('card' in entry) || entry.dragId !== activeDrag.dragId)
    const target = activeDrag.target
    const { columnKey: fromKey } = splitDragId(activeDrag.dragId)
    const fromLane = draggedCard && state.lanes !== 'none' ? laneOf(draggedCard, state.lanes).key : laneKey
    const blocked = (column.key !== fromKey && !column.droppable) || (laneKey !== fromLane && !laneDroppable(laneKey))
    if (!target || target.columnKey !== column.key || target.laneKey !== laneKey || blocked) {
      // Keep the held card visible in place while the pointer is outside every cell.
      return activeDrag.keyboard || target ? without : cards
    }
    // Only a status board keeps the spot a card is dropped on; elsewhere the slot shows where it will land.
    const ordered = columnsBy !== 'status' && draggedCard
    const at = ordered
      ? orderedIndex(without.flatMap((entry) => ('card' in entry ? [entry.card] : [])), draggedCard)
      : Math.max(0, Math.min(target.index, without.length))
    const insert: Entry = activeDrag.keyboard && draggedCard ? { card: draggedCard, dragId: activeDrag.dragId } : { slot: true }
    return [...without.slice(0, at), insert, ...without.slice(at)]
  }

  const loadMore = async (column: BoardColumn) => {
    if (!column.nextCursor) return
    setLoadingColumn(column.key)
    try {
      const page = await listWorkItems(projectId, { ...column.filter, sort: 'manual', cursor: column.nextCursor, limit: PAGE })
      queryClient.setQueryData<WorkItemBoardResponse>(boardKey, (current) =>
        current
          ? {
              ...current,
              columns: current.columns.map((entry) =>
                entry.key !== column.key
                  ? entry
                  : {
                      ...entry,
                      items: [...entry.items, ...page.items.filter((item) => !entry.items.some((existing) => existing.id === item.id))],
                      nextCursor: page.nextCursor,
                      hasMore: Boolean(page.nextCursor),
                    },
              ),
            }
          : current,
      )
    } catch (error) {
      notify({ tone: 'danger', title: `More tasks in ${column.label || 'this column'} didn’t load`, description: editErrorMessage(error, 'Loading more') })
    } finally {
      setLoadingColumn(null)
    }
  }

  // Keyboard: arrows between cards; Space picks a card up and drops it.
  const navigate = (dragId: string, key: string): DropTarget | null => {
    const { columnKey, cardId } = splitDragId(dragId)
    const columnIndex = columns.findIndex((column) => column.key === columnKey)
    const column = columns[columnIndex]
    const card = column?.items.find((item) => item.id === cardId)
    if (!column || !card) return null
    const laneKey = state.lanes === 'none' ? 'all' : laneOf(card, state.lanes).key
    const cell = cellCards(column, laneKey)
    const index = cell.findIndex((item) => item.id === cardId)
    if (key === 'ArrowUp' || key === 'ArrowDown') return { columnKey, laneKey, index: index + (key === 'ArrowDown' ? 1 : -1) }
    const step = key === 'ArrowRight' ? 1 : -1
    for (let next = columnIndex + step; next >= 0 && next < columns.length; next += step) {
      const nextCell = cellCards(columns[next], laneKey)
      if (nextCell.length) return { columnKey: columns[next].key, laneKey, index: Math.min(index, nextCell.length - 1) }
    }
    return null
  }

  useEffect(() => {
    if (!held) return
    const element = scrollerRef.current?.querySelector<HTMLElement>(`[data-drag-id="${CSS.escape(held.dragId)}"]`)
    if (element && document.activeElement !== element) element.focus({ preventScroll: false })
  })

  const focusCard = (dragId: string) => {
    setFocusedId(dragId)
    window.requestAnimationFrame(() => scrollerRef.current?.querySelector<HTMLElement>(`[data-drag-id="${CSS.escape(dragId)}"]`)?.focus())
  }

  // The side panel follows the keyboard from card to card; a click or Enter opens at once.
  const following = useOpenAsYouGo((id) => {
    if (id !== openTaskId) onOpenTask(id)
  })
  const browseTo = (dragId: string) => {
    focusCard(dragId)
    following.follow(splitDragId(dragId).cardId)
  }

  const moveHeld = (key: string) => {
    if (!held) return
    const { target } = held
    const columnIndex = columns.findIndex((column) => column.key === target.columnKey)
    const column = columns[columnIndex]
    if (!column) return
    const size = cellCards(column, target.laneKey).filter((item) => dragIdFor(column.key, item.id) !== held.dragId).length
    let next = target
    if (key === 'ArrowUp' || key === 'ArrowDown') {
      const index = target.index + (key === 'ArrowDown' ? 1 : -1)
      const laneIndex = lanes.findIndex((lane) => lane.key === target.laneKey)
      if (index < 0 && laneIndex > 0) {
        const lane = lanes[laneIndex - 1]
        next = { ...target, laneKey: lane.key, index: cellCards(column, lane.key).length }
      } else if (index > size && laneIndex < lanes.length - 1) {
        next = { ...target, laneKey: lanes[laneIndex + 1].key, index: 0 }
      } else next = { ...target, index: Math.max(0, Math.min(size, index)) }
    } else {
      const other = columns[columnIndex + (key === 'ArrowRight' ? 1 : -1)]
      if (!other) return
      next = { columnKey: other.key, laneKey: target.laneKey, index: Math.min(target.index, cellCards(other, target.laneKey).length) }
    }
    const label = columns.find((entry) => entry.key === next.columnKey)?.label ?? ''
    const laneLabel = lanes.find((lane) => lane.key === next.laneKey)?.label
    setHeld({ ...held, target: next })
    setAnnouncement(`${label}${laneLabel ? `, ${laneLabel}` : ''}, position ${next.index + 1}.`)
  }

  const onCardKeyDown = (event: KeyboardEvent<HTMLElement>, card: WorkItemCard, dragId: string) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const arrows = ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight']
    if (held) return
    if (arrows.includes(event.key)) {
      event.preventDefault()
      const target = navigate(dragId, event.key)
      if (!target) return
      const column = columns.find((entry) => entry.key === target.columnKey)
      const cell = column ? cellCards(column, target.laneKey) : []
      const next = cell[Math.max(0, Math.min(target.index, cell.length - 1))]
      if (column && next) browseTo(dragIdFor(column.key, next.id))
      return
    }
    if (event.key === 'Home' || event.key === 'End') {
      // The ends of the card's own column (and lane).
      event.preventDefault()
      const { columnKey } = splitDragId(dragId)
      const column = columns.find((entry) => entry.key === columnKey)
      const cell = column ? cellCards(column, state.lanes === 'none' ? 'all' : laneOf(card, state.lanes).key) : []
      const next = event.key === 'Home' ? cell[0] : cell[cell.length - 1]
      if (column && next) browseTo(dragIdFor(column.key, next.id))
      return
    }
    if (event.key === ' ' && canMove && card.capabilities.edit && !card.archivedAt) {
      event.preventDefault()
      // React commits the pick-up while this event is still bubbling, so the
      // window listener for a held card would see this same Space and drop it.
      event.stopPropagation()
      const { columnKey } = splitDragId(dragId)
      const column = columns.find((entry) => entry.key === columnKey)
      const laneKey = state.lanes === 'none' ? 'all' : laneOf(card, state.lanes).key
      const index = column ? cellCards(column, laneKey).findIndex((item) => item.id === card.id) : 0
      setHeld({ dragId, target: { columnKey, laneKey, index } })
      setAnnouncement(`Picked up ${card.key}. Arrow keys move it, Space drops it, Escape puts it back.`)
      return
    }
    if (event.key === 'Enter') {
      event.preventDefault()
      following.cancel()
      onOpenTask(card.id)
      return
    }
    const field = PICKER_KEYS[event.key.toLowerCase()]
    if (field && !event.shiftKey && pickerShown(field, fields) && openPicker(field, event.currentTarget, card)) event.preventDefault()
  }

  // While a card is held its element moves between columns, so keys are read from the window.
  useEffect(() => {
    if (!held) return undefined
    const current = held
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return
      const { cardId } = splitDragId(current.dragId)
      const card = columns.flatMap((column) => column.items).find((item) => item.id === cardId)
      if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
        event.preventDefault()
        moveHeld(event.key)
      } else if (event.key === ' ' || event.key === 'Enter') {
        event.preventDefault()
        performMove(current.dragId, current.target)
        setHeld(null)
        focusCard(dragIdFor(current.target.columnKey, cardId))
      } else if (event.key === 'Escape') {
        event.preventDefault()
        setHeld(null)
        setAnnouncement(`${card?.key ?? 'The card'} put back.`)
        focusCard(current.dragId)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  /** Opens a picker for the card, or for the selection it's part of. False when there's nothing it could change. */
  const openPicker = (field: PickerField, anchor: HTMLElement, card?: WorkItemCard) => {
    if (!canMove) return false
    const targets = pickerTargets(selectedCards, card)
    if (!targets.length) return false
    if (targets.length > MAX_BULK) {
      notify({ tone: 'info', title: `Bulk changes take up to ${countLabel(MAX_BULK, 'task')} at a time`, description: 'Select fewer and try again.' })
      return true
    }
    setPicker({ field, anchor, targets })
    return true
  }

  const onSelect = (card: WorkItemCard, range: boolean) => setSelection(pick(selection, card.id, order, { shift: range, toggle: !range }))

  // The card J, K, X and the picker letters act on: the focused one, or the last one focused.
  const current = focusedId ? (readingOrder.find((entry) => entry.dragId === focusedId) ?? null) : null

  useTaskShortcuts(!picker && !held, (event) => {
    const key = event.key.toLowerCase()
    if (canMove && (event.metaKey || event.ctrlKey) && key === 'a' && scrollerRef.current?.contains(document.activeElement)) {
      setSelection(selectAll(order))
      if (order.length > MAX_BULK) notify({ tone: 'info', title: `Selected the first ${countLabel(MAX_BULK, 'task')}`, description: `Bulk changes take up to ${MAX_BULK} at a time.` })
      return true
    }
    if (event.metaKey || event.ctrlKey) return false
    if (key === 'escape') {
      following.cancel()
      if (openTaskId) onClosePanel()
      else if (selection.ids.size) setSelection(emptySelection)
      else return false
      return true
    }
    // J and K walk down each column from anywhere but a field. With nothing
    // focused (or the board itself, after a click between cards), an arrow
    // starts on the card you were on, or the first one.
    if (key === 'j' || key === 'k' || (key.startsWith('arrow') && (pageHasFocus() || document.activeElement === scrollerRef.current))) {
      if (!readingOrder.length) return false
      const index = current ? readingOrder.indexOf(current) : -1
      const step = key === 'j' ? 1 : key === 'k' ? -1 : 0
      const next = readingOrder[index < 0 ? 0 : Math.max(0, Math.min(readingOrder.length - 1, index + step))]
      browseTo(next.dragId)
      return true
    }
    if (key === 'x' && current && canMove) {
      setSelection(toggleOne(selection, current.card.id))
      return true
    }
    const field = PICKER_KEYS[key]
    if (field && !event.shiftKey && pickerShown(field, fields)) {
      const element = current ? scrollerRef.current?.querySelector<HTMLElement>(`[data-drag-id="${CSS.escape(current.dragId)}"]`) : null
      const anchor = element ?? document.querySelector<HTMLElement>('.aw-bulkbar') ?? scrollerRef.current
      return anchor ? openPicker(field, anchor, current?.card) : false
    }
    return false
  })

  const onPickResult = (result: PickResult) => {
    if (!picker) return
    const targets = picker.targets
    applyPick.mutate(
      { projectId, result, targets: targets.map((card) => ({ id: card.id, projectId: card.projectId, version: card.version, key: card.key })) },
      {
        onSuccess: () => {
          if (targets.length > 1) notify({ tone: 'success', title: `Changed ${countLabel(targets.length, 'task')}` })
        },
        onError: (error) =>
          notify({
            tone: 'danger',
            title: targets.length === 1 ? `${targets[0].key} didn’t change` : `${countLabel(targets.length, 'task')} didn’t change`,
            description: editErrorMessage(error),
          }),
      },
    )
  }

  const canAdd = (column: BoardColumn) => canMove && column.droppable

  const closeComposer = (columnKey: string, refocus: boolean) => {
    setComposing(null)
    if (refocus) window.requestAnimationFrame(() => scrollerRef.current?.querySelector<HTMLElement>(`[data-add-column="${CSS.escape(columnKey)}"]`)?.focus())
  }

  const onCreated = (item: WorkItem, column: BoardColumn) => {
    setAnnouncement(`Added ${item.key} to ${column.label || 'None'}.`)
    added.current = { id: item.id, key: item.key, columnKey: column.key, at: Date.now() }
  }

  // Once the board has reloaded with a task just added, bring it into view,
  // or say why it isn't there.
  const { dataUpdatedAt, isFetching } = board
  useEffect(() => {
    const task = added.current
    if (!task || isFetching || dataUpdatedAt < task.at) return
    added.current = null
    const element = scrollerRef.current?.querySelector<HTMLElement>(`[data-drag-id$="|${CSS.escape(task.id)}"]`)
    if (element) {
      element.scrollIntoView({ block: 'nearest', inline: 'nearest' })
      element.classList.add('is-fresh')
      return
    }
    const column = columns.find((entry) => entry.key === task.columnKey)
    notify({
      tone: 'info',
      title: `Added ${task.key}`,
      description: column?.hasMore ? `It’s further down ${column.label || 'the column'}.` : 'The filters on this board hide it.',
      action: { label: 'Open', run: () => onOpenTask(task.id) },
    })
  }, [columns, dataUpdatedAt, isFetching, notify, onOpenTask])

  if (board.isPending) return <BoardSkeleton />
  if (board.isError && !board.data) return <ErrorState error={board.error} onRetry={() => void board.refetch()} title="The board didn’t load" />

  const firstCard = columns.flatMap((column) => column.items.map((card) => dragIdFor(column.key, card.id)))[0] ?? null
  const tabStop = focusedId ?? firstCard
  const columnCount = Math.max(1, columns.length)
  const doneToggle =
    tucked && (doneCount > 0 || state.showDone) ? (
      <div className="aw-tboard__done">
        <Button aria-expanded={state.showDone} className="aw-tboard__donebtn" icon="circle-check" onClick={onToggleDone} size="sm" variant="ghost">
          {state.showDone ? 'Hide done' : 'Show done'} ({doneCount.toLocaleString()})
        </Button>
      </div>
    ) : null

  return (
    <div className="aw-tboard-wrap">
      <p className="visually-hidden" id="board-drag-help">
        Press Space to pick up, arrow keys to move, Space to drop. Enter opens it, X selects it.
      </p>
      <p aria-live="assertive" className="visually-hidden">
        {announcement}
      </p>
      <div
        aria-label="Task board"
        className={['aw-tboard', drag.drag ? 'is-dragging' : '', selection.ids.size ? 'has-selection' : ''].filter(Boolean).join(' ')}
        ref={scrollerRef}
        role="region"
        style={{ '--aw-board-columns': columnCount } as CSSProperties}
        tabIndex={-1}
      >
        <div className="aw-tboard__body">
        <div className={state.lanes === 'none' ? 'aw-tboard__grid aw-tboard__grid--fill' : 'aw-tboard__grid'}>
          {columns.map((column) => (
            <header className="aw-tboard__head" key={column.key}>
              <div className="aw-tboard__headrow">
                <ColumnMark assigneeKinds={assigneeKinds} column={column} columnsBy={columnsBy} />
                <h2 className="aw-tboard__title">{column.label || 'None'}</h2>
                <span className="aw-tboard__count">{column.total}</span>
                {canAdd(column) ? (
                  <IconButton
                    aria-expanded={composing === column.key}
                    className="aw-tboard__add"
                    data-add-column={column.key}
                    icon="plus"
                    label={`Add a task to ${column.label || 'None'}`}
                    onClick={() => setComposing(composing === column.key ? null : column.key)}
                    size="sm"
                  />
                ) : null}
              </div>
              {composing === column.key ? (
                <ColumnComposer
                  column={column}
                  columnsBy={columnsBy}
                  filters={state.filters}
                  notify={notify}
                  onClose={(refocus) => closeComposer(column.key, refocus)}
                  onCreated={(item) => onCreated(item, column)}
                  projectId={projectId}
                />
              ) : null}
            </header>
          ))}
          {lanes.map((lane) => (
            <LaneRows key={lane.key} label={state.lanes === 'none' ? null : lane.label || 'None'}>
              {columns.map((column) => {
                const entries = entriesFor(column, lane.key)
                const isTarget = activeDrag?.target?.columnKey === column.key && activeDrag.target.laneKey === lane.key
                return (
                  <section
                    aria-label={`${column.label || 'None'}${state.lanes === 'none' ? '' : `, ${lane.label || 'None'}`}`}
                    className={isTarget ? 'aw-tboard__cell is-target' : 'aw-tboard__cell'}
                    data-column={column.key}
                    data-drop-cell=""
                    data-lane={lane.key}
                    key={column.key}
                  >
                    <ol className="aw-tboard__cards">
                      {entries.map((entry, index) =>
                        'slot' in entry ? (
                          <li aria-hidden="true" className="aw-tcard-slot" key={`slot-${index}`} />
                        ) : (
                          <BoardCard
                            card={entry.card}
                            dragId={entry.dragId}
                            draggable={canMove && entry.card.capabilities.edit && !entry.card.archivedAt}
                            fields={fields}
                            focused={entry.dragId === tabStop}
                            held={held?.dragId === entry.dragId}
                            key={entry.dragId}
                            onFocus={setFocusedId}
                            onKeyDown={onCardKeyDown}
                            onOpen={(card) => {
                              if (drag.justDragged()) return
                              following.cancel()
                              onOpenTask(card.id)
                            }}
                            onPick={(card, field, anchor) => openPicker(field, anchor, card)}
                            onPointerDown={drag.onPointerDown}
                            onSelect={canMove ? onSelect : undefined}
                            open={entry.card.id === openTaskId}
                            selected={selection.ids.has(entry.card.id)}
                            showStatus={columnsBy !== 'status'}
                          />
                        ),
                      )}
                    </ol>
                    {entries.length === 0 && state.lanes === 'none' ? <p className="aw-tboard__empty">No tasks</p> : null}
                  </section>
                )
              })}
            </LaneRows>
          ))}
          {columns.some((column) => column.hasMore)
            ? columns.map((column) => (
                <div className="aw-tboard__more" key={column.key}>
                  {column.hasMore ? (
                    <Button busy={loadingColumn === column.key} onClick={() => void loadMore(column)} size="sm" variant="ghost">
                      Load more ({Math.max(0, column.total - column.items.length)})
                    </Button>
                  ) : null}
                </div>
              ))
            : null}
        </div>
        {doneToggle}
        </div>
      </div>
      {drag.drag && draggedCard ? (
        <div aria-hidden="true" className="aw-tcard-ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          <span className="aw-tcard__key">{draggedCard.key}</span>
          <span className="aw-tcard__title">{draggedCard.title}</span>
        </div>
      ) : null}
      {canMove ? (
        <SelectionBar
          busy={applyPick.isPending}
          notify={notify}
          onClear={() => setSelection(emptySelection)}
          onPick={(field, anchor) => openPicker(field, anchor)}
          projectId={projectId}
          selected={selectedCards}
        />
      ) : null}
      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={pickerCurrent(picker.targets)}
          field={picker.field}
          onClose={() => setPicker(null)}
          onManageLabels={onManageLabels}
          onPick={onPickResult}
          projectId={projectId}
          workflow={workflow}
        />
      ) : null}
    </div>
  )
}

function LaneRows({ children, label }: { children: ReactNode; label: string | null }) {
  return (
    <>
      {label !== null ? (
        <h3 className="aw-tboard__lane">
          <TaskGlyph name="lanes" />
          {label}
        </h3>
      ) : null}
      {children}
    </>
  )
}

function ColumnMark({ assigneeKinds, column, columnsBy }: { assigneeKinds: Map<string, string>; column: BoardColumn; columnsBy: string }) {
  if (column.status) return <StatusIcon category={column.status.semanticCategory} />
  if (columnsBy === 'priority') return <PriorityIcon decorative priority={(column.key === 'none' ? 'no_priority' : column.key) as WorkItemPriority} />
  if (columnsBy === 'assignee') {
    if (column.key === 'none') return <Icon name="unassigned" size={16} />
    // Agents get square avatars: the kind comes from a card in the column, or the member list.
    const kind = column.items.find((card) => card.assignee?.actorId === column.key)?.assignee?.kind ?? assigneeKinds.get(column.key) ?? 'human'
    return <Avatar kind={actorKind(kind)} name={column.label} seed={column.key} size={18} />
  }
  if (columnsBy === 'milestone') return <TaskGlyph name="milestone" />
  if (columnsBy === 'label') return <TaskGlyph name="tag" />
  return null
}
