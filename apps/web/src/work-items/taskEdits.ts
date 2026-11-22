import { useMutation } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { describeError } from '../ui'
import { normalizeWorkItem, writeWorkItem } from './api'
import type { PickResult } from './FieldPicker'
import { bulkUpdateWorkItems } from './planningApi'
import { useInvalidateWorkItems } from './planningHooks'
import type { BulkChanges, BulkProblem, WorkItem, WorkItemCard, WorkItemPatch } from './types'
import { forget, inOrder, remember, type Undoable } from '../views/saveQueue'
import { undoableTaskChange } from './taskUndo'

/** The PATCH fields for a picker result. */
export function pickToPatch(result: PickResult): Omit<WorkItemPatch, 'expectedVersion'> {
  switch (result.field) {
    case 'status':
      return { statusId: result.status.id }
    case 'priority':
      return { priority: result.priority }
    case 'assignee':
      return { assigneeActorId: result.actor?.id ?? null }
    case 'delegate':
      return { delegateActorId: result.actor?.id ?? null }
    case 'milestone':
      return { milestoneId: result.milestone?.id ?? null }
    case 'type':
      return { typeId: result.type.id }
    case 'labels':
      return {
        ...(result.add.length ? { addLabelIds: result.add } : {}),
        ...(result.remove.length ? { removeLabelIds: result.remove } : {}),
      }
  }
}

/** The bulk changes for a picker result, or null where bulk can't express it (type). */
export function pickToBulk(result: PickResult): BulkChanges | null {
  if (result.field === 'type' || result.field === 'delegate') return null
  return pickToPatch(result) as BulkChanges
}

/**
 * The whole task as it is now, from a version conflict, if the refusal has it:
 * the old routes' `version_conflict` carries it as `details.current`; a task
 * operation's `conflict` (reason `version`) has it under `details.current.task`
 * once `api.ts` has read it (the server's own sends only a few fields).
 */
export function conflictCurrent(error: unknown): WorkItem | null {
  if (!isApiError(error)) return null
  const details = error.details as { current?: unknown; reason?: unknown } | null | undefined
  if (!details || typeof details !== 'object' || !details.current) return null
  if (error.code === 'version_conflict') return normalizeWorkItem(details.current)
  if (error.code !== 'conflict' || details.reason !== 'version') return null
  const current = details.current as { task?: unknown; whole?: unknown }
  return current.task && current.whole === true ? normalizeWorkItem(current.task) : null
}

export function bulkProblems(error: unknown): BulkProblem[] {
  // task_update_many refuses the whole change as a conflict whose reason is
  // "rejected", with one problem per task under details.current.
  if (!isApiError(error) || error.code !== 'conflict') return []
  const details = (error.details ?? {}) as { reason?: unknown; current?: { problems?: unknown } }
  if (details.reason !== 'rejected') return []
  const problems = details.current?.problems
  return (Array.isArray(problems) ? problems : []).flatMap((value) => {
    if (!value || typeof value !== 'object') return []
    const problem = value as Record<string, unknown>
    return [
      {
        id: typeof problem.id === 'string' ? problem.id : '',
        key: typeof problem.task === 'string' ? problem.task : null,
        code: typeof problem.code === 'string' ? problem.code : '',
        message: typeof problem.message === 'string' ? problem.message : '',
      },
    ]
  })
}

/** A plain sentence for a failed task write. */
export function editErrorMessage(error: unknown, subject = 'That change'): string {
  if (conflictCurrent(error)) return `Someone changed this task first. It’s up to date now; make the change again if it still applies.`
  const problems = bulkProblems(error)
  if (problems.length) {
    const first = problems
      .slice(0, 3)
      .map((problem) => `${problem.key ?? 'A task'}: ${problem.message}`)
      .join(' ')
    return `Nothing was changed. ${first}${problems.length > 3 ? ` And ${problems.length - 3} more.` : ''}`
  }
  if (isApiError(error)) return error.message || `${subject} didn’t save. Try again.`
  return `${subject} didn’t save. ${describeError(error).message}`
}

export type EditTarget = { id: string; projectId: string; version: number; key: string }

/** Whether a target is a whole card, which says what a change replaces, so Undo can take it back. */
const isCard = (target: EditTarget | WorkItemCard): target is WorkItemCard => 'status' in target

/**
 * Applies one picker result to one task (PATCH) or several (bulk, all or
 * nothing). Lists refetch afterwards either way.
 */
export function useApplyPick() {
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: async ({ projectId, result, targets }: { projectId: string; result: PickResult; targets: Array<EditTarget | WorkItemCard> }) => {
      const patch = pickToPatch(result)
      // Undo takes the change back on each task the picker changed (T1.27).
      const undos = targets.flatMap((target) => (isCard(target) ? (undoableTaskChange(`Changed the ${result.field} of ${target.key}`, target, patch) ?? []) : []))
      const undo: Undoable | null =
        undos.length === targets.length && undos.length
          ? {
              label: undos.length === 1 ? undos[0].label : `Changed the ${result.field} of ${undos.length} tasks`,
              undo: async (client) => {
                const failed = (await Promise.allSettled(undos.map((one) => one.undo(client)))).filter((outcome) => outcome.status === 'rejected')
                if (failed.length === 1) throw (failed[0] as PromiseRejectedResult).reason
                if (failed.length) throw new Error(`${failed.length} of them changed since, so they weren’t undone.`)
              },
            }
          : null
      if (targets.length === 1 || result.field === 'type') {
        const saves = (async () => {
          for (const target of targets) {
            await inOrder(`task:${target.id}`, () =>
              writeWorkItem({
                projectId: target.projectId,
                workItemId: target.id,
                idempotencyKey: crypto.randomUUID(),
                patch: { expectedVersion: target.version, ...patch },
              }),
            )
          }
        })()
        // Undo can take it back from the moment it's made: it waits for the
        // save itself and does nothing if the save failed (another editor's
        // value that happens to match is never undone), and a failed save
        // forgets it.
        const afterSave: Undoable | null = undo && {
          label: undo.label,
          undo: async (client) => {
            try {
              await saves
            } catch {
              throw new Error('That change didn’t save, so there was nothing to undo.')
            }
            await undo.undo(client)
          },
        }
        if (afterSave) remember(afterSave)
        try {
          await saves
        } catch (error) {
          if (afterSave) forget(afterSave)
          throw error
        }
        return
      }
      const changes = pickToBulk(result)
      if (!changes) return
      await bulkUpdateWorkItems({
        projectId,
        items: targets.map((target) => ({ id: target.id, expectedVersion: target.version })),
        changes,
        idempotencyKey: crypto.randomUUID(),
      })
      if (undo) remember(undo)
    },
    onSettled: (_result, _error, input) => void invalidate(input.projectId),
  })
}

/** A claim or release that was refused says why in words, such as who holds it and until when. */
export function claimErrorMessage(error: unknown, doing: string): string {
  return isApiError(error) && error.message ? error.message : `${doing} didn’t work. Try again.`
}
