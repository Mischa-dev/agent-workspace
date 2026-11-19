import { useMutation } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { describeError } from '../ui'
import { normalizeWorkItem, updateWorkItem } from './api'
import type { PickResult } from './FieldPicker'
import { bulkUpdateWorkItems } from './planningApi'
import { useInvalidateWorkItems } from './planningHooks'
import type { BulkChanges, BulkProblem, WorkItem, WorkItemPatch } from './types'

/** The PATCH fields for a picker result. */
export function pickToPatch(result: PickResult): Omit<WorkItemPatch, 'expectedVersion'> {
  switch (result.field) {
    case 'status':
      return { statusId: result.status.id }
    case 'priority':
      return { priority: result.priority }
    case 'assignee':
      return { assigneeActorId: result.actor?.id ?? null }
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
  if (result.field === 'type') return null
  return pickToPatch(result) as BulkChanges
}

/**
 * The task the server sent back with a version conflict, if it did: the old
 * routes' `version_conflict` carries it as `details.current`, an operation's
 * `conflict` (reason `version`) as `details.current.task`.
 */
export function conflictCurrent(error: unknown): WorkItem | null {
  if (!isApiError(error)) return null
  const details = error.details as { current?: unknown; reason?: unknown } | null | undefined
  if (!details || typeof details !== 'object' || !details.current) return null
  if (error.code === 'version_conflict') return normalizeWorkItem(details.current)
  if (error.code !== 'conflict' || details.reason !== 'version') return null
  const current = details.current as { task?: unknown }
  return current.task ? normalizeWorkItem(current.task) : null
}

export function bulkProblems(error: unknown): BulkProblem[] {
  if (!isApiError(error) || error.code !== 'bulk_rejected') return []
  const details = error.details
  const problems = details && typeof details === 'object' && 'problems' in details ? (details as { problems: unknown }).problems : []
  return (Array.isArray(problems) ? problems : []).flatMap((value) => {
    if (!value || typeof value !== 'object') return []
    const problem = value as Record<string, unknown>
    return [
      {
        id: typeof problem.id === 'string' ? problem.id : '',
        key: typeof problem.key === 'string' ? problem.key : null,
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

/**
 * Applies one picker result to one task (PATCH) or several (bulk, all or
 * nothing). Lists refetch afterwards either way.
 */
export function useApplyPick() {
  const invalidate = useInvalidateWorkItems()
  return useMutation({
    mutationFn: async ({ projectId, result, targets }: { projectId: string; result: PickResult; targets: EditTarget[] }) => {
      if (targets.length === 1 || result.field === 'type') {
        for (const target of targets) {
          await updateWorkItem({
            projectId: target.projectId,
            workItemId: target.id,
            idempotencyKey: crypto.randomUUID(),
            patch: { expectedVersion: target.version, ...pickToPatch(result) },
          })
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
    },
    onSettled: (_result, _error, input) => void invalidate(input.projectId),
  })
}

/** A claim or release that was refused says why in words, such as who holds it and until when. */
export function claimErrorMessage(error: unknown, doing: string): string {
  return isApiError(error) && error.message ? error.message : `${doing} didn’t work. Try again.`
}
