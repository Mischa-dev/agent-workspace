import type { WorkItemPriority } from './types'

export const workItemPriorities: Array<{
  value: WorkItemPriority
  label: string
}> = [
  { value: 'no_priority', label: 'No priority' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'urgent', label: 'Urgent' },
]

export function priorityLabel(priority: WorkItemPriority) {
  return workItemPriorities.find((candidate) => candidate.value === priority)?.label ?? 'No priority'
}

/** How urgent a priority is, for comparing two: no priority is the least. */
export function priorityRank(priority: WorkItemPriority) {
  return workItemPriorities.findIndex((entry) => entry.value === priority)
}
