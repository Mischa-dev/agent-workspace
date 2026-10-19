import { useQueryClient } from '@tanstack/react-query'
import { useImperativeHandle, useRef, useState, type RefObject } from 'react'

import { Attachments } from '../files/Attachments'
import { removeFile, uploadFile, type FileAttachment } from '../files/api'
import { UploadChips } from '../files/UploadChips'
import { useUploads } from '../files/useUploads'
import { Banner, ConfirmDialog, IconButton, describeError, describeLoadError, showToast } from '../ui'
import { taskFilesKey, useTaskFiles } from './hooks'
import { TaskSection } from './TaskSection'
import type { WorkItem } from './types'

/** Lets the rest of the page add files: the Add row's picker, or files dropped anywhere on the task. */
export type TaskFilesHandle = { choose: () => void; add: (files: FileList) => void }

/**
 * The task's own files: add by picker, phone camera or drop; anyone who can
 * edit the task can remove one. Hidden while there are none.
 */
export function TaskFiles({ disabled, handle, item }: { disabled: boolean; handle: RefObject<TaskFilesHandle | null>; item: WorkItem }) {
  const queryClient = useQueryClient()
  const files = useTaskFiles(item.projectId, item.id)
  const input = useRef<HTMLInputElement>(null)
  const [error, setError] = useState<string | null>(null)
  // Removing deletes the file for everyone, so it asks first.
  const [removing, setRemoving] = useState<FileAttachment | null>(null)
  const [removeBusy, setRemoveBusy] = useState(false)
  const [removeError, setRemoveError] = useState<string | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: taskFilesKey(item.projectId, item.id) })
  const upload = uploadFile({ kind: 'task', projectId: item.projectId, workItemId: item.id })
  const uploads = useUploads(async (file, onProgress, signal) => {
    const uploaded = await upload(file, onProgress, signal)
    await refresh()
    return uploaded
  })
  useImperativeHandle(handle, () => ({
    choose: () => input.current?.click(),
    add: (list: FileList) => setError(uploads.add(list)),
  }))
  // A finished upload shows in the list; only running and failed ones stay as chips.
  const waiting = uploads.items.filter((entry) => !entry.file)
  const own = (files.data ?? []).filter((file) => !file.commentId)
  const confirmRemove = () => {
    if (!removing) return
    const file = removing
    setRemoveBusy(true)
    setRemoveError(null)
    removeFile(file.id)
      .then(async () => {
        setRemoving(null)
        await refresh()
        showToast({ title: `Removed ${file.name}`, tone: 'success' })
      })
      .catch((reason: unknown) => setRemoveError(`The file wasn’t removed. ${describeError(reason).message}`))
      .finally(() => setRemoveBusy(false))
  }
  const dialog = (
    <ConfirmDialog
      busy={removeBusy}
      confirmLabel="Remove file"
      error={removeError}
      onClose={() => setRemoving(null)}
      onConfirm={confirmRemove}
      open={removing !== null}
      title={`Remove ${removing?.name ?? 'this file'}?`}
    >
      It’s deleted from this task for everyone.
    </ConfirmDialog>
  )
  const picker = disabled ? null : (
    <input
      aria-label="Choose files to attach"
      className="visually-hidden"
      multiple
      onChange={(event) => {
        if (event.target.files) setError(uploads.add(event.target.files))
        event.target.value = ''
      }}
      ref={input}
      tabIndex={-1}
      type="file"
    />
  )
  if (!own.length && !waiting.length && !error && !files.isError) return picker
  return (
    <TaskSection
      actions={disabled ? null : <IconButton icon="plus" label="Add files" onClick={() => input.current?.click()} size="sm" />}
      count={own.length || undefined}
      id={`files-${item.id}`}
      title="Files"
    >
      {picker}
      {files.isError ? (
        <Banner onRetry={() => void files.refetch()} title="Files didn’t load" tone="danger">
          {describeLoadError(files.error)}
        </Banner>
      ) : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <UploadChips items={waiting} onRemove={uploads.remove} />
      {own.length ? (
        <Attachments
          files={own}
          onRemove={
            disabled
              ? undefined
              : (file) => {
                  setRemoveError(null)
                  setRemoving(file)
                }
          }
        />
      ) : null}
      {dialog}
    </TaskSection>
  )
}
