import { useRef, useState } from 'react'

import { FieldEditor } from '../fields/FieldEditor'
import { FieldValueView } from '../fields/FieldValueView'
import { displayText, isEmpty, type FieldDefinition, type FieldReference, type FieldType, type FieldValue } from '../fields/fieldModel'
import { Banner, DocumentSkeleton, ErrorState, Icon, IconButton, Menu, TextArea, formatFullDate, formatRelative, showToast, type MenuEntry } from '../ui'
import { useRecord, type CollectionBody, type HistoryEntry } from './api'
import { historyValue, recordName, referenceKind } from './collectionModel'
import type { RecordEdits } from './useRecordEdits'

const ACTIONS: Record<string, string> = { created: 'added this', updated: 'changed', archived: 'archived this', restored: 'restored this' }

/**
 * One record beside its table: the title, every field to fill in or change,
 * who added and last changed it, and its history with each value before and
 * after. Escape closes it unless a field or a picker in it wants the key.
 */
export function RecordPanel({
  body,
  canEdit,
  edits,
  onClose,
  projectId,
  recordId,
}: {
  body: CollectionBody
  /** The person may change records here: neither the project nor the collection is archived. */
  canEdit: boolean
  edits: RecordEdits
  onClose: () => void
  projectId: string
  recordId: string
}) {
  const record = useRecord(projectId, body.collection.id, recordId)
  const [editing, setEditing] = useState<{ field: FieldDefinition; anchor: HTMLElement } | null>(null)
  const picked = useRef<string | null>(null)
  const close = <IconButton icon="x" label="Close" onClick={onClose} size="sm" />
  if (record.isPending) {
    return (
      <aside aria-busy="true" aria-label="Record" className="aw-recordpanel">
        <div className="aw-recordpanel__bar">
          <span className="aw-recordpanel__number">{body.collection.name}</span>
          {close}
        </div>
        <DocumentSkeleton />
      </aside>
    )
  }
  if (!record.data) {
    return (
      <aside aria-label="Record" className="aw-recordpanel">
        <div className="aw-recordpanel__bar">
          <span className="aw-recordpanel__number">{body.collection.name}</span>
          {close}
        </div>
        <ErrorState error={record.error} onRetry={() => void record.refetch()} title="The record didn’t load" />
      </aside>
    )
  }
  const { record: current, references, history } = record.data
  const editable = canEdit && !current.archived
  const fields = body.fields.filter((field) => !field.archived).sort((left, right) => left.position - right.position)
  const menu: MenuEntry[] = [
    {
      label: 'Copy link',
      icon: 'link',
      onSelect: () => {
        const url = new URL(window.location.href)
        url.searchParams.set('record', current.id)
        void navigator.clipboard?.writeText(url.toString()).then(() => showToast({ tone: 'success', title: 'Link copied' }))
      },
    },
    ...(canEdit
      ? [
          {
            label: current.archived ? 'Restore' : 'Archive',
            icon: current.archived ? ('undo' as const) : ('archive' as const),
            onSelect: () => {
              void edits.save(current, { archived: !current.archived }).then((done) => {
                if (!done || current.archived) return
                showToast({
                  tone: 'success',
                  title: `${recordName(current)} archived`,
                  onUndo: () => edits.save({ ...current, archived: true }, { archived: false }),
                })
                onClose()
              })
            },
          },
        ]
      : []),
  ]

  const set = (field: FieldDefinition, value: FieldValue | null) => {
    const label = picked.current
    picked.current = null
    const reference: FieldReference | undefined = label && typeof value === 'string' ? { id: value, kind: referenceKind(field), label, key: null } : undefined
    void edits.save(current, { values: { [field.id]: value } }, reference)
  }

  return (
    <aside
      aria-label={recordName(current)}
      className="aw-recordpanel"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || event.defaultPrevented) return
        if ((event.target as HTMLElement).closest('input, textarea, select, .aw-popover, [role="dialog"][aria-modal="true"]')) return
        event.preventDefault()
        onClose()
      }}
    >
      <div className="aw-recordpanel__bar">
        <span className="aw-recordpanel__number">
          {body.collection.name} #{current.number}
        </span>
        <Menu items={menu} label="Record actions" placement="bottom-end" trigger={<IconButton icon="more" label="Record actions" size="sm" />} />
        {close}
      </div>
      {current.archived ? (
        <Banner icon="archive" tone="warning">
          This record is archived, so it’s hidden from the table.
        </Banner>
      ) : null}
      <TitleEditor editable={editable} label={body.collection.titleName} onSave={(title) => void edits.save(current, { title })} title={current.title} />
      <dl className="aw-recordpanel__fields">
        {fields.map((field) => {
          const value = current.values[field.id]
          const shown = displayText(field, value, references)
          let control
          if (!editable) {
            control = <FieldValueView empty="Empty" field={field} references={references} value={value} />
          } else if (field.type === 'checkbox') {
            control = (
              <button
                aria-label={`${field.name}: ${value === true ? 'checked' : 'not checked'}. Change`}
                aria-pressed={value === true}
                className="aw-recordpanel__value"
                onClick={() => set(field, value === true ? null : true)}
                type="button"
              >
                <Icon name={value === true ? 'circle-check' : 'unassigned'} size={16} />
                {value === true ? 'Yes' : <span className="aw-prop__empty">No</span>}
              </button>
            )
          } else {
            control = (
              <button
                aria-haspopup="dialog"
                aria-label={`${field.name}: ${shown || 'empty'}. Change`}
                className={`aw-recordpanel__value${isEmpty(value) ? ' is-empty' : ''}`}
                onClick={(event) => setEditing({ field, anchor: event.currentTarget })}
                type="button"
              >
                {isEmpty(value) ? (
                  <span className="aw-prop__empty">Empty</span>
                ) : field.type === 'url' ? (
                  <span className="aw-prop__text">{shown}</span>
                ) : (
                  <FieldValueView field={field} references={references} value={value} />
                )}
              </button>
            )
          }
          return (
            <div className="aw-recordpanel__field" key={field.id}>
              <dt title={field.description || undefined}>{field.name}</dt>
              <dd>
                {control}
                {field.type === 'url' && editable && typeof value === 'string' ? (
                  <a className="aw-recordpanel__visit" href={value} rel="noreferrer noopener" target="_blank">
                    <Icon label={`Open ${shown}`} name="external" size={14} />
                  </a>
                ) : null}
              </dd>
            </div>
          )
        })}
      </dl>
      {fields.length === 0 ? <p className="aw-recordpanel__note">This collection has no fields yet. Add one from the table’s last column.</p> : null}
      <p className="aw-recordpanel__note">
        Added by {current.createdBy.name || 'someone'} <When at={current.createdAt} />
        {current.version > 1 ? (
          <>
            {' '}
            · changed by {current.updatedBy.name || 'someone'} <When at={current.updatedAt} />
          </>
        ) : null}
      </p>
      <History entries={history} types={new Map(body.fields.map((field) => [field.id, field.type]))} />
      {editing ? (
        <FieldEditor
          anchor={editing.anchor}
          field={editing.field}
          onClose={() => setEditing(null)}
          onLabel={(label) => {
            picked.current = label
          }}
          onSet={(value) => set(editing.field, value)}
          projectId={projectId}
          references={references}
          value={current.values[editing.field.id]}
        />
      ) : null}
    </aside>
  )
}

