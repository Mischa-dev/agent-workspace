import { useRef, useState, type ClipboardEvent, type FormEvent, type RefObject } from 'react'

import { isConnectionError } from '../app/localDrafts'
import { useRetryWhenOnline } from '../app/useRetryWhenOnline'
import { formatBytes, uploadFile } from '../files/api'
import { FileGlyph, Paperclip } from '../files/icons'
import { MAX_ATTACHMENTS } from '../files/useUploads'
import '../files/files.css'
import { Banner, Button, Dialog, Icon, IconButton, SelectField, TextArea, TextField, showToast } from '../ui'
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
  // Files picked, pasted or dropped here upload to the task once it's made.
  const [files, setFiles] = useState<File[]>([])
  const [attaching, setAttaching] = useState(false)
  const fileInput = useRef<HTMLInputElement>(null)
  const addFiles = (picked: Iterable<File>) => {
    // Copied now: a FileList empties when its input is cleared.
    const added = [...picked]
    setFiles((current) => [...current, ...added].slice(0, MAX_ATTACHMENTS))
  }
  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const pasted = [...event.clipboardData.files]
    // A screenshot comes as a file alone; copied text that brings a picture along stays text.
    if (pasted.length === 0 || event.clipboardData.types.includes('text/plain')) return
    event.preventDefault()
    addFiles(pasted)
  }

  const attachTo = async (workItemId: string) => {
    const upload = uploadFile({ kind: 'task', projectId, workItemId })
    const results = await Promise.allSettled(files.map((file) => upload(file, () => undefined, new AbortController().signal)))
    const failed = results.filter((result) => result.status === 'rejected').length
    if (failed) {
      showToast({
        tone: 'danger',
        title: `${failed} ${failed === 1 ? 'file' : 'files'} didn’t upload`,
        description: 'Add them again from the task’s Files section.',
      })
    }
  }

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
          if (files.length === 0) {
            onCreated(item.id)
            return
          }
          setAttaching(true)
          void attachTo(item.id).finally(() => onCreated(item.id))
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
          <Button busy={mutation.isPending || attaching} disabled={!title.trim()} type="submit" variant="primary">
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
        onPaste={onPaste}
        placeholder="Add the outcome you want, any limits, and useful links."
        rows={5}
        value={description}
      />
      <div
        className="aw-newtask-files"
        onDragOver={(event) => {
          if (![...event.dataTransfer.types].includes('Files')) return
          event.preventDefault()
          event.dataTransfer.dropEffect = 'copy'
        }}
        onDrop={(event) => {
          if (event.dataTransfer.files.length === 0) return
          event.preventDefault()
          addFiles(event.dataTransfer.files)
        }}
      >
        {files.length ? (
          <ul aria-label="Files to attach" className="aw-uploads">
            {files.map((file, index) => (
              <li className="aw-upload is-done" key={`${file.name}-${index}`}>
                <span aria-hidden="true" className="aw-upload__icon">
                  <Icon name={FileGlyph} size={18} />
                </span>
                <span className="aw-upload__text">
                  <span className="aw-upload__name">{file.name}</span>
                  <span className="aw-upload__meta">{formatBytes(file.size)}</span>
                </span>
                <IconButton
                  className="aw-upload__remove"
                  icon="x"
                  label={`Remove ${file.name}`}
                  onClick={() => setFiles((current) => current.filter((_, at) => at !== index))}
                  size="sm"
                  tooltip={false}
                />
              </li>
            ))}
          </ul>
        ) : null}
        <Button
          disabled={files.length >= MAX_ATTACHMENTS}
          icon={Paperclip}
          onClick={() => fileInput.current?.click()}
          size="sm"
          variant="ghost"
        >
          Attach files
        </Button>
        <input
          className="visually-hidden"
          multiple
          onChange={(event) => {
            if (event.target.files) addFiles(event.target.files)
            event.target.value = ''
          }}
          ref={fileInput}
          tabIndex={-1}
          type="file"
        />
      </div>
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
