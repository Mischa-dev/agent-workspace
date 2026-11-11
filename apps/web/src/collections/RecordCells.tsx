import { useRef, useState, type KeyboardEvent, type RefObject } from 'react'

import { FieldValueView } from '../fields/FieldValueView'
import { displayText, isEmpty, type FieldDefinition, type FieldReference } from '../fields/fieldModel'
import { Icon, IconButton, TextField } from '../ui'
import type { CollectionRecord } from './api'
import { recordName } from './collectionModel'

/**
 * A record's title in the table: a click renames it in place (Enter saves,
 * Escape puts it back), and the panel button beside it opens the whole
 * record. Read-only, a click opens the record.
 */
export function TitleCell({
  editable,
  onOpen,
  onRename,
  record,
}: {
  editable: boolean
  onOpen: () => void
  onRename: (title: string) => void
  record: CollectionRecord
}) {
  const [draft, setDraft] = useState<string | null>(null)
  // Leaving the box saves, once: Enter leaves it, and Escape leaves it without saving.
  const cancelled = useRef(false)
  if (draft !== null) {
    return (
      <TextField
        aria-label={`Rename ${recordName(record)}`}
        autoFocus
        className="aw-records__titleinput"
        compact
        maxLength={300}
        onBlur={() => {
          const title = draft.trim()
          setDraft(null)
          if (!cancelled.current && title !== record.title) onRename(title)
          cancelled.current = false
        }}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event: KeyboardEvent<HTMLInputElement>) => {
          if (event.key === 'Enter') {
            event.preventDefault()
            event.currentTarget.blur()
          } else if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            cancelled.current = true
            event.currentTarget.blur()
          }
        }}
        value={draft}
      />
    )
  }
  return (
    <span className="aw-records__titlecell">
      <button
        aria-label={editable ? `${recordName(record)}, rename` : `Open ${recordName(record)}`}
        className="aw-records__title"
        onClick={() => (editable ? setDraft(record.title) : onOpen())}
        tabIndex={-1}
        type="button"
      >
        {record.title || <span className="aw-records__empty">Untitled</span>}
      </button>
      <IconButton className="aw-records__open" icon="panel-right" label={`Open ${recordName(record)}`} onClick={onOpen} size="sm" tabIndex={-1} tooltip={false} />
    </span>
  )
}

/**
 * One value in the table, as list cells show it. Editable, a click opens
 * the field's editor beside the cell, and a checkbox flips at once.
 */
export function ValueCell({
  editable,
  field,
  onEdit,
  onToggle,
  record,
  references,
}: {
  editable: boolean
  field: FieldDefinition
  onEdit: (anchor: HTMLElement) => void
  onToggle: () => void
  record: CollectionRecord
  references: readonly FieldReference[]
}) {
  const value = record.values[field.id]
  if (!editable) {
    return (
      <span className="aw-records__value">
        <FieldValueView field={field} references={references} value={value} />
      </span>
    )
  }
  if (field.type === 'checkbox') {
    return (
      <button
        aria-label={`${field.name} of ${recordName(record)}: ${value === true ? 'checked' : 'not checked'}. Change`}
        aria-pressed={value === true}
        className="aw-records__cellbtn"
        onClick={onToggle}
        tabIndex={-1}
        type="button"
      >
        <Icon name={value === true ? 'circle-check' : 'unassigned'} size={16} />
      </button>
    )
  }
  const shown = displayText(field, value, references)
  return (
    <button
      aria-haspopup="dialog"
      aria-label={`${field.name} of ${recordName(record)}: ${shown || 'empty'}. Change`}
      className={`aw-records__cellbtn${isEmpty(value) ? ' is-empty' : ''}`}
      onClick={(event) => onEdit(event.currentTarget)}
      tabIndex={-1}
      type="button"
    >
      {/* A web address reads as text here: a link can't sit inside a button. */}
      {field.type === 'url' ? <span className="aw-prop__text">{shown}</span> : <FieldValueView field={field} references={references} value={value} />}
    </button>
  )
}

/** Type a name and press Enter to add a record; the box stays for the next one. */
export function NewRecordInput({
  className,
  inputRef,
  onAdd,
}: {
  className: string
  inputRef?: RefObject<HTMLInputElement | null>
  onAdd: (title: string) => Promise<CollectionRecord | null>
}) {
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState(false)
  return (
    <form
      className={className}
      onSubmit={(event) => {
        event.preventDefault()
        const name = title.trim()
        if (!name || busy) return
        setBusy(true)
        void onAdd(name).then((added) => {
          setBusy(false)
          if (added) setTitle('')
        })
      }}
    >
      <Icon className={busy ? 'aw-spin' : undefined} name={busy ? 'loader' : 'plus'} size={14} />
      <TextField
        aria-label="New record"
        className="aw-records__newinput"
        compact
        enterKeyHint="done"
        maxLength={300}
        onChange={(event) => setTitle(event.target.value)}
        placeholder="New record"
        ref={inputRef}
        value={title}
      />
    </form>
  )
}
