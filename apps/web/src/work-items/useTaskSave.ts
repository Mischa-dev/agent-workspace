import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import { callOp, newRequestId } from '../api/ops'
import { discardOutbound, useOutbound } from '../data/outbound'
import { showToast } from '../ui'
import { getWorkItem } from './api'
import { useQueueWorkItemUpdate, useUpdateWorkItem } from './hooks'
import { inOrder, remember } from '../views/saveQueue'
import { conflictCurrent, editErrorMessage } from './taskEdits'
import { suggestedUpdate } from './taskSuggest'
import { undoableTaskChange } from './taskUndo'
import type { WorkItem, WorkItemPatch } from './types'

/** The parts of a task that save on their own, one at a time; each custom field is `field:<id>`. */
export type TaskField = BuiltInField | `field:${string}`

type BuiltInField =
  | 'title'
  | 'description'
  | 'status'
  | 'priority'
  | 'assignee'
  | 'delegate'
  | 'type'
  | 'labels'
  | 'milestone'
  | 'parent'
  | 'startDate'
  | 'dueDate'
  | 'estimate'
  | 'blocked'

/** How each field reads in a sentence: "the due date". */
export const FIELD_NAMES: Record<BuiltInField, string> = {
  title: 'title',
  description: 'description',
  status: 'status',
  priority: 'priority',
  assignee: 'assignee',
  delegate: 'delegate',
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

/**
 * Fields whose change can wait on this device while the connection is down
 * (W1.30): plain values the page can show at once. The rest need the server
 * to say what they became, so offline they stop as a failed save.
 */
const WAITS_OFFLINE: ReadonlySet<TaskField> = new Set<TaskField>(['title', 'description', 'priority', 'startDate', 'dueDate', 'estimate'])

/** The task with a change that's waiting on this device shown in it. */
function showing(task: WorkItem, patch: Patch): WorkItem {
  const next = { ...task }
  if (patch.title !== undefined) next.title = patch.title
  if (patch.description !== undefined) next.description = patch.description
  if (patch.priority !== undefined) next.priority = patch.priority
  if (patch.startDate !== undefined) next.startDate = patch.startDate
  if (patch.dueDate !== undefined) next.dueDate = patch.dueDate
  if (patch.estimate !== undefined) next.estimate = patch.estimate
  return next
}

function customId(field: TaskField) {
  return field.startsWith('field:') ? field.slice('field:'.length) : null
}

/** A field's name in a sentence; a custom field is "field". */
export function fieldName(field: TaskField) {
  return customId(field) === null ? FIELD_NAMES[field as BuiltInField] : 'field'
}

/** A field's value as text, to compare what someone saw with what the task holds now. */
export function fieldValue(item: WorkItem, field: TaskField): string {
  const custom = customId(field)
  if (custom !== null) return JSON.stringify(item.fieldValues[custom] ?? null)
  switch (field as BuiltInField) {
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
    case 'delegate':
      return item.delegate?.actorId ?? ''
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
  const custom = customId(field)
  if (custom !== null) return JSON.stringify(patch.fieldValues?.[custom] ?? null)
  switch (field as BuiltInField) {
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
    case 'delegate':
      return patch.delegateActorId ?? ''
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
  /** Waiting for the saves ahead of it, being sent, kept on this device until the connection is back, or stopped for the person to decide. */
  state: 'waiting' | 'saving' | 'queued' | 'conflict' | 'failed'
  theirs: WorkItem | null
  message: string | null
  /**
   * A suggestion's request id and the version it was made against. Trying
   * the same change again on the same version reuses the id, so a proposal
   * whose answer was lost is answered again rather than made twice.
   */
  sent: { requestId: string; version: number } | null
  /** The request id it waits under, while queued. */
  outbound?: string
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
 * Saves each field of a task as soon as it changes, one save at a time,
 * through the shared save queue, so Undo can take the last one back.
 *
 * Every change remembers what the person saw before making it. If someone
 * else changed that same field in the meantime, nothing is overwritten: the
 * change stops as a conflict and keeps the person's text until they choose
 * "Keep yours" or "Use theirs". A version conflict caused by a change to
 * another field is retried against the newer task on its own.
 */
export function useTaskSave(item: WorkItem, { suggest = false }: { suggest?: boolean } = {}) {
  const update = useUpdateWorkItem()
  const queueable = useQueueWorkItemUpdate()
  const queryClient = useQueryClient()
  const [problem, setProblem] = useState<SaveProblem | null>(null)
  // The saves live in a ref, so the queue always reads the latest; the page renders from the copy in state.
  const entries = useRef(new Map<TaskField, Entry>())
  const [shown, setShown] = useState<ReadonlyMap<TaskField, Entry>>(() => new Map())
  const lastId = useRef(0)
  // Each field's last suggestion that became a proposal: the value it
  // proposed, what the task held then, and the task's version. A draft
  // opened while it was on its way started from the proposed value.
  const proposed = useRef(new Map<TaskField, { mine: string; base: string; version: number }>())
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
        if (suggest) {
          await propose(id, entry, task)
          return
        }
        const input = {
          projectId: task.projectId,
          workItemId: task.id,
          idempotencyKey: crypto.randomUUID(),
          patch: { expectedVersion: task.version, ...entry.patch },
        }
        const saved = WAITS_OFFLINE.has(entry.field)
          ? await queueable.mutateAsync({ ...input, label: `The ${fieldName(entry.field)} of ${task.key}` })
          : await update.mutateAsync(input)
        if (saved === null) {
          // Offline: it's kept on this device and goes when the connection is back. Meanwhile the page shows it.
          const key = queryKeys.workItem(projectId, taskId)
          const cached = queryClient.getQueryData<WorkItem>(key)
          if (cached) queryClient.setQueryData(key, showing(cached, entry.patch))
          change(id, { state: 'queued', outbound: input.idempotencyKey })
          return
        }
        // A change queued behind this one started from what this one sent,
        // but the server may have stored it written its own way (a URL gains
        // https://). That's still the person's own change, not someone
        // else's, so the queued one starts from what the task holds now.
        const holds = fieldValue(saved ?? current(), entry.field)
        const queued = entries.current.get(entry.field)
        if (queued && queued.id !== id && entry.mine !== null && queued.base === entry.mine && holds !== entry.mine) {
          entries.current.set(entry.field, { ...queued, base: holds })
        }
        change(id, null)
        const undo = undoableTaskChange(`Changed the ${fieldName(entry.field)} of ${task.key}`, task, entry.patch)
        if (undo) remember(undo)
        return
      } catch (error) {
        // A proposal's conflict carries no task; read it again, and the next
        // try is a new request against the newer version.
        const proposalConflict = suggest && isApiError(error) && error.code === 'conflict'
        if (proposalConflict) change(id, { sent: null })
        const theirs = conflictCurrent(error) ?? (proposalConflict ? await getWorkItem(task.projectId, task.id).catch(() => null) : null)
        if (!theirs) {
          change(id, { state: 'failed', message: problemFor(error, fieldName(entry.field)).message })
          return
        }
        // The task moved on. The next round checks whether this field is one of the things that changed.
        takeNewer(theirs)
      }
    }
    change(id, { state: 'failed', message: 'The task kept changing while this saved. Try again.' })
  }

  /**
   * Someone who only suggests (T1.41): the change goes to `task_update`,
   * which keeps it as a proposal for someone who edits the task. The task
   * stays as it was, so the field shows its value again and a toast says
   * where the change went; it is no one's to undo.
   */
  const propose = async (id: number, entry: Entry, task: WorkItem) => {
    const name = fieldName(entry.field)
    // An earlier try whose answer was lost goes again as it was, with its
    // request id and version, so the server answers with the proposal it
    // may already have made; only a refusal for the version moves it on.
    const sent = find(id)?.sent ?? null
    const version = sent?.version ?? task.version
    const input = suggestedUpdate(task.id, version, entry.patch)
    if (!input) {
      change(id, { state: 'failed', message: `Only someone who edits this task can change its ${name}.` })
      return
    }
    const requestId = sent?.requestId ?? newRequestId()
    change(id, { sent: { requestId, version } })
    const written = await callOp('task_update', input, { requestId })
    // A suggestion leaves the task as it was, so a change to the same field
    // queued behind this one starts from what this one started from (the
    // task's value when it was sent), never from a live change since. A
    // change that landed (their own task) already left the right base: it.
    const queued = entries.current.get(entry.field)
    if (written.proposalId && queued && queued.id !== id && entry.mine !== null && queued.base === entry.mine) {
      entries.current.set(entry.field, { ...queued, base: entry.base })
    }
    if (written.proposalId && entry.mine !== null) {
      proposed.current.set(entry.field, { mine: entry.mine, base: entry.base, version: written.version })
    }
    change(id, null)
    if (written.proposalId) {
      showToast({ tone: 'success', title: `Suggested a new ${name}`, description: 'Someone who edits this task accepts or rejects it.' })
    } else {
      // Their own task, which they may change: it landed. As on the direct
      // path, a change queued behind it starts from what the server stored,
      // which may be written its own way (a URL gains https://).
      await queryClient.invalidateQueries({ queryKey: queryKeys.workItem(task.projectId, task.id) })
      // Only the version this write made is known to hold its value; a newer
      // one may be someone else's change, which must still stop it.
      const now = current()
      const holds = fieldValue(now, entry.field)
      const next = entries.current.get(entry.field)
      if (now.version === written.version && next && next.id !== id && entry.mine !== null && next.base === entry.mine && holds !== entry.mine) {
        entries.current.set(entry.field, { ...next, base: holds })
      }
    }
  }
  // A queued change is done once it's gone from the device; one the server refused comes back as a failed save.
  const waiting = useOutbound(`task:${taskId}`)
  useEffect(() => {
    let changed = false
    for (const entry of [...entries.current.values()]) {
      if (entry.state !== 'queued' || !entry.outbound) continue
      const kept = waiting.find((candidate) => candidate.id === entry.outbound)
      if (kept && !kept.refused) continue
      changed = true
      if (!kept) {
        entries.current.delete(entry.field)
        continue
      }
      entries.current.set(entry.field, { ...entry, state: 'failed', message: kept.refused ?? null, outbound: undefined })
      void discardOutbound(kept.id)
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItem(projectId, taskId) })
    }
    if (changed) setShown(new Map(entries.current))
  }, [projectId, queryClient, shown, taskId, waiting])

  // One task's saves go one at a time, with the list's and board's (T1.27).
  const enqueue = (id: number) => {
    void inOrder(`task:${taskId}`, () => attempt(id).catch(() => change(id, { state: 'failed', message: 'That didn’t save. Try again.' })))
  }

  /** What the person sees for a field right now: their own change on its way, or the task's value. */
  const baseFor = useCallback(
    (field: TaskField) => {
      const entry = entries.current.get(field)
      if (entry && entry.mine !== null && (entry.state === 'waiting' || entry.state === 'saving' || entry.state === 'queued')) return entry.mine
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
    let from = previous?.state === 'waiting' ? previous.base : (base ?? baseFor(field))
    // A draft opened while a suggestion was on its way started from that
    // suggestion, which never reached the task: while the task is still the
    // version it answered with, the draft starts from what it held then.
    const earlier = proposed.current.get(field)
    const now = current()
    if (earlier && from === earlier.mine && now.version === earlier.version && fieldValue(now, field) === earlier.base) from = earlier.base
    lastId.current += 1
    const entry: Entry = { id: lastId.current, field, patch, mine: patchValue(field, patch), base: from, state: 'waiting', theirs: null, message: null, sent: null }
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
      // A changed patch is a new request; the same one again (a retry from
      // the description's own button) keeps the request id.
      // "Keep yours" is a new request against the task as it is now.
      sent: overTheirs || (patch && JSON.stringify(patch) !== JSON.stringify(entry.patch)) ? null : entry.sent,
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
    return { state: entry.state === 'waiting' || entry.state === 'queued' ? 'saving' : entry.state, text, theirs: entry.theirs, message: entry.message }
  }

  const values = [...shown.values()]
  /** Stopped saves of the fields that have no place of their own to say so (all but the title and description). */
  const stopped = values
    .filter((entry) => (entry.state === 'conflict' || entry.state === 'failed') && entry.field !== 'title' && entry.field !== 'description')
    .map((entry) => ({ field: entry.field, state: entry.state as 'conflict' | 'failed', theirs: entry.theirs, message: entry.message }))
  const saving = values.some((entry) => entry.state === 'waiting' || entry.state === 'saving')
  /** Changes kept on this device until the connection is back. */
  const queued = values.some((entry) => entry.state === 'queued')

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
    queued,
    problem,
    fail,
    clearProblem: () => setProblem(null),
  }
}

export type TaskSave = ReturnType<typeof useTaskSave>
