import { useEffect, useRef, useState, type FormEvent } from 'react'

import { Banner, Button, Dialog, ListSkeleton, TextField, countLabel, describeLoadError } from '../ui'
import { useCreateLabel, useLabels, useUpdateLabel } from './planningHooks'
import { editErrorMessage } from './taskEdits'
import type { ProjectLabel } from './types'

const PRESET_COLORS = ['#6e56cf', '#3e63dd', '#0d9488', '#30a46c', '#e5484d', '#f76b15', '#d6409f', '#8b8d98']

/**
 * The browser's color picker. It previews while you drag and saves once, when
 * the picker closes (the native change event), so dragging doesn't send a
 * save for every shade it passes.
 */
function ColorInput({ label, onChange, value }: { label: string; onChange: (color: string) => void; value: string }) {
  const [draft, setDraft] = useState(value)
  const [shown, setShown] = useState(value)
  if (shown !== value) {
    setShown(value)
    setDraft(value)
  }
  const input = useRef<HTMLInputElement>(null)
  const onChangeRef = useRef(onChange)
  useEffect(() => {
    onChangeRef.current = onChange
  }, [onChange])
  useEffect(() => {
    const element = input.current
    if (!element) return undefined
    const commit = () => onChangeRef.current(element.value)
    element.addEventListener('change', commit)
    return () => element.removeEventListener('change', commit)
  }, [])
  return (
    <input aria-label={label} className="aw-labelmgr__color" onChange={(event) => setDraft(event.target.value)} ref={input} type="color" value={draft} />
  )
}

/** Labels for the whole project: add, rename, recolor, archive and restore. */
export function LabelManager({ onClose, projectId }: { onClose: () => void; projectId: string }) {
  const labels = useLabels(projectId, true)
  const create = useCreateLabel()
  const [name, setName] = useState('')
  const [color, setColor] = useState(PRESET_COLORS[0])

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || create.isPending) return
    create.mutate(
      { projectId, name: name.trim(), color, idempotencyKey: crypto.randomUUID() },
      {
        onSuccess: () => {
          setName('')
          setColor(PRESET_COLORS[(PRESET_COLORS.indexOf(color) + 1) % PRESET_COLORS.length])
        },
      },
    )
  }

  const active = (labels.data ?? []).filter((label) => !label.archived)
  const archived = (labels.data ?? []).filter((label) => label.archived)
  return (
    <Dialog description="Labels are shared by everyone in this project." onClose={onClose} open title="Manage labels" width={560}>
      <form className="aw-labelmgr__new" onSubmit={submit}>
        <ColorInput label="New label color" onChange={setColor} value={color} />
        <TextField aria-label="New label name" data-autofocus maxLength={60} onChange={(event) => setName(event.target.value)} placeholder="New label" value={name} />
        <Button busy={create.isPending} disabled={!name.trim()} icon="plus" size="sm" type="submit">
          Add
        </Button>
      </form>
      {create.isError ? <Banner tone="danger">{editErrorMessage(create.error, 'The label')}</Banner> : null}
      {labels.isPending ? <ListSkeleton label="Loading labels" rows={3} /> : null}
      {labels.isError ? (
        <Banner onRetry={() => void labels.refetch()} title="Labels didn’t load" tone="danger">
          {describeLoadError(labels.error)}
        </Banner>
      ) : null}
      {labels.isSuccess && active.length === 0 ? <p className="aw-quiet">No labels yet. Add one above.</p> : null}
      {active.length ? (
        <ul aria-label="Labels" className="aw-labelmgr__list">
          {active.map((label) => (
            <LabelRow key={label.id} label={label} projectId={projectId} />
          ))}
        </ul>
      ) : null}
      {archived.length ? (
        <>
          <h3 className="aw-links__title">Archived</h3>
          <ul aria-label="Archived labels" className="aw-labelmgr__list">
            {archived.map((label) => (
              <LabelRow key={label.id} label={label} projectId={projectId} />
            ))}
          </ul>
        </>
      ) : null}
    </Dialog>
  )
}

function LabelRow({ label, projectId }: { label: ProjectLabel; projectId: string }) {
  const update = useUpdateLabel()
  const [name, setName] = useState(label.name)
  const [shown, setShown] = useState(label.name)
  if (shown !== label.name) {
    setShown(label.name)
    setName(label.name)
  }
  const change = (fields: { name?: string; color?: string; archived?: boolean }) =>
    update.mutate({ projectId, labelId: label.id, expectedVersion: label.version, ...fields, idempotencyKey: crypto.randomUUID() })
  return (
    <li className={label.archived ? 'aw-labelmgr__row is-archived' : 'aw-labelmgr__row'}>
      <ColorInput label={`Color of ${label.name}`} onChange={(color) => change({ color })} value={label.color} />
      <TextField
        aria-label={`Name of ${label.name}`}
        disabled={label.archived}
        maxLength={60}
        onBlur={() => {
          if (name.trim() && name.trim() !== label.name) change({ name: name.trim() })
          else setName(label.name)
        }}
        onChange={(event) => setName(event.target.value)}
        onKeyDown={(event) => event.key === 'Enter' && event.currentTarget.blur()}
        value={name}
      />
      <span className="aw-labelmgr__count">
        {countLabel(label.taskCount, 'task')}
      </span>
      <Button busy={update.isPending} onClick={() => change({ archived: !label.archived })} size="sm" variant="ghost">
        {label.archived ? 'Restore' : 'Archive'}
      </Button>
      {update.isError ? (
        <p className="aw-prop__error" role="alert">
          {editErrorMessage(update.error, 'The label change')}
        </p>
      ) : null}
    </li>
  )
}
