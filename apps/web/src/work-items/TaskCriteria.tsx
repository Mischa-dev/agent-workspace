import { useRef, useState, type FormEvent } from 'react'

import { Banner, Button, Checkbox, Icon, IconButton, TextField } from '../ui'
import { useCreateAcceptanceCriterion, useRemoveAcceptanceCriterion, useUpdateAcceptanceCriterion } from './hooks'
import type { WorkItem } from './types'

export function AcceptanceCriteria({
  disabled,
  item,
}: {
  disabled: boolean
  item: WorkItem
}) {
  const create = useCreateAcceptanceCriterion()
  const update = useUpdateAcceptanceCriterion()
  const remove = useRemoveAcceptanceCriterion()
  const [text, setText] = useState('')
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editingText, setEditingText] = useState('')
  const operationKey = useRef(crypto.randomUUID())
  const busy = create.isPending || update.isPending || remove.isPending
  const total = item.acceptanceCriteria.length
  const done = item.acceptanceCriteria.filter((criterion) => criterion.completed).length

  const nextKey = () => {
    const key = operationKey.current
    operationKey.current = crypto.randomUUID()
    return key
  }
  const base = { projectId: item.projectId, workItemId: item.id, expectedVersion: item.version }
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim() || busy) return
    create.mutate(
      { ...base, text: text.trim(), idempotencyKey: nextKey() },
      {
        onSuccess: () => setText(''),
      },
    )
  }

  return (
    <section aria-labelledby={`acceptance-${item.id}`} className="aw-worksection">
      <header className="aw-worksection__head">
        <h2 className="aw-worksection__title" id={`acceptance-${item.id}`}>
          <Icon name="checklist" size={16} />
          Acceptance criteria
        </h2>
        {total ? (
          <span className="aw-progress">
            <span className="aw-progress__text">
              {done} of {total} done
            </span>
            <progress aria-hidden="true" className="aw-progress__bar" max={total} value={done} />
          </span>
        ) : null}
      </header>
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
                    onChange={(completed) =>
                      update.mutate(
                        { ...base, criterionId: criterion.id, completed, idempotencyKey: nextKey() },
                      )
                    }
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
                        onClick={() =>
                          update.mutate({ ...base, criterionId: criterion.id, position: index, idempotencyKey: nextKey() })
                        }
                        size="sm"
                      />
                      <IconButton
                        disabled={busy || index === total - 1}
                        icon="arrow-down"
                        label={`Move ${criterion.text} down`}
                        onClick={() =>
                          update.mutate(
                            { ...base, criterionId: criterion.id, position: index + 2, idempotencyKey: nextKey() },
                              )
                        }
                        size="sm"
                      />
                      <IconButton
                        disabled={busy}
                        icon="trash"
                        label={`Remove ${criterion.text}`}
                        onClick={() =>
                          remove.mutate({ ...base, criterionId: criterion.id, idempotencyKey: nextKey() })
                        }
                        size="sm"
                      />
                    </div>
                  ) : null}
                </>
              )}
            </li>
          ))}
        </ol>
      ) : (
        <p className="aw-quiet">No acceptance criteria yet. Add what has to be true for this to count as done.</p>
      )}
      {!disabled ? (
        <form className="aw-inline-add" onSubmit={submit}>
          <TextField
            aria-label="New acceptance criterion"
            id={`criterion-${item.id}`}
            maxLength={1000}
            onChange={(event) => setText(event.target.value)}
            placeholder="Add a criterion, like “The export includes archived items”"
            value={text}
          />
          <Button busy={create.isPending} disabled={!text.trim() || busy} icon="plus" size="sm" type="submit">
            Add
          </Button>
        </form>
      ) : null}
      {create.isError || update.isError || remove.isError ? (
        <Banner tone="warning">The checklist changed before that finished. It’s up to date now, so try again.</Banner>
      ) : null}
    </section>
  )
}
