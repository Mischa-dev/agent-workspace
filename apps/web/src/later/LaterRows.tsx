import { Link } from 'react-router-dom'

import { Icon, IconButton, Menu, StatusChip, formatFullDate, plainPreview, type IconName, type MenuEntry } from '../ui'
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
    return plainPreview(item.message?.preview, 200) || 'A message'
  }
  if (item.kind === 'work_item') return item.workItem?.title ?? 'A task'
  return plainPreview(item.note, 200) || 'Reminder'
}

function savedMeta(item: SavedItem) {
  if (item.kind === 'message' && item.message) {
    return `${item.message.authorName} in ${item.message.place} · ${item.projectName}`
  }
  if (item.kind === 'work_item' && item.workItem) return `${item.workItem.key} · ${item.projectName}`
  return 'Reminder'
}

/**
 * The quick reminder times as the menu shows them, each read again when it's
 * picked, so a page left open for an hour still sets "In 20 minutes" from now.
 */
function freshChoice(id: string) {
  return whenChoices(new Date()).find((choice) => choice.id === id)?.at ?? null
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
  const note = item.kind !== 'reminder' ? plainPreview(item.note, 160) : ''
  const remindItems: MenuEntry[] = [
    { heading: 'Remind me' },
    ...whenChoices(now).map((choice) => ({
      label: choice.label,
      icon: 'clock' as const,
      onSelect: () => onRemind(freshChoice(choice.id) ?? choice.at),
    })),
    { label: 'Custom time…', icon: 'clock' as const, onSelect: onRemindCustom },
    ...(item.remindAt ? [{ separator: true as const }, { label: 'No reminder', icon: 'x' as const, onSelect: () => onRemind(null) }] : []),
  ]
  return (
    <li className={`aw-later__row${item.done ? ' is-done' : ''}`}>
      <span className="aw-later__icon">
        <Icon name={KIND_ICON[item.kind]} size={16} />
      </span>
      <div className="aw-later__text">
        {/* A reminder lives here; it has nowhere else to open. */}
        {item.kind === 'reminder' ? (
          <span className="aw-later__title">{title}</span>
        ) : (
          <Link className="aw-later__title" to={item.href}>
            {title}
          </Link>
        )}
        <span className="aw-later__meta">
          {savedMeta(item)}
          {note ? <span className="aw-later__note"> · {note}</span> : null}
        </span>
      </div>
      <div className="aw-later__end">
        {item.remindAt && !item.done ? (
          <span title={formatFullDate(item.remindAt)}>
            <StatusChip icon="clock" tone={due ? 'warning' : 'muted'}>
              {due ? `Due · ${describeWhen(item.remindAt, now)}` : describeWhen(item.remindAt, now)}
            </StatusChip>
          </span>
        ) : null}
        {item.done ? null : (
          <Menu items={remindItems} label={`Remind me: ${title}`} placement="bottom-end" trigger={<IconButton disabled={busy} icon="clock" label="Remind me" size="sm" />} />
        )}
        <IconButton
          disabled={busy}
          icon={item.done ? 'undo' : 'check'}
          label={item.done ? 'Move back to Saved' : 'Mark done'}
          onClick={() => onDone(!item.done)}
          size="sm"
        />
        <IconButton disabled={busy} icon="trash" label="Remove from Later" onClick={onRemove} size="sm" />
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
  onCopy,
  onEdit,
  onReschedule,
  onSendNow,
}: {
  busy: boolean
  item: ScheduledMessage
  now: Date
  onCancel: () => void
  onCopy: () => void
  onEdit: () => void
  onReschedule: () => void
  onSendNow: () => void
}) {
  const waiting = item.status === 'scheduled'
  // A failed message keeps every way to rescue its text; throwing it away asks first.
  const menu: MenuEntry[] = [
    { label: 'Change time…', icon: 'clock', onSelect: onReschedule },
    ...(plainOnly(item) ? [{ label: 'Edit text…', icon: 'pencil' as const, onSelect: onEdit }] : []),
    { label: 'Copy text', icon: 'copy', onSelect: onCopy },
    { separator: true },
    waiting
      ? { label: 'Cancel message…', icon: 'x', danger: true, onSelect: onCancel }
      : { label: 'Discard message…', icon: 'trash', danger: true, onSelect: onCancel },
  ]
  return (
    <li className="aw-later__row">
      <span className={`aw-later__icon${waiting ? '' : ' is-failed'}`}>
        <Icon name={waiting ? 'clock' : 'circle-alert'} size={16} />
      </span>
      <div className="aw-later__text">
        <Link className="aw-later__title" to={item.href}>
          {plainPreview(item.plainText, 200) || 'A message'}
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
        <IconButton disabled={busy} icon={waiting ? 'send' : 'retry'} label={waiting ? 'Send now' : 'Try sending again'} onClick={onSendNow} size="sm" />
        <Menu
          items={menu}
          label={`Scheduled message to ${item.place}`}
          placement="bottom-end"
          trigger={<IconButton disabled={busy} icon="more" label="More" size="sm" />}
        />
      </div>
    </li>
  )
}
