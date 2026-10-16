import { memo, type KeyboardEvent, type MouseEvent, type PointerEvent } from 'react'

import { AssigneeMark, DueDate, TaskBadges } from './ListCells'
import type { PickerField } from './FieldPicker'
import { workItemHref } from './links'
import type { WorkItemCard } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

/** One card on the board. The whole card is the drag handle and opens the task on click or Enter. */
export const BoardCard = memo(function BoardCard({
  card,
  dragId,
  draggable,
  focused,
  held,
  onFocus,
  onKeyDown,
  onOpen,
  onPick,
  onPointerDown,
  open,
  showStatus,
}: {
  card: WorkItemCard
  dragId: string
  draggable: boolean
  focused: boolean
  /** Picked up with the keyboard and being moved. */
  held: boolean
  onFocus: (dragId: string) => void
  onKeyDown: (event: KeyboardEvent<HTMLElement>, card: WorkItemCard, dragId: string) => void
  onOpen: (card: WorkItemCard, event: MouseEvent<HTMLElement>) => void
  onPick: (card: WorkItemCard, field: PickerField, anchor: HTMLElement) => void
  onPointerDown: (event: PointerEvent<HTMLElement>, cardId: string) => void
  open: boolean
  showStatus: boolean
}) {
  const editable = draggable && card.capabilities.edit && !card.archivedAt
  return (
    <li
      aria-describedby="board-drag-help"
      aria-label={`${card.key} ${card.title}`}
      className={['aw-tcard', held ? 'is-held' : '', open ? 'is-open' : '', draggable ? 'is-draggable' : ''].filter(Boolean).join(' ')}
      data-drag-id={dragId}
      onClick={(event) => {
        if ((event.target as HTMLElement).closest('button')) return
        onOpen(card, event)
      }}
      onContextMenu={(event) => {
        // A long press picks the card up; it shouldn't open the system menu too.
        if (draggable && (event.nativeEvent as { pointerType?: string }).pointerType === 'touch') event.preventDefault()
      }}
      onFocus={(event) => event.target === event.currentTarget && onFocus(dragId)}
      onKeyDown={(event) => event.target === event.currentTarget && onKeyDown(event, card, dragId)}
      onPointerDown={(event) => onPointerDown(event, dragId)}
      tabIndex={focused ? 0 : -1}
    >
      <div className="aw-tcard__top">
        {showStatus ? <StatusIcon category={card.status.semanticCategory} label={card.status.name} /> : null}
        <span className="aw-tcard__key">{card.key}</span>
        <span className="aw-tcard__due">
          <DueDate card={card} />
        </span>
      </div>
      <a className="aw-tcard__title" href={workItemHref(card.projectId, card.id)} onClick={(event) => event.preventDefault()} tabIndex={-1}>
        {card.title}
      </a>
      <div className="aw-tcard__meta">
        {editable ? (
          <button
            aria-label={`Priority of ${card.key}, change`}
            className="aw-tasks__cellbtn"
            onClick={(event) => onPick(card, 'priority', event.currentTarget)}
            tabIndex={-1}
            type="button"
          >
            <PriorityIcon priority={card.priority} />
          </button>
        ) : (
          <PriorityIcon priority={card.priority} />
        )}
        <TaskBadges card={card} compact />
        <span className="aw-tcard__assignee">
          {editable ? (
            <button
              aria-label={card.assignee ? `Assigned to ${card.assignee.displayName}, change` : `Assign ${card.key}`}
              className="aw-tasks__cellbtn"
              onClick={(event) => onPick(card, 'assignee', event.currentTarget)}
              tabIndex={-1}
              type="button"
            >
              <AssigneeMark card={card} size={20} />
            </button>
          ) : (
            <AssigneeMark card={card} size={20} />
          )}
        </span>
      </div>
    </li>
  )
})
