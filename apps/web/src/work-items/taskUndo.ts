import { queryKeys } from '../api/queryKeys'
import { inOrder, type Undoable } from '../views/saveQueue'
import { getWorkItem, updateWorkItem } from './api'
import type { WorkItem, WorkItemCard, WorkItemPatch } from './types'

type Patch = Omit<WorkItemPatch, 'expectedVersion'>

/** A task as a list or board holds it, or opened. */
type Before = WorkItem | WorkItemCard

const sameIds = (ids: readonly string[] | undefined) => ids ?? []

/**
 * The patch that puts back what `patch` replaced on `before`, or null when
 * some part of it can't be put back from what `before` holds (a delegate
 * picked on a card, which doesn't show one).
 */
export function inverseOf(before: Before, patch: Patch): Patch | null {
  const inverse: Patch = {}
  const labels = new Set(before.labels.map((label) => label.id))
  for (const key of Object.keys(patch) as Array<keyof Patch>) {
    switch (key) {
      case 'title':
        inverse.title = before.title
        break
      case 'description':
        if (!('description' in before)) return null
        inverse.description = before.description
        break
      case 'typeId':
        inverse.typeId = before.itemType.id
        break
      case 'statusId':
        inverse.statusId = before.status.id
        break
      case 'priority':
        inverse.priority = before.priority
        break
      case 'assigneeActorId':
        inverse.assigneeActorId = before.assignee?.actorId ?? null
        break
      case 'delegateActorId':
        if (!('delegate' in before)) return null
        inverse.delegateActorId = before.delegate?.actorId ?? null
        break
      case 'parentWorkItemId':
        inverse.parentWorkItemId = 'parent' in before ? (before.parent?.id ?? null) : before.parentId
        break
      case 'milestoneId':
        inverse.milestoneId = before.milestone?.id ?? null
        break
      case 'startDate':
        inverse.startDate = before.startDate
        break
      case 'dueDate':
        inverse.dueDate = before.dueDate
        break
      case 'estimate':
        inverse.estimate = before.estimate
        break
      case 'blockedReason':
        inverse.blockedReason = 'blocked' in before ? (before.blocked?.reason ?? null) : before.blockedReason
        break
      case 'addLabelIds': {
        const added = sameIds(patch.addLabelIds).filter((id) => !labels.has(id))
        if (added.length) inverse.removeLabelIds = [...(inverse.removeLabelIds ?? []), ...added]
        break
      }
      case 'removeLabelIds': {
        const removed = sameIds(patch.removeLabelIds).filter((id) => labels.has(id))
        if (removed.length) inverse.addLabelIds = [...(inverse.addLabelIds ?? []), ...removed]
        break
      }
      case 'fieldValues':
        inverse.fieldValues = Object.fromEntries(Object.keys(patch.fieldValues ?? {}).map((id) => [id, before.fieldValues[id] ?? null]))
        break
    }
  }
  return inverse
}

const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null)

/** Whether `task` still holds what `patch` set, so taking it back undoes nothing of anyone else's. */
export function stillAsSet(task: WorkItem, patch: Patch): boolean {
  const now = inverseOf(task, patch)
  if (!now) return false
  const labels = new Set(task.labels.map((label) => label.id))
  return (Object.keys(patch) as Array<keyof Patch>).every((key) => {
    switch (key) {
      case 'addLabelIds':
        return sameIds(patch.addLabelIds).every((id) => labels.has(id))
      case 'removeLabelIds':
        return sameIds(patch.removeLabelIds).every((id) => !labels.has(id))
      case 'fieldValues':
        return Object.entries(patch.fieldValues ?? {}).every(([id, value]) => same(now.fieldValues?.[id], value))
      default:
        return same(now[key], patch[key])
    }
  })
}

/**
 * A change to a task that Undo can take back (T1.27), or null when it
 * can't be: it reads the task again, and puts the old values back only
 * while the task still holds what the change set.
 */
export function undoableTaskChange(label: string, before: Before, patch: Patch): Undoable | null {
  const inverse = inverseOf(before, patch)
  if (!inverse || !Object.keys(inverse).length) return null
  const { id, projectId, key } = before
  return {
    label,
    undo: async (client) => {
      // Once the change itself has saved.
      await inOrder(`task:${id}`, async () => undefined)
      const now = await getWorkItem(projectId, id)
      if (!stillAsSet(now, patch)) throw new Error(`${key} changed since, so it wasn’t undone.`)
      const saved = await inOrder(`task:${id}`, () =>
        updateWorkItem({ projectId, workItemId: id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: now.version, ...inverse } }),
      )
      client.setQueryData(queryKeys.workItem(projectId, id), saved)
      await client.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    },
  }
}
