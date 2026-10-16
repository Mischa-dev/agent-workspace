import { useState, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'

import { Avatar, Button, Checkbox, Icon, IconButton, TextField, actorKind } from '../ui'
import { createWorkItem } from './api'
import { useInvalidateWorkItems, useSetDependencies, useSetRelation } from './planningHooks'
import { TaskGlyph } from './TaskGlyph'
import { TaskSearchPicker } from './TaskSearchPicker'
import type { ActorRef, TaskRef, WorkItem } from './types'
import type { TaskSave } from './useTaskSave'
import { StatusIcon } from './WorkItemIcons'

export type OpenTask = (task: { id: string; projectId: string; href: string }) => void

function TaskRefRow({ actions, assignee, onOpen, task }: { actions?: ReactNode; assignee?: ActorRef | null; onOpen: OpenTask; task: TaskRef }) {
  return (
    <li className={task.finished ? 'aw-taskref is-finished' : 'aw-taskref'}>
      <StatusIcon category={task.statusCategory} label={task.statusName || undefined} />
      <span className="aw-taskref__key">{task.key}</span>
      {task.href ? (
        <a
          className="aw-taskref__title"
          href={task.href}
          onClick={(event) => {
            if (event.metaKey || event.ctrlKey || event.shiftKey) return
            event.preventDefault()
            onOpen(task)
          }}
        >
          {task.title}
        </a>
      ) : (
        <span className="aw-taskref__title">{task.title}</span>
      )}
      {task.archived ? <span className="aw-worklist__tag">Archived</span> : null}
      {assignee ? <Avatar kind={actorKind(assignee.kind)} label name={assignee.displayName} seed={assignee.id} size={20} /> : null}
      {actions ? <span className="aw-taskref__actions">{actions}</span> : null}
    </li>
  )
}

function SectionHead({ action, count, icon, id, title }: { action?: ReactNode; count?: ReactNode; icon: ReactNode; id: string; title: string }) {
  return (
    <header className="aw-worksection__head">
      <h2 className="aw-worksection__title" id={id}>
        {icon}
        {title}
        {count !== undefined ? <span className="aw-worksection__count">{count}</span> : null}
      </h2>
      {action}
    </header>
  )
}

/** Subtasks: progress, the list, and a field to add one. */
export function TaskSubtasks({ disabled, item, onOpen }: { disabled: boolean; item: WorkItem; onOpen: OpenTask }) {
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const invalidate = useInvalidateWorkItems()
  const done = item.subtasks.filter((task) => task.finished).length
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    try {
      await createWorkItem({
        projectId: item.projectId,
        title: title.trim(),
        priority: 'no_priority',
        parentWorkItemId: item.id,
        ...(item.milestone ? { milestoneId: item.milestone.id } : {}),
        idempotencyKey: crypto.randomUUID(),
      })
      setTitle('')
    } catch {
      setError('The subtask wasn’t added. Try again.')
    } finally {
      setBusy(false)
      void invalidate(item.projectId)
    }
  }
  return (
    <section aria-labelledby={`subtasks-${item.id}`} className="aw-worksection">
      <SectionHead
        count={item.subtasks.length ? `${done} of ${item.subtasks.length} done` : undefined}
        icon={<TaskGlyph name="subtasks" size={16} />}
        id={`subtasks-${item.id}`}
        title="Subtasks"
      />
      {item.subtasks.length ? (
        <ul className="aw-taskrefs">
          {item.subtasks.map((task) => (
            <TaskRefRow assignee={task.assignee} key={task.id} onOpen={onOpen} task={task} />
          ))}
        </ul>
      ) : (
        <p className="aw-quiet">No subtasks. Split the work into steps someone can pick up on their own.</p>
      )}
      {!disabled ? (
        <form className="aw-inline-add" onSubmit={(event) => void submit(event)}>
          <TextField aria-label="New subtask" maxLength={240} onChange={(event) => setTitle(event.target.value)} placeholder="Add a subtask" value={title} />
          <Button busy={busy} disabled={!title.trim()} icon="plus" size="sm" type="submit">
            Add
          </Button>
        </form>
      ) : null}
      {error ? <p className="aw-prop__error" role="alert">{error}</p> : null}
    </section>
  )
}

type Adding = { kind: 'blockedBy' | 'blocks' | 'relates_to' | 'duplicates'; anchor: HTMLElement }

