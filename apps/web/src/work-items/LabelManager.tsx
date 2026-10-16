import { useState, type FormEvent } from 'react'

import { Banner, Button, Dialog, MessageSkeleton, TextField } from '../ui'
import { useCreateLabel, useLabels, useUpdateLabel } from './planningHooks'
import { editErrorMessage } from './taskEdits'
import type { ProjectLabel } from './types'

const PRESET_COLORS = ['#6e56cf', '#3e63dd', '#0d9488', '#30a46c', '#e5484d', '#f76b15', '#d6409f', '#8b8d98']

function ColorInput({ label, onChange, value }: { label: string; onChange: (color: string) => void; value: string }) {
  return (
    <input aria-label={label} className="aw-labelmgr__color" onChange={(event) => onChange(event.target.value)} type="color" value={value} />
  )
}

/** Labels for the whole Project: add, rename, recolor, archive and restore. */
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
    <Dialog description="Labels are shared by everyone in this Project." onClose={onClose} open title="Manage labels" width={560}>
      <form className="aw-labelmgr__new" onSubmit={submit}>
        <ColorInput label="New label color" onChange={setColor} value={color} />
        <TextField aria-label="New label name" data-autofocus maxLength={60} onChange={(event) => setName(event.target.value)} placeholder="New label" value={name} />
        <Button busy={create.isPending} disabled={!name.trim()} icon="plus" size="sm" type="submit">
          Add
        </Button>
      </form>
      {create.isError ? <Banner tone="danger">{editErrorMessage(create.error, 'The label')}</Banner> : null}
      {labels.isPending ? <MessageSkeleton label="Loading labels" rows={2} /> : null}
      {labels.isError ? (
        <Banner action={<Button onClick={() => void labels.refetch()} size="sm">Try again</Button>} tone="danger">
          Labels didn’t load.
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
        {label.taskCount} {label.taskCount === 1 ? 'work item' : 'work items'}
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
