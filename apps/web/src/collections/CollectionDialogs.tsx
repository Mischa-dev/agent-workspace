import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Banner, Button, Dialog, TextField, describeError } from '../ui'
import { collectionKeys, createCollection, updateCollection, type CollectionBody, type CollectionSummary } from './api'

/**
 * Makes a collection, or changes one's name, description and what its title
 * column is called (such as Name, Version or Player).
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
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the collection a name.')
    setBusy(true)
    const details = { name: name.trim(), description: description.trim(), titleName: titleName.trim() || 'Name' }
    try {
      const body = collection
        ? await updateCollection(projectId, collection.id, { expectedVersion: collection.version, ...details })
        : await createCollection(projectId, { ...details, idempotencyKey: crypto.randomUUID() })
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
    </Dialog>
  )
}

/** Names a new view, or renames one. */
export function ViewNameDialog({
  description,
  initial,
  onClose,
  onSubmit,
  submitLabel,
  title,
}: {
  description?: string
  initial: string
  onClose: () => void
  /** Resolves once saved; a rejection shows its message. */
  onSubmit: (name: string) => Promise<unknown>
  submitLabel: string
  title: string
}) {
  const [name, setName] = useState(initial)
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the view a name.')
    setBusy(true)
    try {
      await onSubmit(name.trim())
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
    </Dialog>
  )
}
