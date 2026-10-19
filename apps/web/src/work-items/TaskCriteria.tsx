import { useRef, useState, type FormEvent } from 'react'

import { Banner, Button, Checkbox, IconButton, TextField, describeError } from '../ui'
import { useCreateAcceptanceCriterion, useRemoveAcceptanceCriterion, useUpdateAcceptanceCriterion } from './hooks'
import { AddForm } from './TaskConnections'
import { TaskSection } from './TaskSection'
import type { WorkItem } from './types'

/** What has to be true for the task to count as done. Hidden while there are none and nobody is adding one. */
export function AcceptanceCriteria({
  adding,
  disabled,
  item,
  onAdding,
}: {
  adding: boolean
  disabled: boolean
  item: WorkItem
  onAdding: (adding: boolean) => void
}) {
  const create = useCreateAcceptanceCriterion()
  const update = useUpdateAcceptanceCriterion()
  const remove = useRemoveAcceptanceCriterion()
  const failure = create.error ?? update.error ?? remove.error
  const [text, setText] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editingText, setEditingText] = useState('')
  const operationKey = useRef(crypto.randomUUID())
  const busy = create.isPending || update.isPending || remove.isPending
  const total = item.acceptanceCriteria.length
  const done = item.acceptanceCriteria.filter((criterion) => criterion.completed).length
  if (!total && !(adding && !disabled)) return null

  const nextKey = () => {
    const key = operationKey.current
    operationKey.current = crypto.randomUUID()
    return key
  }
  const base = { projectId: item.projectId, workItemId: item.id, expectedVersion: item.version }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim() || busy) return
    create.mutate({ ...base, text: text.trim(), idempotencyKey: nextKey() }, { onSuccess: () => setText('') })
  }

  return (
    <TaskSection
      actions={!disabled && !adding ? <IconButton icon="plus" label="Add a criterion" onClick={() => onAdding(true)} size="sm" /> : null}
      count={total ? `${done}/${total}` : undefined}
      id={`acceptance-${item.id}`}
      title="Acceptance criteria"
    >
      {total ? (
        <ol className="aw-criteria">
          {item.acceptanceCriteria.map((criterion, index) => (
            <li className="aw-criterion" key={criterion.id}>
              {editingId === criterion.id ? (
                <form
                  className="aw-inline-add"
                  onSubmit={(event) => {
                    event.preventDefault()
                    if (!editingText.trim() || busy) return
                    update.mutate(
                      { ...base, criterionId: criterion.id, text: editingText.trim(), idempotencyKey: nextKey() },
                      {
                        onSuccess: () => {
                          setEditingId(null)
                          setEditingText('')
                        },
                      },
                    )
                  }}
                >
                  <TextField
                    aria-label="Criterion text"
                    autoFocus
                    maxLength={1000}
                    onChange={(event) => setEditingText(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key !== 'Escape') return
                      event.preventDefault()
                      event.stopPropagation()
                      setEditingId(null)
                    }}
                    value={editingText}
                  />
                  <Button busy={update.isPending} disabled={!editingText.trim()} size="sm" type="submit" variant="primary">
                    Save
                  </Button>
                  <Button disabled={busy} onClick={() => setEditingId(null)} size="sm" variant="ghost">
                    Cancel
                  </Button>
                </form>
              ) : (
                <>
                  <Checkbox
                    checked={criterion.completed}
                    description={criterion.completedBy ? `Done by ${criterion.completedBy.displayName}` : undefined}
                    disabled={disabled || busy}
                    label={criterion.text}
                    onChange={(completed) => update.mutate({ ...base, criterionId: criterion.id, completed, idempotencyKey: nextKey() })}
                  />
                  {!disabled ? (
                    <div className="aw-criterion__actions">
                      <IconButton
                        disabled={busy}
                        icon="pencil"
                        label={`Edit ${criterion.text}`}
                        onClick={() => {
                          setEditingId(criterion.id)
                          setEditingText(criterion.text)
                        }}
                        size="sm"
                      />
                      <IconButton
                        disabled={busy || index === 0}
                        icon="arrow-up"
                        label={`Move ${criterion.text} up`}
                        onClick={() => update.mutate({ ...base, criterionId: criterion.id, position: index, idempotencyKey: nextKey() })}
                        size="sm"
                      />
                      <IconButton
                        disabled={busy || index === total - 1}
                        icon="arrow-down"
                        label={`Move ${criterion.text} down`}
                        onClick={() => update.mutate({ ...base, criterionId: criterion.id, position: index + 2, idempotencyKey: nextKey() })}
                        size="sm"
                      />
                      <IconButton
                        disabled={busy}
                        icon="trash"
                        label={`Remove ${criterion.text}`}
                        onClick={() => remove.mutate({ ...base, criterionId: criterion.id, idempotencyKey: nextKey() })}
                        size="sm"
                      />
                    </div>
                  ) : null}
                </>
              )}
            </li>
          ))}
        </ol>
      ) : null}
      {adding && !disabled ? (
        <AddForm
          busy={create.isPending}
          label="New acceptance criterion"
          maxLength={1000}
          onCancel={() => {
            setText('')
            onAdding(false)
          }}
          onChange={setText}
          onSubmit={submit}
          placeholder="Something that must be true, like “Archived tasks are left out”"
          value={text}
        />
      ) : null}
      {failure ? (
        describeError(failure).stale ? (
          <Banner tone="warning">The checklist changed before that finished. It’s up to date now, so try again.</Banner>
        ) : (
          <Banner tone="danger">The checklist wasn’t saved. {describeError(failure).message}</Banner>
        )
      ) : null}
    </TaskSection>
  )
}
