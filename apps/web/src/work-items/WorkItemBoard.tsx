import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { Button, ErrorState, Icon, MessageSkeleton, actorKind, Avatar } from '../ui'
import { listWorkItems, moveWorkItem, updateWorkItem } from './api'
import { BoardCard } from './BoardCard'
import { applyMove, dropAnchor, fieldMove, laneOf, lanesFor, orderedIndex, PRIORITY_ORDER, runOptimisticMove } from './boardModel'
import { FieldPicker, type PickerField, type PickResult } from './FieldPicker'
import { allowedStatuses } from './pickerModel'
import { boardQuery, type ListState } from './listModel'
import type { Notice } from './notice'
import { priorityLabel } from './constants'
import { useAssignableActors } from './hooks'
import { useBoard, useInvalidateWorkItems, useMilestones } from './planningHooks'
import { useTaskShortcuts } from './shortcutKeys'
import { TaskGlyph } from './TaskGlyph'
import { editErrorMessage, useApplyPick } from './taskEdits'
import type { BoardColumn, WorkItemBoardResponse, WorkItemCard, WorkItemPriority, WorkItemWorkflow } from './types'
import { useBoardDrag, type DropTarget } from './useBoardDrag'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

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
 * at once and go back if the server refuses them.
 */
export function WorkItemBoard({
  disabled,
  notify,
  onClosePanel,
  onManageLabels,
  onOpenTask,
  openTaskId,
  projectId,
  state,
  workflow,
}: {
  disabled: boolean
  notify: (notice: Notice | null) => void
  onClosePanel: () => void
  onManageLabels: () => void
  onOpenTask: (id: string) => void
  openTaskId: string | null
  projectId: string
  state: ListState
  workflow: WorkItemWorkflow
}) {
  const query = useMemo(() => boardQuery(state), [state])
  const boardKey = queryKeys.workItemBoard(projectId, query)
  const board = useBoard(projectId, query, true)
  const queryClient = useQueryClient()
  const invalidate = useInvalidateWorkItems()
  const applyPick = useApplyPick()
  const scrollerRef = useRef<HTMLDivElement>(null)
  const ghostRef = useRef<HTMLDivElement>(null)
  const [focusedId, setFocusedId] = useState<string | null>(null)
  const [held, setHeld] = useState<{ dragId: string; target: DropTarget } | null>(null)
  const [announcement, setAnnouncement] = useState('')
  const [picker, setPicker] = useState<{ field: PickerField; anchor: HTMLElement; card: WorkItemCard } | null>(null)
  const [loadingColumn, setLoadingColumn] = useState<string | null>(null)

  const columns = useMemo(() => board.data?.columns ?? [], [board.data])
  const columnsBy = board.data?.columnsBy ?? state.columnsBy
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
  const canMove = !disabled
  // A lane for an archived milestone or a removed member holds its tasks, but nothing new can join it.
  const laneDroppable = useCallback(
    (laneKey: string) => knownLanes.length === 0 || laneKey === 'none' || knownLanes.some((lane) => lane.key === laneKey),
    [knownLanes],
  )

  const cellCards = useCallback(
    (column: BoardColumn, laneKey: string) =>
      column.items.filter((card) => state.lanes === 'none' || laneOf(card, state.lanes).key === laneKey),
    [state.lanes],
  )

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
        notify({ tone: 'warning', message: `This workflow doesn’t allow moving from ${card.status.name} to ${to.label}.` })
        return
      }
      if ((!sameColumn && !to.droppable) || (laneChanged && !laneDroppable(target.laneKey))) {
        notify({ tone: 'warning', message: 'That column is archived or no longer available, so tasks can’t be moved into it.' })
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
      const send = async (): Promise<{ laneError?: unknown }> => {
        let version = card.version
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
          if (!laneChange) return {}
          try {
            await updateWorkItem({ projectId, workItemId: card.id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: version, ...laneChange.patch } })
          } catch (laneError) {
            return { laneError }
          }
          return {}
        }
        const patch = { ...(columnChange ? columnChange.patch : {}), ...(laneChange?.patch ?? {}) }
        if (Object.keys(patch).length === 0) return {}
        await updateWorkItem({ projectId, workItemId: card.id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: version, ...patch } })
        return {}
      }

      setAnnouncement(`Moved ${card.key} to ${to.label}${laneChange ? `, ${laneLabel}` : ''}.`)
      runOptimisticMove(
        queryClient,
        boardKey,
        (current) => applyMove(current, cardId, to.key, columnIndex, change, from.key),
        send,
      )
        .then(({ laneError }) => {
          if (laneError === undefined) return
          setAnnouncement(`Moved ${card.key} to ${to.label}, but its lane didn’t change.`)
          notify({ tone: 'warning', message: `${card.key} moved to ${to.label}, but it stayed in ${lane.label}. ${editErrorMessage(laneError, 'The lane change')}` })
        })
        .catch((error) => {
          setAnnouncement(`${card.key} went back: the move didn’t save.`)
          notify({ tone: 'danger', message: `${card.key} went back to ${from.label}. ${editErrorMessage(error, 'The move')}` })
        })
        .finally(() => void invalidate(projectId))
    },
    [boardKey, columnsBy, invalidate, laneDroppable, lanes, notify, projectId, queryClient, state.lanes, workflow],
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
      notify({ tone: 'danger', message: editErrorMessage(error, 'Loading more') })
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
      if (column && next) focusCard(dragIdFor(column.key, next.id))
      return
    }
    if (event.key === ' ' && canMove && card.capabilities.edit && !card.archivedAt) {
      event.preventDefault()
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
      onOpenTask(card.id)
      return
    }
    const field = PICKER_KEYS[event.key.toLowerCase()]
    if (field && canMove && card.capabilities.edit && !card.archivedAt) {
      event.preventDefault()
      setPicker({ field, anchor: event.currentTarget, card })
    }
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

  useTaskShortcuts(!picker && !held, (event) => {
    if (event.key === 'Escape' && openTaskId) {
      onClosePanel()
      return true
    }
    return false
  })

  const onPickResult = (result: PickResult) => {
    if (!picker) return
    const card = picker.card
    applyPick.mutate(
      { projectId, result, targets: [{ id: card.id, projectId: card.projectId, version: card.version, key: card.key }] },
      { onError: (error) => notify({ tone: 'danger', message: editErrorMessage(error) }) },
    )
  }

  if (board.isPending) return <MessageSkeleton label="Loading the board" rows={3} />
  if (board.isError && !board.data) {
    return (
      <ErrorState action={<Button onClick={() => void board.refetch()}>Try again</Button>} title="The board didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }

  const firstCard = columns.flatMap((column) => column.items.map((card) => dragIdFor(column.key, card.id)))[0] ?? null
  const tabStop = focusedId ?? firstCard
  const columnCount = Math.max(1, columns.length)

  return (
    <div className="aw-tboard-wrap">
      <p className="visually-hidden" id="board-drag-help">
        Press Space to pick up, arrow keys to move, Space to drop. Enter opens it.
      </p>
      <p aria-live="assertive" className="visually-hidden">
        {announcement}
      </p>
      <div
        aria-label="Work item board"
        className={drag.drag ? 'aw-tboard is-dragging' : 'aw-tboard'}
        ref={scrollerRef}
        role="region"
        style={{ '--aw-board-columns': columnCount } as CSSProperties}
        tabIndex={-1}
      >
        <div className="aw-tboard__grid">
          {columns.map((column) => (
            <header className="aw-tboard__head" key={column.key}>
              <ColumnMark column={column} columnsBy={columnsBy} />
              <h2 className="aw-tboard__title">{column.label || 'None'}</h2>
              <span className="aw-tboard__count">{column.total}</span>
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
                            focused={entry.dragId === tabStop}
                            held={held?.dragId === entry.dragId}
                            key={entry.dragId}
                            onFocus={setFocusedId}
                            onKeyDown={onCardKeyDown}
                            onOpen={(card, event: MouseEvent<HTMLElement>) => {
                              if (drag.justDragged() || event.metaKey || event.ctrlKey) return
                              onOpenTask(card.id)
                            }}
                            onPick={(card, field, anchor) => setPicker({ field, anchor, card })}
                            onPointerDown={drag.onPointerDown}
                            open={entry.card.id === openTaskId}
                            showStatus={columnsBy !== 'status'}
                          />
                        ),
                      )}
                    </ol>
                    {entries.length === 0 && state.lanes === 'none' ? <p className="aw-tboard__empty">Nothing here</p> : null}
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
      </div>
      {drag.drag && draggedCard ? (
        <div aria-hidden="true" className="aw-tcard-ghost" ref={ghostRef} style={{ width: drag.drag.width }}>
          <span className="aw-tcard__key">{draggedCard.key}</span>
          <span className="aw-tcard__title">{draggedCard.title}</span>
        </div>
      ) : null}
      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={{
            statusId: picker.card.status.id,
            priority: picker.card.priority,
            assigneeId: picker.card.assignee?.actorId ?? null,
            assignee: picker.card.assignee
              ? { id: picker.card.assignee.actorId, displayName: picker.card.assignee.displayName, handle: picker.card.assignee.handle, kind: picker.card.assignee.kind }
              : null,
            labelIds: picker.card.labels.map((label) => label.id),
            milestoneId: picker.card.milestone?.id ?? null,
            typeId: picker.card.itemType.id,
          }}
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

function ColumnMark({ column, columnsBy }: { column: BoardColumn; columnsBy: string }) {
  if (column.status) return <StatusIcon category={column.status.semanticCategory} />
  if (columnsBy === 'priority') return <PriorityIcon decorative priority={(column.key === 'none' ? 'no_priority' : column.key) as WorkItemPriority} />
  if (columnsBy === 'assignee') {
    return column.key === 'none' ? <Icon name="unassigned" size={16} /> : <Avatar kind={actorKind('human')} name={column.label} seed={column.key} size={18} />
  }
  if (columnsBy === 'milestone') return <TaskGlyph name="milestone" />
  if (columnsBy === 'label') return <TaskGlyph name="tag" />
  return null
}
