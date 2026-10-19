import { useEffect, useRef, useState } from 'react'

import { Button } from '../ui'
import { newTaskValues } from './boardModel'
import { useCreateWorkItem } from './hooks'
import type { FilterValues } from './listModel'
import type { Notice } from './notice'
import { editErrorMessage } from './taskEdits'
import type { BoardColumn, BoardColumnsBy, WorkItem } from './types'

/**
 * Adds tasks to one board column: type a title and press Enter, and the box
 * stays for the next one. Escape, or leaving it empty, puts it away.
 */
export function ColumnComposer({
  column,
  columnsBy,
  filters,
  notify,
  onClose,
  onCreated,
  projectId,
}: {
  column: BoardColumn
  columnsBy: BoardColumnsBy
  filters: FilterValues
  notify: (notice: Notice) => void
  /** `refocus` when the person put it away (Escape, Cancel) rather than clicking elsewhere. */
  onClose: (refocus: boolean) => void
  onCreated: (item: WorkItem) => void
  projectId: string
}) {
  const [title, setTitle] = useState('')
  const input = useRef<HTMLTextAreaElement>(null)
  // One key per task, so a retried Enter can't add it twice.
  const idempotencyKey = useRef(crypto.randomUUID())
  const create = useCreateWorkItem()
  const name = column.label || 'None'

  useEffect(() => input.current?.focus({ preventScroll: true }), [])

  const submit = () => {
    const text = title.replace(/\s+/g, ' ').trim()
    if (!text || create.isPending) return
    create.mutate(
      { projectId, title: text, ...newTaskValues(columnsBy, column.key, filters), idempotencyKey: idempotencyKey.current },
      {
        onSuccess: (item) => {
          idempotencyKey.current = crypto.randomUUID()
          setTitle('')
          onCreated(item)
        },
        onError: (error) => notify({ tone: 'danger', title: `The task wasn’t added to ${name}`, description: editErrorMessage(error, 'Adding it') }),
      },
    )
  }

  return (
    <form
      className="aw-tcomposer"
      onBlur={(event) => {
        if (!title.trim() && !create.isPending && !event.currentTarget.contains(event.relatedTarget as Node | null)) onClose(false)
      }}
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
    >
      <textarea
        aria-label={`New task in ${name}`}
        className="aw-tcomposer__input"
        maxLength={240}
        onChange={(event) => setTitle(event.target.value)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return
          if (event.key === 'Enter') {
            event.preventDefault()
            submit()
          } else if (event.key === 'Escape') {
            event.preventDefault()
            event.stopPropagation()
            onClose(true)
          }
        }}
        placeholder="Task title"
        ref={input}
        rows={2}
        value={title}
      />
      <div className="aw-tcomposer__actions">
        <Button busy={create.isPending} disabled={!title.trim()} size="sm" type="submit" variant="primary">
          Add task
        </Button>
        <Button onClick={() => onClose(true)} size="sm" variant="ghost">
          Cancel
        </Button>
      </div>
    </form>
  )
}
