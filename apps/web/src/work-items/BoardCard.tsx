import { memo, type KeyboardEvent, type MouseEvent, type PointerEvent } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { isEmpty, type FieldDefinition } from '../fields/fieldModel'
import { Icon } from '../ui'
import type { CardFields } from './cardFields'
import { AssigneeMark, DueDate, TaskBadges } from './ListCells'
import type { PickerField } from './FieldPicker'
import { workItemHref } from './links'
import type { WorkItemCard } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'

/**
 * One card on the board. The whole card is the drag handle and opens the task
 * on click or Enter; Cmd/Ctrl-click, Shift-click or its box selects it.
 */
export const BoardCard = memo(function BoardCard({
  card,
  dragId,
  draggable,
  fields,
  focused,
  held,
  onFocus,
  onKeyDown,
  onOpen,
  onPick,
  onPointerDown,
  onSelect,
  open,
  selected,
  showStatus,
}: {
  card: WorkItemCard
  dragId: string
  draggable: boolean
  fields: CardFields
  focused: boolean
  /** Picked up with the keyboard and being moved. */
  held: boolean
  onFocus: (dragId: string) => void
  onKeyDown: (event: KeyboardEvent<HTMLElement>, card: WorkItemCard, dragId: string) => void
  onOpen: (card: WorkItemCard, event: MouseEvent<HTMLElement>) => void
  onPick: (card: WorkItemCard, field: PickerField, anchor: HTMLElement) => void
  onPointerDown: (event: PointerEvent<HTMLElement>, cardId: string) => void
  /** Left out where nothing can change, so there's nothing to select for. */
  onSelect?: (card: WorkItemCard, range: boolean) => void
  open: boolean
  selected: boolean
  showStatus: boolean
}) {
  const editable = draggable && card.capabilities.edit && !card.archivedAt
  return (
    <li
      aria-describedby="board-drag-help"
      aria-label={`${card.key} ${card.title}${selected ? ', selected' : ''}`}
      className={['aw-tcard', held ? 'is-held' : '', open ? 'is-open' : '', selected ? 'is-selected' : '', draggable ? 'is-draggable' : ''].filter(Boolean).join(' ')}
      data-drag-id={dragId}
      onClick={(event) => {
        if ((event.target as HTMLElement).closest('button, label')) return
        // A click on the title lands on its link; the card is what J, K and X go on from.
        if (document.activeElement !== event.currentTarget) event.currentTarget.focus({ preventScroll: true })
        if (onSelect && (event.metaKey || event.ctrlKey || event.shiftKey)) {
          onSelect(card, event.shiftKey)
          return
        }
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
      {onSelect ? (
        // The box sits on the card's corner, so it takes no room from the card.
        <label className="aw-tcard__check">
          <input
            aria-label={`Select ${card.key}`}
            checked={selected}
            className="aw-check__input"
            onChange={() => undefined}
            onClick={(event) => onSelect(card, event.shiftKey)}
            tabIndex={-1}
            type="checkbox"
          />
          <span aria-hidden="true" className="aw-check__box">
            <Icon name="check" size={12} />
          </span>
        </label>
      ) : null}
      <div className="aw-tcard__top">
        {showStatus ? <StatusIcon category={card.status.semanticCategory} label={card.status.name} /> : null}
        {fields.keys ? <span className="aw-tcard__key">{card.key}</span> : null}
        {fields.dates ? (
          <span className="aw-tcard__due">
            <DueDate card={card} />
          </span>
        ) : null}
      </div>
      {/* Not natively draggable: the browser's own link drag would cancel the pointer drag. */}
      <a
        className="aw-tcard__title"
        draggable={false}
        href={workItemHref(card.projectId, card.id)}
        onClick={(event) => event.preventDefault()}
        tabIndex={-1}
      >
        {card.title}
      </a>
      <CardCustomFields card={card} fields={fields.custom} />
      <div className="aw-tcard__meta">
        {!fields.priority ? null : editable ? (
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
        <TaskBadges card={card} compact fields={fields} />
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

/** Most custom fields a card shows, in the project's order, so cards stay small. */
const CARD_FIELDS = 4

/** The custom fields a card has values for (AWD-89), each with its name, wrapping onto more lines; nothing when it has none. */
function CardCustomFields({ card, fields }: { card: WorkItemCard; fields: readonly FieldDefinition[] }) {
  const set = fields.filter((field) => field.type !== 'text' && !isEmpty(card.fieldValues[field.id])).slice(0, CARD_FIELDS)
  if (!set.length) return null
  return (
    <ul aria-label="Fields" className="aw-tcard__fields">
      {set.map((field) => (
        <li className="aw-taskmeta__item" key={field.id} title={field.name}>
          <span className="aw-taskmeta__text">{field.name}</span>
          <FieldValueView field={field} references={card.fieldReferences} value={card.fieldValues[field.id]} />
        </li>
      ))}
    </ul>
  )
}
