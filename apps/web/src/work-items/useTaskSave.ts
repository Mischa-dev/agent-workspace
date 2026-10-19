import { useCallback, useLayoutEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import { useUpdateWorkItem } from './hooks'
import { conflictCurrent, editErrorMessage } from './taskEdits'
import type { WorkItem, WorkItemPatch } from './types'

/** The parts of a task that save on their own, one at a time. */
export type TaskField =
  | 'title'
  | 'description'
  | 'status'
  | 'priority'
  | 'assignee'
  | 'type'
  | 'labels'
  | 'milestone'
  | 'parent'
  | 'startDate'
  | 'dueDate'
  | 'estimate'
  | 'blocked'

/** How each field reads in a sentence: "the due date". */
export const FIELD_NAMES: Record<TaskField, string> = {
  title: 'title',
  description: 'description',
  status: 'status',
  priority: 'priority',
  assignee: 'assignee',
  type: 'type',
  labels: 'labels',
  milestone: 'milestone',
  parent: 'parent',
  startDate: 'start date',
  dueDate: 'due date',
  estimate: 'estimate',
  blocked: 'blocked note',
}

type Patch = Omit<WorkItemPatch, 'expectedVersion'>

/** A field's value as text, to compare what someone saw with what the task holds now. */
export function fieldValue(item: WorkItem, field: TaskField): string {
  switch (field) {
    case 'title':
      return item.title
    case 'description':
      return item.description
    case 'status':
      return item.status.id
    case 'priority':
      return item.priority
    case 'assignee':
      return item.assignee?.actorId ?? ''
    case 'type':
      return item.itemType.id
    case 'labels':
      return item.labels
        .map((label) => label.id)
        .sort()
        .join(',')
    case 'milestone':
      return item.milestone?.id ?? ''
    case 'parent':
      return item.parent?.id ?? ''
    case 'startDate':
      return item.startDate ?? ''
    case 'dueDate':
      return item.dueDate ?? ''
    case 'estimate':
      return item.estimate === null ? '' : String(item.estimate)
    case 'blocked':
      return item.blocked?.reason ?? ''
  }
}

/** What a patch sets, in fieldValue's terms. Labels are added and removed, so they never clash. */
function patchValue(field: TaskField, patch: Patch): string | null {
  switch (field) {
    case 'title':
      return patch.title ?? ''
    case 'description':
      return patch.description ?? ''
    case 'status':
      return patch.statusId ?? ''
    case 'priority':
      return patch.priority ?? ''
    case 'assignee':
      return patch.assigneeActorId ?? ''
    case 'type':
      return patch.typeId ?? ''
    case 'labels':
      return null
    case 'milestone':
      return patch.milestoneId ?? ''
    case 'parent':
      return patch.parentWorkItemId ?? ''
    case 'startDate':
      return patch.startDate ?? ''
    case 'dueDate':
      return patch.dueDate ?? ''
    case 'estimate':
      return patch.estimate == null ? '' : String(patch.estimate)
    case 'blocked':
      return patch.blockedReason ?? ''
  }
}

export type SaveProblem = { field: string; message: string; conflict: boolean }

/** What went wrong, for the note under the title: a conflict says the task is up to date now; anything else is the server's message. */
export function problemFor(error: unknown, field: string): SaveProblem {
  if (conflictCurrent(error)) {
    return { field, conflict: true, message: `The ${field} didn’t change. The task is up to date now; try again if it still applies.` }
  }
  if (isApiError(error) && error.code === 'dependency_cycle') {
    const details = error.details as { path?: unknown } | undefined
    const path = Array.isArray(details?.path) ? details.path.filter((entry): entry is string => typeof entry === 'string') : []
    return { field, conflict: false, message: path.length ? `That would make a loop: ${path.join(' → ')}.` : error.message }
  }
  return { field, conflict: false, message: editErrorMessage(error, `The ${field} change`) }
}

type Entry = {
  id: number
  field: TaskField
  patch: Patch
  /** What the change sets, or null for changes that can't clash (labels). */
  mine: string | null
  /** What the person saw before the change. If the task holds something else when it's sent, someone else changed it. */
  base: string
  /** Waiting for the saves ahead of it, being sent, or stopped for the person to decide. */
  state: 'waiting' | 'saving' | 'conflict' | 'failed'
  theirs: WorkItem | null
  message: string | null
}

/** A field's save as the page shows it. `text` is the typed title or description, kept until it's saved or let go. */
export type PendingSave = {
  state: 'saving' | 'conflict' | 'failed'
  text: string | null
  /** The task as the other person left it, for a conflict. */
  theirs: WorkItem | null
  message: string | null
}

const MAX_ATTEMPTS = 4

/**
 * Saves each field of a task as soon as it changes, one save at a time.
 *
 * Every change remembers what the person saw before making it. If someone
 * else changed that same field in the meantime, nothing is overwritten: the
 * change stops as a conflict and keeps the person's text until they choose
 * "Keep yours" or "Use theirs". A version conflict caused by a change to
 * another field is retried against the newer task on its own.
 */
export function useTaskSave(item: WorkItem) {
  const update = useUpdateWorkItem()
  const queryClient = useQueryClient()
  const [problem, setProblem] = useState<SaveProblem | null>(null)
  // The saves live in a ref, so the queue always reads the latest; the page renders from the copy in state.
  const entries = useRef(new Map<TaskField, Entry>())
  const [shown, setShown] = useState<ReadonlyMap<TaskField, Entry>>(() => new Map())
  const lastId = useRef(0)
  const queue = useRef<Promise<void>>(Promise.resolve())
  const fallback = useRef(item)
  useLayoutEffect(() => {
    fallback.current = item
  }, [item])
  const { id: taskId, projectId } = item

  const current = useCallback(
    () => queryClient.getQueryData<WorkItem>(queryKeys.workItem(projectId, taskId)) ?? fallback.current,
    [projectId, queryClient, taskId],
  )

  const publish = () => setShown(new Map(entries.current))
  const find = (id: number) => [...entries.current.values()].find((entry) => entry.id === id) ?? null
  const change = (id: number, next: Partial<Entry> | null) => {
    const entry = find(id)
    if (!entry) return
    if (next) entries.current.set(entry.field, { ...entry, ...next })
    else entries.current.delete(entry.field)
    publish()
  }

  // A newer task from a conflict goes into the cache, unless a live update already brought a newer one.
  const takeNewer = useCallback(
    (theirs: WorkItem) => {
      const key = queryKeys.workItem(theirs.projectId, theirs.id)
      const cached = queryClient.getQueryData<WorkItem>(key)
      if (!cached || cached.version <= theirs.version) queryClient.setQueryData(key, theirs)
    },
    [queryClient],
  )

  const attempt = async (id: number) => {
    for (let round = 0; round < MAX_ATTEMPTS; round += 1) {
      const entry = find(id)
      if (!entry || (entry.state !== 'waiting' && entry.state !== 'saving')) return
      const task = current()
      if (entry.mine !== null) {
        const now = fieldValue(task, entry.field)
        if (now !== entry.base) {
          // Someone else changed this field. Already the same is fine; anything else is the person's call.
          change(id, now === entry.mine ? null : { state: 'conflict', theirs: task })
          return
        }
        if (now === entry.mine) {
          change(id, null)
          return
        }
      }
      change(id, { state: 'saving' })
      try {
        await update.mutateAsync({
          projectId: task.projectId,
          workItemId: task.id,
          idempotencyKey: crypto.randomUUID(),
          patch: { expectedVersion: task.version, ...entry.patch },
        })
        change(id, null)
        return
      } catch (error) {
        const theirs = conflictCurrent(error)
        if (!theirs) {
          change(id, { state: 'failed', message: problemFor(error, FIELD_NAMES[entry.field]).message })
          return
        }
        // The task moved on. The next round checks whether this field is one of the things that changed.
        takeNewer(theirs)
      }
    }
    change(id, { state: 'failed', message: 'The task kept changing while this saved. Try again.' })
  }

  const enqueue = (id: number) => {
    queue.current = queue.current.then(() =>
      attempt(id).catch(() => change(id, { state: 'failed', message: 'That didn’t save. Try again.' })),
    )
  }

  /** What the person sees for a field right now: their own change on its way, or the task's value. */
  const baseFor = useCallback(
    (field: TaskField) => {
      const entry = entries.current.get(field)
      if (entry && entry.mine !== null && (entry.state === 'waiting' || entry.state === 'saving')) return entry.mine
      return fieldValue(current(), field)
    },
    [current],
  )

  /**
   * Saves one field. `base` is what the person saw when they started the
   * change; it defaults to what the field shows now, which suits pickers.
   */
  const save = (field: TaskField, patch: Patch, base?: string) => {
    setProblem(null)
    const previous = entries.current.get(field)
    // A change that replaces one not sent yet starts from what the person saw before either.
    const from = previous?.state === 'waiting' ? previous.base : (base ?? baseFor(field))
    lastId.current += 1
    const entry: Entry = { id: lastId.current, field, patch, mine: patchValue(field, patch), base: from, state: 'waiting', theirs: null, message: null }
    entries.current.set(field, entry)
    publish()
    enqueue(entry.id)
  }

  const restart = (field: TaskField, patch: Patch | undefined, overTheirs: boolean) => {
    const entry = entries.current.get(field)
    if (!entry) return
    const next = patch ?? entry.patch
    entries.current.set(field, {
      ...entry,
      patch: next,
      mine: patchValue(field, next),
      base: overTheirs ? fieldValue(current(), field) : entry.base,
      state: 'waiting',
      theirs: null,
      message: null,
    })
    publish()
    enqueue(entry.id)
  }

  /** Saves the person's version over the other one; a new patch carries text edited since. */
  const keepMine = (field: TaskField, patch?: Patch) => restart(field, patch, true)

  /** Tries a failed save again, still checking against what the person first saw. */
  const retry = (field: TaskField, patch?: Patch) => restart(field, patch, false)

  /** Lets go of the person's change ("Use theirs", "Discard"); the field shows the task's value again. */
  const discard = (field: TaskField) => {
    if (!entries.current.delete(field)) return
    publish()
  }

  /** How a field's save stands, or null when nothing of the person's is waiting. */
  const pending = (field: TaskField): PendingSave | null => {
    const entry = shown.get(field)
    if (!entry) return null
    const text = field === 'title' ? (entry.patch.title ?? null) : field === 'description' ? (entry.patch.description ?? null) : null
    return { state: entry.state === 'waiting' ? 'saving' : entry.state, text, theirs: entry.theirs, message: entry.message }
  }

  const values = [...shown.values()]
  /** Stopped saves of the fields that have no place of their own to say so (all but the title and description). */
  const stopped = values
    .filter((entry) => (entry.state === 'conflict' || entry.state === 'failed') && entry.field !== 'title' && entry.field !== 'description')
    .map((entry) => ({ field: entry.field, state: entry.state as 'conflict' | 'failed', theirs: entry.theirs, message: entry.message }))
  const saving = values.some((entry) => entry.state === 'waiting' || entry.state === 'saving')

  /** For other task writes (dependencies, relations): a conflict loads the newer task and says so. */
  const fail = useCallback(
    (field: string, error: unknown) => {
      const theirs = conflictCurrent(error)
      if (theirs) takeNewer(theirs)
      setProblem(problemFor(error, field))
    },
    [takeNewer],
  )

  return {
    save,
    baseFor,
    pending,
    keepMine,
    retry,
    discard,
    stopped,
    saving,
    problem,
    fail,
    clearProblem: () => setProblem(null),
  }
}

export type TaskSave = ReturnType<typeof useTaskSave>
