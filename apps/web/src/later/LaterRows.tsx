import { Icon, IconButton, Menu, StatusChip, formatFullDate, plainPreview, type IconName, type MenuEntry } from '../ui'
import { ListPiece, ListPieceTitle } from '../pieces/ListPiece'
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

/** Saved messages, tasks and reminders, each with its reminder and Done. */
export function SavedList({
  busyId,
  items,
  label,
  now,
  onDone,
  onRemind,
  onRemindCustom,
  onRemove,
}: {
  busyId: string | null
  items: readonly SavedItem[]
  label: string
  now: Date
  onDone: (item: SavedItem, done: boolean) => void
  onRemind: (item: SavedItem, at: Date | null) => void
  onRemindCustom: (item: SavedItem) => void
  onRemove: (item: SavedItem) => void
}) {
  return (
    <ListPiece
      actions={(item) => {
        const busy = busyId === item.id
        const remindItems: MenuEntry[] = [
          { heading: 'Remind me' },
          ...whenChoices(now).map((choice) => ({
            label: choice.label,
            icon: 'clock' as const,
            onSelect: () => onRemind(item, freshChoice(choice.id) ?? choice.at),
          })),
          { label: 'Custom time…', icon: 'clock' as const, onSelect: () => onRemindCustom(item) },
          ...(item.remindAt ? [{ separator: true as const }, { label: 'No reminder', icon: 'x' as const, onSelect: () => onRemind(item, null) }] : []),
        ]
        return (
          <>
            {item.done ? null : (
              <Menu
                items={remindItems}
                label={`Remind me: ${savedTitle(item)}`}
                placement="bottom-end"
                trigger={<IconButton disabled={busy} icon="clock" label="Remind me" size="sm" />}
              />
            )}
            <IconButton
              disabled={busy}
              icon={item.done ? 'undo' : 'check'}
              label={item.done ? 'Move back to Saved' : 'Mark done'}
              onClick={() => onDone(item, !item.done)}
              size="sm"
            />
            <IconButton disabled={busy} icon="trash" label="Remove from Later" onClick={() => onRemove(item)} size="sm" />
          </>
        )
      }}
      aside={(item) => {
        if (!item.remindAt || item.done) return null
        const due = new Date(item.remindAt).getTime() <= now.getTime()
        return (
          <span title={formatFullDate(item.remindAt)}>
            <StatusChip icon="clock" tone={due ? 'warning' : 'muted'}>
              {due ? `Due · ${describeWhen(item.remindAt, now)}` : describeWhen(item.remindAt, now)}
            </StatusChip>
          </span>
        )
      }}
      isDone={(item) => item.done}
      itemKey={(item) => item.id}
      items={items}
      label={label}
      leading={(item) => (
        <span className="aw-later__icon">
          <Icon name={KIND_ICON[item.kind]} size={16} />
        </span>
      )}
      meta={(item) => {
        const note = item.kind !== 'reminder' ? plainPreview(item.note, 160) : ''
        return (
          <>
            {savedMeta(item)}
            {note ? <span className="aw-later__note"> · {note}</span> : null}
          </>
        )
      }}
      // A reminder lives here; it has nowhere else to open.
      title={(item) => <ListPieceTitle to={item.kind === 'reminder' ? undefined : item.href}>{savedTitle(item)}</ListPieceTitle>}
    />
  )
}

/** Plain text only: a message with mentions or task links is rescheduled, not rewritten here. */
function plainOnly(item: ScheduledMessage) {
  return Array.isArray(item.content?.nodes) && item.content.nodes.every((node) => node.type === 'text')
}

/** Messages you scheduled: when each sends, and Send now, Change time, Edit and Cancel. */
export function ScheduledList({
  busyId,
  items,
  now,
  onCancel,
  onCopy,
  onEdit,
  onReschedule,
  onSendNow,
}: {
  busyId: string | null
  items: readonly ScheduledMessage[]
  now: Date
  onCancel: (item: ScheduledMessage) => void
  onCopy: (item: ScheduledMessage) => void
  onEdit: (item: ScheduledMessage) => void
  onReschedule: (item: ScheduledMessage) => void
  onSendNow: (item: ScheduledMessage) => void
}) {
  return (
    <ListPiece
      actions={(item) => {
        const waiting = item.status === 'scheduled'
        const busy = busyId === item.id
        // A failed message keeps every way to rescue its text; throwing it away asks first.
        const menu: MenuEntry[] = [
          { label: 'Change time…', icon: 'clock', onSelect: () => onReschedule(item) },
          ...(plainOnly(item) ? [{ label: 'Edit text…', icon: 'pencil' as const, onSelect: () => onEdit(item) }] : []),
          { label: 'Copy text', icon: 'copy', onSelect: () => onCopy(item) },
          { separator: true },
          waiting
            ? { label: 'Cancel message…', icon: 'x', danger: true, onSelect: () => onCancel(item) }
            : { label: 'Discard message…', icon: 'trash', danger: true, onSelect: () => onCancel(item) },
        ]
        return (
          <>
            <IconButton
              disabled={busy}
              icon={waiting ? 'send' : 'retry'}
              label={waiting ? 'Send now' : 'Try sending again'}
              onClick={() => onSendNow(item)}
              size="sm"
            />
            <Menu
              items={menu}
              label={`Scheduled message to ${item.place}`}
              placement="bottom-end"
              trigger={<IconButton disabled={busy} icon="more" label="More" size="sm" />}
            />
          </>
        )
      }}
      aside={(item) => {
        const waiting = item.status === 'scheduled'
        return (
          <span title={formatFullDate(item.sendAt)}>
            <StatusChip icon={waiting ? 'clock' : 'circle-alert'} tone={waiting ? 'muted' : 'danger'}>
              {waiting ? describeWhen(item.sendAt, now) : 'Not sent'}
            </StatusChip>
          </span>
        )
      }}
      itemKey={(item) => item.id}
      items={items}
      label="Scheduled messages"
      leading={(item) => (
        <span className={`aw-later__icon${item.status === 'scheduled' ? '' : ' is-failed'}`}>
          <Icon name={item.status === 'scheduled' ? 'clock' : 'circle-alert'} size={16} />
        </span>
      )}
      meta={(item) => (
        <>
          To {item.place} · {item.projectName}
          {item.failure ? <span className="aw-later__failure"> · {item.failure}</span> : null}
        </>
      )}
      title={(item) => <ListPieceTitle to={item.href}>{plainPreview(item.plainText, 200) || 'A message'}</ListPieceTitle>}
    />
  )
}
