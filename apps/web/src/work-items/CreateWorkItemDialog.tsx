import { useState, type FormEvent, type RefObject } from 'react'

import { isConnectionError } from '../app/localDrafts'
import { useRetryWhenOnline } from '../app/useRetryWhenOnline'
import { Banner, Button, Dialog, SelectField, TextArea, TextField } from '../ui'
import { workItemPriorities } from './constants'
import { useCreateWorkItem } from './hooks'
import { useTaskDraft } from './taskDraft'
import type { WorkItemPriority, WorkItemWorkflow } from './types'
import { PriorityIcon } from './WorkItemIcons'

export function CreateWorkItemDialog({
  onClose,
  onCreated,
  projectId,
  returnFocusRef,
  workflow,
}: {
  onClose: () => void
  onCreated: (id: string) => void
  projectId: string
  returnFocusRef: RefObject<HTMLButtonElement | null>
  workflow: WorkItemWorkflow
}) {
  const mutation = useCreateWorkItem()
  // The title and description are kept in this browser until the task is made.
  const { draft, update, clear } = useTaskDraft(projectId)
  const { title, description } = draft
  const [offline, setOffline] = useState(false)
  const [typeId, setTypeId] = useState(workflow.types.find((item) => item.isDefault)?.id ?? workflow.types[0]?.id ?? '')
  const [priority, setPriority] = useState<WorkItemPriority>('no_priority')

  const create = () => {
    if (!title.trim() || mutation.isPending) return
    mutation.mutate(
      {
        projectId,
        title: title.trim(),
        description,
        typeId: typeId || undefined,
        priority,
        idempotencyKey: draft.idempotencyKey,
      },
      {
        onSuccess: (item) => {
          setOffline(false)
          clear()
          onCreated(item.id)
        },
        onError: (reason) => setOffline(isConnectionError(reason)),
      },
    )
  }
  // Made with the same key once the connection is back, so it can't be made twice.
  useRetryWhenOnline(offline, create)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    create()
  }

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={mutation.isPending} disabled={!title.trim()} type="submit" variant="primary">
            Create work item
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocusRef}
      title="New work item"
      width={560}
    >
      <TextField
        data-autofocus
        id="new-work-item-title"
        label="Title"
        maxLength={240}
        onChange={(event) => update({ title: event.target.value })}
        placeholder="What needs doing?"
        required
        value={title}
      />
      <TextArea
        id="new-work-item-description"
        label="Description"
        maxLength={100000}
        onChange={(event) => update({ description: event.target.value })}
        placeholder="Add the outcome you want, any limits, and useful links."
        rows={5}
        value={description}
      />
      <div className="aw-form__grid">
        <SelectField id="new-work-item-type" label="Type" onChange={(event) => setTypeId(event.target.value)} value={typeId}>
          {workflow.types.map((type) => (
            <option key={type.id} value={type.id}>
              {type.name}
            </option>
          ))}
        </SelectField>
        <SelectField
          id="new-work-item-priority"
          label="Priority"
          onChange={(event) => setPriority(event.target.value as WorkItemPriority)}
          prefix={<PriorityIcon decorative priority={priority} />}
          value={priority}
        >
          {workItemPriorities.map((item) => (
            <option key={item.value} value={item.value}>
              {item.label}
            </option>
          ))}
        </SelectField>
      </div>
      {offline ? (
        <Banner tone="offline">No connection. This task is saved here and will be created when you’re back online.</Banner>
      ) : mutation.isError ? (
        <Banner tone="danger">{mutation.error.message}</Banner>
      ) : null}
    </Dialog>
  )
}
