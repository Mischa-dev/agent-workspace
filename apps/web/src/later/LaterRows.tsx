import { Link } from 'react-router-dom'

import { Icon, IconButton, Menu, StatusChip, formatFullDate, type IconName, type MenuEntry } from '../ui'
import type { SavedItem, ScheduledMessage } from './api'
import { describeWhen, whenChoices } from './when'

const KIND_ICON: Record<SavedItem['kind'], IconName> = {
  message: 'message',
  work_item: 'work',
  reminder: 'clock',
}

function savedTitle(item: SavedItem) {
  if (item.kind === 'message') {
    if (item.message?.removed) return 'This message was removed'
    return item.message?.preview || 'A message'
  }
  if (item.kind === 'work_item') return item.workItem?.title ?? 'A task'
  return item.note
}

function savedMeta(item: SavedItem) {
  if (item.kind === 'message' && item.message) {
    return `${item.message.authorName} in ${item.message.place} · ${item.projectName}`
  }
  if (item.kind === 'work_item' && item.workItem) return `${item.workItem.key} · ${item.projectName}`
  return 'Reminder'
}

/** One saved message, task or reminder, with its reminder and Done. */
export function SavedRow({
  busy,
  item,
  now,
  onDone,
  onRemind,
  onRemindCustom,
  onRemove,
}: {
  busy: boolean
  item: SavedItem
  now: Date
  onDone: (done: boolean) => void
  onRemind: (at: Date | null) => void
  onRemindCustom: () => void
  onRemove: () => void
}) {
  const title = savedTitle(item)
  const due = item.remindAt && !item.done && new Date(item.remindAt).getTime() <= now.getTime()
  const remindItems: MenuEntry[] = [
    { heading: 'Remind me' },
    ...whenChoices(now).map((choice) => ({ label: choice.label, icon: 'clock' as const, onSelect: () => onRemind(choice.at) })),
    { label: 'Custom time…', icon: 'clock' as const, onSelect: onRemindCustom },
    ...(item.remindAt ? [{ separator: true as const }, { label: 'No reminder', icon: 'x' as const, onSelect: () => onRemind(null) }] : []),
  ]
  return (
    <li className={`aw-later__row${item.done ? ' is-done' : ''}`}>
      <span className="aw-later__icon">
        <Icon name={KIND_ICON[item.kind]} size={16} />
      </span>
      <div className="aw-later__text">
        <Link className="aw-later__title" to={item.href}>
          {title}
        </Link>
        <span className="aw-later__meta">
          {savedMeta(item)}
          {item.note && item.kind !== 'reminder' ? <span className="aw-later__note"> · {item.note}</span> : null}
        </span>
      </div>
      <div className="aw-later__end">
        {item.remindAt && !item.done ? (
          <span title={formatFullDate(item.remindAt)}>
            <StatusChip icon="clock" tone={due ? 'warning' : 'muted'}>
              {describeWhen(item.remindAt, now)}
            </StatusChip>
          </span>
        ) : null}
        {item.done ? null : (
          <Menu items={remindItems} label={`Remind me: ${title}`} placement="bottom-end" trigger={<IconButton disabled={busy} icon="clock" label="Remind me" size="sm" />} />
        )}
        <IconButton
          disabled={busy}
          icon={item.done ? 'undo' : 'check'}
          label={item.done ? 'Move back to Later' : 'Mark done'}
          onClick={() => onDone(!item.done)}
          size="sm"
        />
        <IconButton disabled={busy} icon="trash" label="Remove" onClick={onRemove} size="sm" />
      </div>
    </li>
  )
}

/** Plain text only: a message with mentions or task links is rescheduled, not rewritten here. */
function plainOnly(item: ScheduledMessage) {
  return Array.isArray(item.content?.nodes) && item.content.nodes.every((node) => node.type === 'text')
}

/** One message you scheduled: when it sends, and Send now, Change time, Edit and Cancel. */
export function ScheduledRow({
  busy,
  item,
  now,
  onCancel,
  onEdit,
  onReschedule,
  onSendNow,
}: {
  busy: boolean
  item: ScheduledMessage
  now: Date
  onCancel: () => void
  onEdit: () => void
  onReschedule: () => void
  onSendNow: () => void
}) {
  const waiting = item.status === 'scheduled'
  return (
    <li className="aw-later__row">
      <span className="aw-later__icon">
        <Icon name={item.status === 'failed' ? 'circle-alert' : 'clock'} size={16} />
      </span>
      <div className="aw-later__text">
        <Link className="aw-later__title" to={item.href}>
          {item.plainText}
        </Link>
        <span className="aw-later__meta">
          To {item.place} · {item.projectName}
          {item.failure ? <span className="aw-later__failure"> · {item.failure}</span> : null}
        </span>
      </div>
      <div className="aw-later__end">
        <span title={formatFullDate(item.sendAt)}>
          <StatusChip icon={waiting ? 'clock' : 'circle-alert'} tone={waiting ? 'muted' : 'danger'}>
            {waiting ? describeWhen(item.sendAt, now) : 'Not sent'}
          </StatusChip>
        </span>
        {waiting ? (
          <>
            <IconButton disabled={busy} icon="send" label="Send now" onClick={onSendNow} size="sm" />
            <Menu
              items={[
                { label: 'Change time…', icon: 'clock', onSelect: onReschedule },
                ...(plainOnly(item) ? [{ label: 'Edit text…', icon: 'pencil' as const, onSelect: onEdit }] : []),
                { separator: true },
                { label: 'Cancel message', icon: 'x', danger: true, onSelect: onCancel },
              ]}
              label={`Scheduled message to ${item.place}`}
              placement="bottom-end"
              trigger={<IconButton disabled={busy} icon="more" label="More" size="sm" />}
            />
          </>
        ) : (
          <IconButton disabled={busy} icon="trash" label="Dismiss" onClick={onCancel} size="sm" />
        )}
      </div>
    </li>
  )
}
