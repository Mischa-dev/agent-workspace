import { IconButton, SelectField, TextField } from '../ui'
import { CATEGORY_LABELS } from './filterModel'
import { SEMANTIC_CATEGORIES } from './listModel'
import type { DraftEntry, DraftStatus } from './workflowModel'

type EntryProps<T extends DraftEntry> = {
  disabled: boolean
  entries: T[]
  noun: 'status' | 'type'
  onChange: (ref: string, change: Partial<T>) => void
  onDefault: (ref: string) => void
  onMove: (index: number, step: -1 | 1) => void
  onRemove: (ref: string) => void
}

/** Statuses or types in order, each with its name, default choice, order buttons and remove. */
export function WorkflowEntries<T extends DraftEntry>({ disabled, entries, noun, onChange, onDefault, onMove, onRemove }: EntryProps<T>) {
  const plural = noun === 'status' ? 'Statuses' : 'Types'
  return (
    <ol aria-label={plural} className="aw-wf__entries">
      {entries.map((entry, index) => {
        const name = entry.name.trim() || `this ${noun}`
        return (
          <li className="aw-wf__entry" key={entry.ref}>
            <span className="aw-wf__position" aria-hidden="true">
              {index + 1}
            </span>
            <TextField
              aria-label={`${noun === 'status' ? 'Status' : 'Type'} ${index + 1} name`}
              className="aw-wf__name"
              disabled={disabled}
              maxLength={60}
              onChange={(event) => onChange(entry.ref, { name: event.target.value } as Partial<T>)}
              value={entry.name}
            />
            {'semanticCategory' in entry ? (
              <SelectField
                aria-label={`What ${name} means`}
                className="aw-wf__category"
                disabled={disabled}
                onChange={(event) => onChange(entry.ref, { semanticCategory: event.target.value } as Partial<DraftStatus> as Partial<T>)}
                value={(entry as unknown as DraftStatus).semanticCategory}
              >
                {SEMANTIC_CATEGORIES.map((category) => (
                  <option key={category} value={category}>
                    {CATEGORY_LABELS[category] ?? category}
                  </option>
                ))}
              </SelectField>
            ) : null}
            <label className="aw-wf__default">
              <input checked={entry.isDefault} disabled={disabled} name={`default-${noun}`} onChange={() => onDefault(entry.ref)} type="radio" />
              Default
            </label>
            <span className="aw-wf__count">{entry.taskCount ? `${entry.taskCount} ${entry.taskCount === 1 ? 'task' : 'tasks'}` : entry.id ? 'No tasks' : 'New'}</span>
            <span className="aw-wf__buttons">
              <IconButton disabled={disabled || index === 0} icon="arrow-up" label={`Move ${name} up`} onClick={() => onMove(index, -1)} size="sm" />
              <IconButton
                disabled={disabled || index === entries.length - 1}
                icon="arrow-down"
                label={`Move ${name} down`}
                onClick={() => onMove(index, 1)}
                size="sm"
              />
              <IconButton disabled={disabled || entries.length === 1} icon="trash" label={`Remove ${name}`} onClick={() => onRemove(entry.ref)} size="sm" />
            </span>
          </li>
        )
      })}
    </ol>
  )
}

/** Allowed status changes: a row per "from" status and a column per "to" status. */
export function TransitionMatrix({
  disabled,
  onToggle,
  pairs,
  statuses,
}: {
  disabled: boolean
  onToggle: (from: string, to: string) => void
  pairs: string[]
  statuses: DraftStatus[]
}) {
  const allowed = new Set(pairs)
  return (
    <div className="aw-wf__matrix-scroll" role="region" aria-label="Allowed status changes" tabIndex={0}>
      <table className="aw-wf__matrix">
        <thead>
          <tr>
            <th scope="col">
              <span className="aw-wf__corner">From ↓ To →</span>
            </th>
            {statuses.map((status) => (
              <th key={status.ref} scope="col">
                {status.name || 'Unnamed'}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {statuses.map((from) => (
            <tr key={from.ref}>
              <th scope="row">{from.name || 'Unnamed'}</th>
              {statuses.map((to) => (
                <td key={to.ref}>
                  {from.ref === to.ref ? (
                    <span aria-hidden="true" className="aw-wf__self">
                      —
                    </span>
                  ) : (
                    <input
                      aria-label={`Allow ${from.name || 'Unnamed'} to ${to.name || 'Unnamed'}`}
                      checked={allowed.has(`${from.ref}|${to.ref}`)}
                      disabled={disabled}
                      onChange={() => onToggle(from.ref, to.ref)}
                      type="checkbox"
                    />
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
