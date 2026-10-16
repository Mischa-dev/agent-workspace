import { useEffect, useState } from 'react'

import type { PickerField } from './FieldPicker'
import type { WorkItemWorkflow } from './types'

export const PICKER_LABELS: Record<PickerField, string> = {
  status: 'Change status',
  priority: 'Change priority',
  assignee: 'Assign to',
  labels: 'Change labels',
  milestone: 'Set milestone',
  type: 'Change type',
}

/** The option value meaning "none" (unassigned, no milestone). */
export const NONE = '__none'

export function useDebouncedValue<T>(value: T, delay: number) {
  const [debounced, setDebounced] = useState(value)
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delay)
    return () => window.clearTimeout(timer)
  }, [delay, value])
  return debounced
}

/** Statuses a task in `fromStatusId` may move to, its own included. */
export function allowedStatuses(workflow: WorkItemWorkflow, fromStatusId: string | undefined) {
  if (!fromStatusId) return workflow.statuses
  const allowed = new Set(workflow.transitions.filter((transition) => transition.fromStatusId === fromStatusId).map((transition) => transition.toStatusId))
  allowed.add(fromStatusId)
  return workflow.statuses.filter((status) => allowed.has(status.id))
}
