import { useQueryClient } from '@tanstack/react-query'
import { useRef, useState, type DragEvent } from 'react'

import { queryKeys } from '../api/queryKeys'
import { Attachments } from '../files/Attachments'
import { removeFile, uploadFile, type FileAttachment } from '../files/api'
import { UploadChips } from '../files/UploadChips'
import { useUploads } from '../files/useUploads'
import { Banner, Button, Icon } from '../ui'
import { useTaskFiles } from './hooks'
import type { WorkItem } from './types'
import { Paperclip } from '../files/icons'

/** The task's own files: add by picker, phone camera or drop; anyone who can edit the task can remove one. */
export function TaskFiles({ disabled, item }: { disabled: boolean; item: WorkItem }) {
  const queryClient = useQueryClient()
  const files = useTaskFiles(item.projectId, item.id)
  const input = useRef<HTMLInputElement>(null)
  const [dragging, setDragging] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: queryKeys.workItemFiles(item.projectId, item.id) })
  const upload = uploadFile({ kind: 'task', projectId: item.projectId, workItemId: item.id })
  const uploads = useUploads(async (file, onProgress, signal) => {
    const uploaded = await upload(file, onProgress, signal)
    await refresh()
    return uploaded
  })
  // A finished upload shows in the list; only running and failed ones stay as chips.
  const waiting = uploads.items.filter((entry) => !entry.file)
  const own = (files.data ?? []).filter((file) => !file.commentId)
  const remove = (file: FileAttachment) => {
    setError(null)
    removeFile(file.id)
      .then(refresh)
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The file wasn’t removed.'))
  }
  const onDrop = (event: DragEvent<HTMLElement>) => {
    setDragging(false)
    if (disabled || !event.dataTransfer.types.includes('Files')) return
    event.preventDefault()
    setError(uploads.add(event.dataTransfer.files))
  }
  if (!own.length && disabled && !files.isError) return null
  return (
    <section
      aria-labelledby={`files-${item.id}`}
      className={dragging ? 'aw-worksection is-dropping' : 'aw-worksection'}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false)
      }}
      onDragOver={(event) => {
        if (disabled || !event.dataTransfer.types.includes('Files')) return
        event.preventDefault()
        setDragging(true)
      }}
      onDrop={onDrop}
    >
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id={`files-${item.id}`}>
          <Icon name={Paperclip} size={16} />
          Files
        </h2>
        {!disabled ? (
          <>
            <Button icon="plus" onClick={() => input.current?.click()} size="sm" variant="ghost">
              Add files
            </Button>
            <input
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
          </>
        ) : null}
      </header>
      {files.isError ? <Banner tone="danger">Files didn’t load.</Banner> : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <UploadChips items={waiting} onRemove={uploads.remove} />
      {own.length ? (
        <Attachments files={own} onRemove={disabled ? undefined : remove} />
      ) : files.isSuccess && !waiting.length ? (
        <p className="aw-quiet">{disabled ? 'No files.' : 'No files yet. Drop files here or add them.'}</p>
      ) : null}
    </section>
  )
}
