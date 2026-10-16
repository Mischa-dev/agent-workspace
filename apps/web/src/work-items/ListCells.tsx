import type { ReactNode } from 'react'

import { Avatar, Icon, actorKind } from '../ui'
import { formatDateLabel } from '../ui/pickers'
import type { PickerField } from './FieldPicker'
import type { ListRow } from './listRows'
import { TaskGlyph } from './TaskGlyph'
import type { WorkItemCard, WorkItemGroupBy } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { WorkItemLabels } from './WorkItemLabels'
import { isOverdue } from './taskDisplay'

/** Small marks for what else is true of a task: blocked, subtasks, milestone, labels. */
export function TaskBadges({ card, compact = false }: { card: WorkItemCard; compact?: boolean }) {
  const blocked = Boolean(card.blockedReason) || card.openBlockerCount > 0
  return (
    <span className="aw-taskmeta">
      {blocked ? (
        <span className="aw-taskmeta__item is-blocked" title={card.blockedReason ?? `${card.openBlockerCount} open blocker${card.openBlockerCount === 1 ? '' : 's'}`}>
          <TaskGlyph name="blocked" />
          <span className="aw-taskmeta__text">{card.blockedReason ? 'Blocked' : `Blocked by ${card.openBlockerCount}`}</span>
        </span>
      ) : null}
      {card.claimedBy ? (
        <span className="aw-taskmeta__item is-claimed" title={`${card.claimedBy.displayName} is working on this`}>
          <Icon name="bot" size={14} />
          <span className="visually-hidden">Claimed by {card.claimedBy.displayName}</span>
        </span>
      ) : null}
      {card.subtaskCount > 0 ? (
        <span className="aw-taskmeta__item" title={`${card.subtasksDone} of ${card.subtaskCount} subtasks done`}>
          <TaskGlyph name="subtasks" />
          <span className="aw-taskmeta__text">
            {card.subtasksDone}/{card.subtaskCount}
          </span>
        </span>
      ) : null}
      {card.milestone && !compact ? (
        <span className="aw-taskmeta__item" title={`Milestone: ${card.milestone.name}`}>
          <TaskGlyph name="milestone" />
          <span className="aw-taskmeta__text aw-taskmeta__name">{card.milestone.name}</span>
        </span>
      ) : null}
      <WorkItemLabels labels={card.labels} max={compact ? 2 : 2} />
    </span>
  )
}

export function DueDate({ card }: { card: Pick<WorkItemCard, 'dueDate' | 'status'> }) {
  if (!card.dueDate) return null
  const overdue = isOverdue(card)
  return (
    <span className={overdue ? 'aw-due is-overdue' : 'aw-due'} title={overdue ? 'Overdue' : 'Due'}>
      {overdue ? <span className="visually-hidden">Overdue, </span> : null}
      {formatDateLabel(card.dueDate, { month: 'short', day: 'numeric' })}
    </span>
  )
}

export function AssigneeMark({ card, size = 22 }: { card: Pick<WorkItemCard, 'assignee'>; size?: number }) {
  return card.assignee ? (
    <Avatar kind={actorKind(card.assignee.kind)} label name={card.assignee.displayName} seed={card.assignee.actorId} size={size} />
  ) : (
    <span aria-label="Unassigned" className="aw-tasks__nobody" role="img">
      <Icon name="unassigned" size={16} />
    </span>
  )
}

/** A cell button that opens a picker. It isn't a tab stop: the row is, and its letter keys open the same pickers. */
export function PickerCell({
  children,
  field,
  label,
  onPick,
}: {
  children: ReactNode
  field: PickerField
  label: string
  onPick: (field: PickerField, anchor: HTMLElement) => void
}) {
  return (
    <button aria-label={label} className="aw-tasks__cellbtn" onClick={(event) => onPick(field, event.currentTarget)} tabIndex={-1} type="button">
      {children}
    </button>
  )
}

export function GroupHeader({
  groupBy,
  onToggle,
  row,
  statusCategory,
}: {
  groupBy: WorkItemGroupBy | null
  onToggle: () => void
  row: Extract<ListRow, { kind: 'group' }>
  statusCategory?: string
}) {
  return (
    <button aria-expanded={!row.collapsed} className="aw-tasks__group" onClick={onToggle} tabIndex={-1} type="button">
      <Icon className={row.collapsed ? 'aw-tasks__chevron' : 'aw-tasks__chevron is-open'} name="chevron-right" size={14} />
      {row.color ? <span aria-hidden="true" className="aw-label__dot" style={{ backgroundColor: row.color }} /> : null}
      {groupBy === 'status' && statusCategory ? <StatusIcon category={statusCategory} /> : null}
      {groupBy === 'priority' && row.groupKey !== 'none' ? <PriorityIcon decorative priority={row.groupKey as WorkItemCard['priority']} /> : null}
      <span className="aw-tasks__grouplabel">{row.label || 'None'}</span>
      <span className="aw-tasks__groupcount">{row.count ?? row.loaded}</span>
    </button>
  )
}