function When({ at }: { at: string }) {
  if (!at) return null
  return (
    <time dateTime={at} title={formatFullDate(at)}>
      {formatRelative(at)}
    </time>
  )
}

/**
 * The title, edited in place: Enter or leaving the box saves, Escape puts it
 * back. While the box has focus it keeps what's typed; otherwise it shows
 * the title as it is now, whoever changed it.
 */
function TitleEditor({ editable, label, onSave, title }: { editable: boolean; label: string; onSave: (title: string) => void; title: string }) {
  const [draft, setDraft] = useState<string | null>(null)
  if (!editable) return <h2 className="aw-recordpanel__title">{title || 'Untitled'}</h2>
  return (
    <TextArea
      aria-label={label}
      className="aw-recordpanel__titleinput"
      maxLength={300}
      onBlur={() => {
        const next = draft?.trim()
        setDraft(null)
        if (next !== undefined && next !== title) onSave(next)
      }}
      onChange={(event) => setDraft(event.target.value.replace(/\n/g, ' '))}
      onFocus={() => setDraft(title)}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault()
          event.currentTarget.blur()
        } else if (event.key === 'Escape' && draft !== title) {
          event.preventDefault()
          event.stopPropagation()
          setDraft(title)
        }
      }}
      placeholder="Untitled"
      rows={1}
      value={draft ?? title}
    />
  )
}

/** Who changed what, newest first, with each value before and after. */
function History({ entries, types }: { entries: HistoryEntry[]; types: ReadonlyMap<string, FieldType> }) {
  if (!entries.length) return null
  return (
    <section aria-labelledby="record-history" className="aw-recordpanel__history">
      <h3 className="aw-recordpanel__heading" id="record-history">
        History
      </h3>
      <ol className="aw-recordpanel__entries">
        {entries.map((entry) => (
          <li className="aw-recordpanel__entry" key={entry.id}>
            <p className="aw-recordpanel__who">
              <strong>{entry.actor || 'Someone'}</strong> {ACTIONS[entry.action] ?? entry.action}
              {entry.via ? <span className="aw-recordpanel__via"> through {entry.via}</span> : null} · <When at={entry.at} />
            </p>
            {entry.changes.length ? (
              <ul className="aw-recordpanel__changes">
                {entry.changes.map((change) => (
                  <li key={change.field}>
                    <span className="aw-recordpanel__changename">{change.name}</span>{' '}
                    {entry.action === 'created'
                      ? historyValue(change.after, types.get(change.field))
                      : `${historyValue(change.before, types.get(change.field))} → ${historyValue(change.after, types.get(change.field))}`}
                  </li>
                ))}
              </ul>
            ) : null}
          </li>
        ))}
      </ol>
    </section>
  )
}
