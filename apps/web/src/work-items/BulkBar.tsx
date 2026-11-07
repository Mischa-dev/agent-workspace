import { Button } from '../ui'
import { DatePicker } from '../ui/pickers'
import { ALL_FIELDS, pickerShown, type CardFields } from './cardFields'
import type { PickerField } from './FieldPicker'
import { MAX_BULK } from './selectionModel'

const ACTIONS: Array<{ field: PickerField; label: string; shortcut: string }> = [
  { field: 'status', label: 'Status', shortcut: 'S' },
  { field: 'assignee', label: 'Assignee', shortcut: 'A' },
  { field: 'labels', label: 'Labels', shortcut: 'L' },
  { field: 'priority', label: 'Priority', shortcut: 'P' },
  { field: 'milestone', label: 'Milestone', shortcut: 'M' },
]

/** Changes for every selected task at once. All of them change, or none do. */
export function BulkBar({
  archived,
  busy,
  count,
  fields = ALL_FIELDS,
  onArchive,
  onClear,
  onDueDate,
  onField,
  onLaunch,
  onPick,
  onRestore,
}: {
  /** Every selected task is archived, so restoring is the only change. */
  archived: boolean
  busy: boolean
  count: number
  /** What the project's layout shows: hidden fields get no button. */
  fields?: CardFields
  onArchive: () => void
  onClear: () => void
  onDueDate: (value: string | null) => void
  /** Set or clear a custom field on each selected task; left out where the project has none to show. */
  onField?: (anchor: HTMLElement) => void
  /** Start an agent on each selected task; left out where launching isn't offered. */
  onLaunch?: () => void
  onPick: (field: PickerField, anchor: HTMLElement) => void
  onRestore: () => void
}) {
  if (count === 0) return null
  const tooMany = count > MAX_BULK
  return (
    <div aria-label="Change selected tasks" className="aw-bulkbar" role="toolbar">
      <span aria-live="polite" className="aw-bulkbar__count">
        {count} selected
      </span>
      <div className="aw-bulkbar__actions">
        {tooMany ? (
          <span className="aw-bulkbar__count">Bulk changes take up to {MAX_BULK} tasks at a time.</span>
        ) : archived ? (
          <Button busy={busy} icon="archive" onClick={onRestore} size="sm" variant="ghost">
            Restore
          </Button>
        ) : (
          <>
            {ACTIONS.filter((action) => pickerShown(action.field, fields)).map((action) => (
              <Button
                aria-keyshortcuts={action.shortcut}
                disabled={busy}
                key={action.field}
                onClick={(event) => onPick(action.field, event.currentTarget)}
                size="sm"
                title={`${action.label} (${action.shortcut})`}
              >
                {action.label}
              </Button>
            ))}
            {onField ? (
              <Button aria-haspopup="dialog" disabled={busy} onClick={(event) => onField(event.currentTarget)} size="sm">
                Field
              </Button>
            ) : null}
            {onLaunch ? (
              <Button disabled={busy} icon="play" onClick={onLaunch} size="sm">
                Launch
              </Button>
            ) : null}
            {fields.dates ? (
              <DatePicker className="aw-bulkbar__date" disabled={busy} label="Due date for selected" onChange={onDueDate} placeholder="Due date" value={null} />
            ) : null}
            <Button busy={busy} icon="archive" onClick={onArchive} size="sm" variant="ghost">
              Archive
            </Button>
          </>
        )}
      </div>
      <Button aria-keyshortcuts="Escape" icon="x" onClick={onClear} size="sm" title="Clear selection (Esc)" variant="ghost">
        Clear
      </Button>
    </div>
  )
}