/** What this work item waits on, what waits on it, and other links between tasks. */
export function TaskDependencies({ disabled, item, onOpen, save }: { disabled: boolean; item: WorkItem; onOpen: OpenTask; save: TaskSave }) {
  const dependencies = useSetDependencies()
  const relations = useSetRelation()
  const navigate = useNavigate()
  const [adding, setAdding] = useState<Adding | null>(null)
  const [closeDuplicate, setCloseDuplicate] = useState(true)
  const busy = dependencies.isPending || relations.isPending
  const base = { projectId: item.projectId, workItemId: item.id, expectedVersion: item.version }
  const linked = [item.id, ...item.blockedBy.map((task) => task.id), ...item.blocks.map((task) => task.id), ...item.relations.map((relation) => relation.task.id)]

  const changeDependency = (direction: 'blockedBy' | 'blocks', target: string, remove = false) =>
    dependencies.mutate(
      { ...base, [direction]: [target], ...(remove ? { remove: true } : {}), idempotencyKey: crypto.randomUUID() },
      { onError: (error) => save.fail('Dependencies', error) },
    )
  const changeRelation = (kind: 'relates_to' | 'duplicates', target: string, options: { remove?: boolean; close?: boolean } = {}) =>
    relations.mutate(
      { ...base, kind, target, ...options, idempotencyKey: crypto.randomUUID() },
      { onError: (error) => save.fail('Related work items', error) },
    )

  const remove = (label: string, run: () => void) => (
    <IconButton disabled={busy} icon="x" label={label} onClick={run} size="sm" />
  )
  const openRef: OpenTask = (task) => (task.projectId === item.projectId ? onOpen(task) : navigate(task.href))
  const openBlockers = item.blockedBy.filter((task) => !task.finished).length

  return (
    <section aria-labelledby={`deps-${item.id}`} className="aw-worksection">
      <SectionHead
        count={item.ready ? 'Ready to start' : openBlockers ? `Waiting on ${openBlockers}` : undefined}
        icon={<Icon name="link" size={16} />}
        id={`deps-${item.id}`}
        title="Dependencies"
      />
      <div className="aw-deps">
        <DependencyGroup
          add={!disabled ? (anchor) => setAdding({ kind: 'blockedBy', anchor }) : undefined}
          addLabel="Add a blocker"
          empty="Nothing blocks this."
          title="Blocked by"
        >
          {item.blockedBy.map((task) => (
            <TaskRefRow
              actions={!disabled ? remove(`Remove ${task.key} as a blocker`, () => changeDependency('blockedBy', task.id, true)) : null}
              key={task.id}
              onOpen={openRef}
              task={task}
            />
          ))}
        </DependencyGroup>
        <DependencyGroup
          add={!disabled ? (anchor) => setAdding({ kind: 'blocks', anchor }) : undefined}
          addLabel="Add a work item this blocks"
          empty="This doesn’t block anything."
          title="Blocks"
        >
          {item.blocks.map((task) => (
            <TaskRefRow
              actions={!disabled ? remove(`Stop ${task.key} waiting on this`, () => changeDependency('blocks', task.id, true)) : null}
              key={task.id}
              onOpen={openRef}
              task={task}
            />
          ))}
        </DependencyGroup>
        <DependencyGroup
          add={!disabled ? (anchor) => setAdding({ kind: 'relates_to', anchor }) : undefined}
          addLabel="Relate to another work item"
          empty="No related work items."
          extra={
            !disabled ? (
              <Button onClick={(event) => setAdding({ kind: 'duplicates', anchor: event.currentTarget })} size="sm" variant="ghost">
                Mark as duplicate
              </Button>
            ) : null
          }
          title="Related"
        >
          {item.relations.map((relation) => (
            <TaskRefRow
              actions={
                <>
                  <span className="aw-taskref__kind">
                    {relation.kind === 'duplicates' ? 'Duplicate of' : relation.kind === 'duplicated_by' ? 'Duplicated by' : 'Related'}
                  </span>
                  {!disabled && relation.kind !== 'duplicated_by'
                    ? remove(`Remove the link to ${relation.task.key}`, () =>
                        changeRelation(relation.kind === 'duplicates' ? 'duplicates' : 'relates_to', relation.task.id, { remove: true }),
                      )
                    : null}
                </>
              }
              key={`${relation.kind}-${relation.task.id}`}
              onOpen={openRef}
              task={relation.task}
            />
          ))}
        </DependencyGroup>
      </div>
      <TaskSearchPicker
        anchor={adding?.anchor ?? null}
        exclude={linked}
        extra={
          adding?.kind === 'duplicates' ? (
            <div className="aw-deps__close">
              <Checkbox
                checked={closeDuplicate}
                description="Moves it to the workflow’s canceled status."
                label="Close this as a duplicate"
                onChange={setCloseDuplicate}
              />
            </div>
          ) : null
        }
        label={
          adding?.kind === 'blockedBy'
            ? 'Add a blocker'
            : adding?.kind === 'blocks'
              ? 'Add a work item this blocks'
              : adding?.kind === 'duplicates'
                ? 'Duplicate of'
                : 'Relate to'
        }
        onClose={() => setAdding(null)}
        onPick={(target) => {
          const kind = adding?.kind
          setAdding(null)
          const reference = target.id ?? target.key
          if (!reference || !kind) return
          if (kind === 'blockedBy' || kind === 'blocks') changeDependency(kind, reference)
          else if (kind === 'duplicates') changeRelation('duplicates', reference, { close: closeDuplicate })
          else changeRelation('relates_to', reference)
        }}
        projectId={item.projectId}
      />
    </section>
  )
}

function DependencyGroup({
  add,
  addLabel,
  children,
  empty,
  extra,
  title,
}: {
  add?: (anchor: HTMLElement) => void
  addLabel: string
  children: ReactNode[]
  empty: string
  extra?: ReactNode
  title: string
}) {
  return (
    <div className="aw-deps__group">
      <div className="aw-deps__head">
        <h3 className="aw-links__title">{title}</h3>
        {extra}
        {add ? <IconButton icon="plus" label={addLabel} onClick={(event) => add(event.currentTarget)} size="sm" /> : null}
      </div>
      {children.length ? <ul className="aw-taskrefs">{children}</ul> : <p className="aw-quiet">{empty}</p>}
    </div>
  )
}
