import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Banner, Button, Dialog, SegmentedControl, Switch, TextField, describeError } from '../ui'
import { collectionKeys, createCollection, updateCollection, type CollectionBody, type CollectionSummary, type ViewType } from './api'

/**
 * Makes a collection, or changes one's name, description, what its title
 * column is called (such as Name, Version or Player) and whether its records
 * act like tasks.
 */
export function CollectionDetailsDialog({
  collection,
  onClose,
  onSaved,
  projectId,
}: {
  collection: CollectionSummary | null
  onClose: () => void
  onSaved: (body: CollectionBody) => void
  projectId: string
}) {
  const client = useQueryClient()
  const [name, setName] = useState(collection?.name ?? '')
  const [description, setDescription] = useState(collection?.description ?? '')
  const [titleName, setTitleName] = useState(collection?.titleName ?? 'Name')
  const [taskLike, setTaskLike] = useState(collection?.actsLikeTask === true)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the collection a name.')
    setBusy(true)
    const details = { name: name.trim(), description: description.trim(), titleName: titleName.trim() || 'Name' }
    try {
      const switched = taskLike !== (collection?.actsLikeTask === true) ? { actsLikeTask: taskLike } : {}
      let body: CollectionBody
      if (collection) {
        body = await updateCollection(projectId, collection.id, { expectedVersion: collection.version, ...details, ...switched })
      } else {
        body = await createCollection(projectId, { ...details, idempotencyKey: crypto.randomUUID() })
        if (taskLike) body = await updateCollection(projectId, body.collection.id, { expectedVersion: body.collection.version, actsLikeTask: true })
      }
      client.setQueryData(collectionKeys.one(projectId, body.collection.id), body)
      void client.invalidateQueries({ queryKey: collectionKeys.all(projectId) })
      onSaved(body)
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
      setBusy(false)
    }
  }
  return (
    <Dialog
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {collection ? 'Save' : 'Make collection'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={collection ? `Edit ${collection.name}` : 'New collection'}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField
        autoFocus
        hint={collection ? undefined : 'What each record is a list of, such as Playtest feedback, Builds or Recipes.'}
        label="Name"
        maxLength={80}
        onChange={(event) => setName(event.target.value)}
        required
        value={name}
      />
      <TextField hint="Optional." label="Description" maxLength={500} onChange={(event) => setDescription(event.target.value)} value={description} />
      <TextField
        hint="The first column, which names each record."
        label="Title column"
        maxLength={60}
        onChange={(event) => setTitleName(event.target.value)}
        value={titleName}
      />
      {collection?.builtin ? null : (
        <Switch
          checked={taskLike}
          description={
            collection?.actsLikeTask && collection.records > 0
              ? 'Its records are tasks now, with their own history, so this stays on.'
              : 'Each record gets a status, assignee, priority, dates and labels, a key like the project’s tasks, and a task page. Records already here become tasks.'
          }
          disabled={collection?.actsLikeTask === true && collection.records > 0}
          label="Records act like tasks"
          onChange={setTaskLike}
        />
      )}
    </Dialog>
  )
}

const LAYOUTS: { value: ViewType; label: string; icon: 'table' | 'board' | 'list' | 'clock'; hint: string }[] = [
  { value: 'table', label: 'Table', icon: 'table', hint: 'Rows and columns, each value edited in place.' },
  { value: 'board', label: 'Board', icon: 'board', hint: 'A column for each value of a choice, person or checkbox field. Drag a card to change it.' },
  { value: 'gallery', label: 'Gallery', icon: 'list', hint: 'A card for each record, with the fields you choose.' },
  { value: 'calendar', label: 'Calendar', icon: 'clock', hint: 'A month, with each record on the day of a date field.' },
]

/** Names a new view (and picks its layout, when `layout` is given), or renames one. */
export function ViewNameDialog({
  description,
  initial,
  layout,
  onClose,
  onSubmit,
  submitLabel,
  title,
}: {
  description?: string
  initial: string
  /** The layout a new view starts with; left out, the dialog only names. */
  layout?: ViewType
  onClose: () => void
  /** Resolves once saved; a rejection shows its message. */
  onSubmit: (name: string, layout: ViewType) => Promise<unknown>
  submitLabel: string
  title: string
}) {
  const [name, setName] = useState(initial)
  const [type, setType] = useState<ViewType>(layout ?? 'table')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the view a name.')
    setBusy(true)
    try {
      await onSubmit(name.trim(), type)
      onClose()
    } catch (error) {
      setProblem(describeError(error).message)
      setBusy(false)
    }
  }
  return (
    <Dialog
      description={description}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            {submitLabel}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
      open
      title={title}
      width={400}
    >
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <TextField autoFocus label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
      {layout ? (
        <div className="aw-collection__layout">
          <SegmentedControl label="Layout" onChange={setType} options={LAYOUTS} value={type} />
          <p className="aw-collection__layouthint">{LAYOUTS.find((entry) => entry.value === type)?.hint}</p>
        </div>
      ) : null}
    </Dialog>
  )
}
