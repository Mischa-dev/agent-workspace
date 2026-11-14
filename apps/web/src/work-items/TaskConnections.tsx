import { useRef, useState, type ComponentProps, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'

import { useProjectExperience } from '../experience/layout'
import { Avatar, Button, Checkbox, IconButton, Menu, TextField, actorKind } from '../ui'
import { createWorkItem } from './api'
import { useInvalidateWorkItems, useSetDependencies, useSetRelation } from './planningHooks'
import { TaskSearchPicker } from './TaskSearchPicker'
import { TaskSection } from './TaskSection'
import type { ActorRef, TaskRef, WorkItem } from './types'
import type { TaskSave } from './useTaskSave'
import { StatusIcon } from './WorkItemIcons'
import '../styles/work.css'

export type OpenTask = (task: { id: string; projectId: string; href: string }) => void

function TaskRefRow({ actions, assignee, onOpen, task }: { actions?: ReactNode; assignee?: ActorRef | null; onOpen: OpenTask; task: TaskRef }) {
  const keys = useProjectExperience(task.projectId).shows('tech.task_keys')
  return (
    <li className={task.finished ? 'aw-taskref is-finished' : 'aw-taskref'}>
      <StatusIcon category={task.statusCategory} label={task.statusName || undefined} />
      {keys ? <span className="aw-taskref__key">{task.key}</span> : null}
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

/** Subtasks: the list with how many are done, and a field to add more. Hidden while there are none and nobody is adding one. */
export function TaskSubtasks({
  adding,
  disabled,
  item,
  onAdding,
  onOpen,
}: {
  adding: boolean
  disabled: boolean
  item: WorkItem
  onAdding: (adding: boolean) => void
  onOpen: OpenTask
}) {
  const [title, setTitle] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const invalidate = useInvalidateWorkItems()
  const done = item.subtasks.filter((task) => task.finished).length
  if (!item.subtasks.length && !(adding && !disabled)) return null
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
    <TaskSection
      actions={!disabled && !adding ? <IconButton icon="plus" label="Add a subtask" onClick={() => onAdding(true)} size="sm" /> : null}
      count={item.subtasks.length ? `${done}/${item.subtasks.length}` : undefined}
      id={`subtasks-${item.id}`}
      title="Subtasks"
    >
      {item.subtasks.length ? (
        <ul className="aw-taskrefs">
          {item.subtasks.map((task) => (
            <TaskRefRow assignee={task.assignee} key={task.id} onOpen={onOpen} task={task} />
          ))}
        </ul>
      ) : null}
      {adding && !disabled ? (
        <AddForm
          busy={busy}
          label="New subtask"
          maxLength={240}
          onCancel={() => {
            setTitle('')
            onAdding(false)
          }}
          onChange={setTitle}
          onSubmit={(event) => void submit(event)}
          placeholder="Subtask title, then Enter"
          value={title}
        />
      ) : null}
      {error ? (
        <p className="aw-prop__error" role="alert">
          {error}
        </p>
      ) : null}
    </TaskSection>
  )
}

/**
 * The one-line field that adds a subtask or a criterion. It stays open for
 * the next one; Escape, or leaving it empty, closes it.
 */
export function AddForm({
  busy,
  label,
  maxLength,
  onCancel,
  onChange,
  onSubmit,
  placeholder,
  value,
}: {
  busy: boolean
  label: string
  maxLength: number
  onCancel: () => void
  onChange: (value: string) => void
  onSubmit: (event: FormEvent<HTMLFormElement>) => void
  placeholder: string
  value: string
}) {
  return (
    <form className="aw-inline-add" onSubmit={onSubmit}>
      <TextField
        aria-label={label}
        autoFocus
        maxLength={maxLength}
        onBlur={(event) => {
          if (!value.trim() && !event.currentTarget.form?.contains(event.relatedTarget as Node | null)) onCancel()
        }}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== 'Escape') return
          event.preventDefault()
          event.stopPropagation()
          onCancel()
        }}
        placeholder={placeholder}
        value={value}
      />
      <Button busy={busy} disabled={!value.trim()} size="sm" type="submit" variant="primary">
        Add
      </Button>
      <Button onClick={onCancel} size="sm" variant="ghost">
        Done
      </Button>
    </form>
  )
}

export type DependencyKind = 'blockedBy' | 'blocks' | 'relates_to' | 'duplicates'
export type DependencyRequest = { kind: DependencyKind; anchor: HTMLElement }

const DEPENDENCY_CHOICES: { kind: DependencyKind; label: string }[] = [
  { kind: 'blockedBy', label: 'Blocked by…' },
  { kind: 'blocks', label: 'Blocks…' },
  { kind: 'relates_to', label: 'Related to…' },
  { kind: 'duplicates', label: 'Duplicate of…' },
]

const PICKER_LABELS: Record<DependencyKind, string> = {
  blockedBy: 'Blocked by',
  blocks: 'Blocks',
  relates_to: 'Related to',
  duplicates: 'Duplicate of',
}

