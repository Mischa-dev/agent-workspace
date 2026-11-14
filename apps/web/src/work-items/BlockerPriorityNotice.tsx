import { Fragment } from 'react'
import { useMutation } from '@tanstack/react-query'

import { Button, countLabel, showToast } from '../ui'
import { getWorkItem } from './api'
import { priorityLabel, priorityRank } from './constants'
import { bulkUpdateWorkItems } from './planningApi'
import { useInvalidateWorkItems } from './planningHooks'
import { editErrorMessage } from './taskEdits'
import { TaskNotice } from './TaskNotice'
import type { TaskRef, WorkItem, WorkItemPriority } from './types'

/**
 * The blockers still open on a task, split by whether they sit below a
 * priority (and so could be raised to it) or already at or above it (and so
 * stay as they are: raising never moves a task down).
 */
function blockersAgainst(blockedBy: TaskRef[], priority: WorkItemPriority) {
  const open = blockedBy.filter((task) => !task.finished && !task.archived)
  return {
    lower: open.filter((task) => priorityRank(task.priority) < priorityRank(priority)),
    atOrAbove: open.filter((task) => priorityRank(task.priority) >= priorityRank(priority)),
  }
}

/**
 * Raises each blocker that is still below the priority, reading each one
 * fresh first so nothing that moved up in the meantime is brought down.
 * Blockers in one project change together, all or nothing.
 */
async function raiseBlockers(blockers: TaskRef[], priority: WorkItemPriority) {
  const fresh = await Promise.all(blockers.map((task) => getWorkItem(task.projectId, task.id)))
  const finished = (task: WorkItem) => task.status.semanticCategory === 'completed' || task.status.semanticCategory === 'canceled'
  const due = fresh.filter((task) => !task.archivedAt && !finished(task) && priorityRank(task.priority) < priorityRank(priority))
  const byProject = new Map<string, WorkItem[]>()
  for (const task of due) byProject.set(task.projectId, [...(byProject.get(task.projectId) ?? []), task])
  for (const [projectId, tasks] of byProject) {
    await bulkUpdateWorkItems({
      projectId,
      items: tasks.map((task) => ({ id: task.id, expectedVersion: task.version })),
      changes: { priority },
      idempotencyKey: crypto.randomUUID(),
    })
  }
  return { raised: due, projects: [...byProject.keys()] }
}

/** Task keys in a sentence ("AW-2, AW-3 and AW-4"), each kept on one line. */
function keys(tasks: TaskRef[]) {
  return tasks.map((task, index) => (
    <Fragment key={task.id}>
      {index === 0 ? '' : index === tasks.length - 1 ? ' and ' : ', '}
      <span className="aw-tasknote__key">{task.key}</span>
    </Fragment>
  ))
}

/** "is low priority", or "has no priority". */
function standing(priority: WorkItemPriority) {
  return priority === 'no_priority' ? 'has no priority' : `is ${priorityLabel(priority).toLowerCase()} priority`
}

/** The blockers that won't change, so it's plain that raising never lowers one. */
function stays(tasks: TaskRef[], label: string) {
  if (tasks.length === 1)
    return (
      <>
        {keys(tasks)} is already {priorityLabel(tasks[0].priority).toLowerCase()}, so it stays as it is.
      </>
    )
  return (
    <>
      {keys(tasks)} are already {label} or higher, so they stay as they are.
    </>
  )
}

/**
 * After someone raises a task's priority, offers to raise the blockers that
 * would otherwise hold it up at a lower one (AWD-77). Blockers already as
 * urgent or more are named as staying put, so it's clear nothing moves down.
 */
export function BlockerPriorityNotice({ item, onDone, priority }: { item: WorkItem; onDone: () => void; priority: WorkItemPriority }) {
  const invalidate = useInvalidateWorkItems()
  const { lower, atOrAbove } = blockersAgainst(item.blockedBy, priority)
  const raise = useMutation({
    mutationFn: () => raiseBlockers(lower, priority),
    onSuccess: ({ raised }) => {
      onDone()
      if (raised.length)
        showToast({
          title: `Raised ${countLabel(raised.length, 'blocker')} to ${priorityLabel(priority)}`,
          description: raised.map((task) => task.key).join(', '),
          tone: 'success',
        })
    },
    onSettled: (result) => {
      for (const projectId of new Set([item.projectId, ...(result?.projects ?? [])])) void invalidate(projectId)
    },
  })
  // The priority didn't stick (a failed save, or someone changed it since), or there's nothing below it.
  if (item.priority !== priority || !lower.length) return null

  const label = priorityLabel(priority).toLowerCase()
  const one = lower.length === 1
  return (
    <TaskNotice
      actions={
        <>
          <Button busy={raise.isPending} onClick={() => raise.mutate()} size="sm">
            Raise to {priorityLabel(priority)}
          </Button>
          <Button disabled={raise.isPending} onClick={onDone} size="sm" variant="ghost">
            Not now
          </Button>
        </>
      }
      icon="arrow-up"
      live="status"
      title={
        one ? (
          <>
            {keys(lower)} blocks this and {standing(lower[0].priority)}.
          </>
        ) : (
          <>{keys(lower)} block this at a lower priority.</>
        )
      }
      tone="muted"
    >
      <p>
        Raise {one ? 'it' : 'them'} to {label} too, so {one ? 'it doesn’t' : 'they don’t'} hold this up?
        {atOrAbove.length ? <> {stays(atOrAbove, label)}</> : null}
      </p>
      {raise.isError ? <p role="alert">{editErrorMessage(raise.error, 'Raising the blockers')}</p> : null}
    </TaskNotice>
  )
}
