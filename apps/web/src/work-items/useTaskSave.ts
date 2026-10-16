import { useCallback, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { isApiError } from '../api/errors'
import { useUpdateWorkItem } from './hooks'
import { conflictCurrent, editErrorMessage } from './taskEdits'
import type { WorkItem, WorkItemPatch } from './types'

export type SaveProblem = { field: string; message: string; conflict: boolean }

/** "Someone else changed…" for a conflict; the server's message otherwise. */
export function problemFor(error: unknown, field: string): SaveProblem {
  const current = conflictCurrent(error)
  if (current) {
    return {
      field,
      conflict: true,
      message: `Someone else changed this work item, so your change to ${field.toLocaleLowerCase()} wasn’t saved. It now shows the latest version; make the change again if it still applies.`,
    }
  }
  if (isApiError(error) && error.code === 'dependency_cycle') {
    const details = error.details as { path?: unknown } | undefined
    const path = Array.isArray(details?.path) ? details.path.filter((entry): entry is string => typeof entry === 'string') : []
    return { field, conflict: false, message: path.length ? `That would make a loop: ${path.join(' → ')}.` : error.message }
  }
  return { field, conflict: false, message: editErrorMessage(error, `The ${field.toLocaleLowerCase()} change`) }
}

/**
 * Saves one field of a task as soon as it changes. A version conflict loads
 * the latest task into the cache, so every field shows it, and says so.
 */
export function useTaskSave(item: WorkItem) {
  const update = useUpdateWorkItem()
  const queryClient = useQueryClient()
  const [problem, setProblem] = useState<SaveProblem | null>(null)
  const [saving, setSaving] = useState<string | null>(null)

  const takeConflict = useCallback(
    (error: unknown) => {
      const current = conflictCurrent(error)
      if (current) queryClient.setQueryData(queryKeys.workItem(current.projectId, current.id), current)
    },
    [queryClient],
  )

  const save = useCallback(
    (field: string, patch: Omit<WorkItemPatch, 'expectedVersion'>, onDone?: (saved: boolean) => void) => {
      setProblem(null)
      setSaving(field)
      update.mutate(
        { projectId: item.projectId, workItemId: item.id, idempotencyKey: crypto.randomUUID(), patch: { expectedVersion: item.version, ...patch } },
        {
          onSuccess: () => onDone?.(true),
          onError: (error) => {
            takeConflict(error)
            setProblem(problemFor(error, field))
            onDone?.(false)
          },
          onSettled: () => setSaving(null),
        },
      )
    },
    [item.id, item.projectId, item.version, takeConflict, update],
  )

  /** For other task writes (dependencies, relations): the same conflict handling. */
  const fail = useCallback(
    (field: string, error: unknown) => {
      takeConflict(error)
      setProblem(problemFor(error, field))
    },
    [takeConflict],
  )

  return { save, saving, problem, fail, clearProblem: () => setProblem(null) }
}

export type TaskSave = ReturnType<typeof useTaskSave>