/**
 * A menu of the kinds of link between tasks. Choosing one opens the task
 * search beside it, a frame later so the menu has handed focus back first.
 */
export function DependencyMenu({ onChoose, trigger }: { onChoose: (request: DependencyRequest) => void; trigger: ComponentProps<typeof Menu>['trigger'] }) {
  const anchor = useRef<HTMLSpanElement>(null)
  return (
    <span className="aw-menuanchor" ref={anchor}>
      <Menu
        items={DEPENDENCY_CHOICES.map((choice) => ({
          label: choice.label,
          onSelect: () =>
            window.requestAnimationFrame(() => {
              if (anchor.current) onChoose({ kind: choice.kind, anchor: anchor.current })
            }),
        }))}
        label="Add a dependency"
        trigger={trigger}
      />
    </span>
  )
}

/** What this task waits on, what waits on it, and other links between tasks. Hidden while there are none. */
export function TaskDependencies({
  disabled,
  item,
  onOpen,
  onRequest,
  request,
  save,
}: {
  disabled: boolean
  item: WorkItem
  onOpen: OpenTask
  onRequest: (request: DependencyRequest | null) => void
  /** The task search to show, opened from here or from the page's Add row. */
  request: DependencyRequest | null
  save: TaskSave
}) {
  const dependencies = useSetDependencies()
  const relations = useSetRelation()
  const navigate = useNavigate()
  const [closeDuplicate, setCloseDuplicate] = useState(true)
  const busy = dependencies.isPending || relations.isPending
  const base = { projectId: item.projectId, workItemId: item.id, expectedVersion: item.version }
  const linked = [item.id, ...item.blockedBy.map((task) => task.id), ...item.blocks.map((task) => task.id), ...item.relations.map((relation) => relation.task.id)]

  const changeDependency = (direction: 'blockedBy' | 'blocks', target: string, remove = false) =>
    dependencies.mutate(
      { ...base, [direction]: [target], ...(remove ? { remove: true } : {}), idempotencyKey: crypto.randomUUID() },
      { onError: (error) => save.fail('dependencies', error) },
    )
  const changeRelation = (kind: 'relates_to' | 'duplicates', target: string, options: { remove?: boolean; close?: boolean } = {}) =>
    relations.mutate(
      { ...base, kind, target, ...options, idempotencyKey: crypto.randomUUID() },
      { onError: (error) => save.fail('related tasks', error) },
    )

  const remove = (label: string, run: () => void) => <IconButton disabled={busy} icon="x" label={label} onClick={run} size="sm" />
  const openRef: OpenTask = (task) => (task.projectId === item.projectId ? onOpen(task) : navigate(task.href))

  const picker = (
    <TaskSearchPicker
      anchor={request?.anchor ?? null}
      exclude={linked}
      extra={
        request?.kind === 'duplicates' ? (
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
      label={request ? PICKER_LABELS[request.kind] : 'Find a task'}
      onClose={() => onRequest(null)}
      onPick={(target) => {
        const kind = request?.kind
        onRequest(null)
        const reference = target.id ?? target.key
        if (!reference || !kind) return
        if (kind === 'blockedBy' || kind === 'blocks') changeDependency(kind, reference)
        else if (kind === 'duplicates') changeRelation('duplicates', reference, { close: closeDuplicate })
        else changeRelation('relates_to', reference)
      }}
      projectId={item.projectId}
    />
  )
  if (!item.blockedBy.length && !item.blocks.length && !item.relations.length) return picker

  return (
    <TaskSection
      actions={!disabled ? <DependencyMenu onChoose={onRequest} trigger={<IconButton icon="plus" label="Add a dependency" size="sm" />} /> : null}
      id={`deps-${item.id}`}
      title="Dependencies"
    >
      <div className="aw-deps">
        {item.blockedBy.length ? (
          <DependencyGroup title="Blocked by">
            {item.blockedBy.map((task) => (
              <TaskRefRow
                actions={!disabled ? remove(`Remove ${task.key} as a blocker`, () => changeDependency('blockedBy', task.id, true)) : null}
                key={task.id}
                onOpen={openRef}
                task={task}
              />
            ))}
          </DependencyGroup>
        ) : null}
        {item.blocks.length ? (
          <DependencyGroup title="Blocks">
            {item.blocks.map((task) => (
              <TaskRefRow
                actions={!disabled ? remove(`Stop ${task.key} waiting on this`, () => changeDependency('blocks', task.id, true)) : null}
                key={task.id}
                onOpen={openRef}
                task={task}
              />
            ))}
          </DependencyGroup>
        ) : null}
        {item.relations.length ? (
          <DependencyGroup title="Related">
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
        ) : null}
      </div>
      {picker}
    </TaskSection>
  )
}

function DependencyGroup({ children, title }: { children: ReactNode; title: string }) {
  return (
    <div className="aw-deps__group">
      <h3 className="aw-deps__title">{title}</h3>
      <ul className="aw-taskrefs">{children}</ul>
    </div>
  )
}
