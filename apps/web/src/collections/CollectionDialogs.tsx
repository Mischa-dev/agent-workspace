import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { Banner, Button, Checkbox, Dialog, SegmentedControl, SelectField, Switch, TextField, describeError } from '../ui'
import { useWorkItemWorkflow } from '../work-items/hooks'
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
  // Its own workflow, for a collection whose records already act like tasks.
  const ownWorkflow = Boolean(collection && !collection.builtin && collection.actsLikeTask)
  const workflow = useWorkItemWorkflow(ownWorkflow ? projectId : '')
  const [statuses, setStatuses] = useState<string[]>(collection?.taskStatuses ?? [])
  const [startStatus, setStartStatus] = useState(collection?.startStatusId ?? '')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const submit = async () => {
    setProblem(null)
    if (!name.trim()) return setProblem('Give the collection a name.')
    setBusy(true)
    const details = { name: name.trim(), description: description.trim(), titleName: titleName.trim() || 'Name' }
    try {
      const switched = taskLike !== (collection?.actsLikeTask === true) ? { actsLikeTask: taskLike } : {}
      const workflowChanged =
        ownWorkflow && taskLike && (statuses.join() !== (collection?.taskStatuses ?? []).join() || startStatus !== (collection?.startStatusId ?? ''))
      if (workflowChanged && startStatus && statuses.length > 0 && !statuses.includes(startStatus)) {
        setBusy(false)
        return setProblem('New tasks start in one of the statuses it uses.')
      }
      const ownStatuses = workflowChanged ? { workflow: { statuses, startStatusId: startStatus || null } } : {}
      let body: CollectionBody
      if (collection) {
        body = await updateCollection(projectId, collection.id, { expectedVersion: collection.version, ...details, ...switched, ...ownStatuses })
      } else {
        body = await createCollection(projectId, { ...details, actsLikeTask: taskLike, idempotencyKey: crypto.randomUUID() })
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
              ? 'Turned off, its tasks are put away with their keys and history, and come back as they were when it is turned on again. Not while one is claimed or has a run.'
              : 'Each record gets a status, assignee, priority, dates and labels, a key like the project’s tasks, and a task page. Records already here become tasks.'
          }
          label="Records act like tasks"
          onChange={setTaskLike}
        />
      )}
      {ownWorkflow && taskLike && workflow.data ? (
        <fieldset className="aw-collection-workflow">
          <legend>Statuses its tasks use</legend>
          <p className="aw-collection-workflow__hint">From the space’s workflow. Leave all unticked to use every one.</p>
          {workflow.data.statuses.map((status) => (
            <Checkbox
              checked={statuses.includes(status.id)}
              key={status.id}
              label={status.name}
              onChange={(checked) =>
                setStatuses((current) =>
                  checked
                    ? workflow.data.statuses.map((entry) => entry.id).filter((id) => id === status.id || current.includes(id))
                    : current.filter((id) => id !== status.id),
                )
              }
            />
          ))}
          <SelectField label="New tasks start in" onChange={(event) => setStartStatus(event.target.value)} value={startStatus}>
            <option value="">The space’s default</option>
            {workflow.data.statuses
              .filter((status) => statuses.length === 0 || statuses.includes(status.id))
              .map((status) => (
                <option key={status.id} value={status.id}>
                  {status.name}
                </option>
              ))}
          </SelectField>
        </fieldset>
      ) : null}
    </Dialog>
  )
}

const LAYOUTS: { value: ViewType; label: string; icon: 'table' | 'board' | 'list' | 'clock' | 'chart'; hint: string }[] = [
  { value: 'table', label: 'Table', icon: 'table', hint: 'Rows and columns, each value edited in place.' },
  { value: 'board', label: 'Board', icon: 'board', hint: 'A column for each value of a choice, person or checkbox field. Drag a card to change it.' },
  { value: 'list', label: 'List', icon: 'list', hint: 'One line for each record, with the fields you choose beside its name.' },
  { value: 'gallery', label: 'Gallery', icon: 'list', hint: 'A card for each record, with the fields you choose.' },
  { value: 'calendar', label: 'Calendar', icon: 'clock', hint: 'A month, with each record on the day of a date field.' },
  { value: 'timeline', label: 'Timeline', icon: 'clock', hint: 'Each record a bar from one date field to another, week by week.' },
  { value: 'chart', label: 'Chart', icon: 'chart', hint: 'Bars, a line or a donut of the records by group: how many, or a number field added up.' },
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
