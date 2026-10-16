import { Button, DatePicker } from '../ui'
import type { PickerField } from './FieldPicker'

const ACTIONS: Array<{ field: PickerField; label: string; shortcut: string }> = [
  { field: 'status', label: 'Status', shortcut: 'S' },
  { field: 'assignee', label: 'Assignee', shortcut: 'A' },
  { field: 'labels', label: 'Labels', shortcut: 'L' },
  { field: 'priority', label: 'Priority', shortcut: 'P' },
  { field: 'milestone', label: 'Milestone', shortcut: 'M' },
]

/** Changes for every selected task at once. All of them change, or none do. */
export function BulkBar({
  busy,
  count,
  onArchive,
  onClear,
  onDueDate,
  onPick,
}: {
  busy: boolean
  count: number
  onArchive: () => void
  onClear: () => void
  onDueDate: (value: string | null) => void
  onPick: (field: PickerField, anchor: HTMLElement) => void
}) {
  if (count === 0) return null
  return (
    <div aria-label="Change selected work items" className="aw-bulkbar" role="toolbar">
      <span aria-live="polite" className="aw-bulkbar__count">
        {count} selected
      </span>
      <div className="aw-bulkbar__actions">
        {ACTIONS.map((action) => (
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
        <DatePicker className="aw-bulkbar__date" disabled={busy} label="Due date for selected" onChange={onDueDate} placeholder="Due date" value={null} />
        <Button busy={busy} icon="archive" onClick={onArchive} size="sm" variant="ghost">
          Archive
        </Button>
      </div>
      <Button aria-keyshortcuts="Escape" icon="x" onClick={onClear} size="sm" title="Clear selection (Esc)" variant="ghost">
        Clear
      </Button>
    </div>
  )
}
