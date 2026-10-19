import { priorityLabel } from './constants'
import type { FilterValues } from './listModel'
import type { Milestone, ProjectLabel, WorkItemPriority, WorkItemWorkflow } from './types'
import { formatDateLabel } from '../ui/dateModel'

/** The filters a person adds from the Filter menu, in the order the menu lists them. */
export type FilterField =
  | 'status'
  | 'category'
  | 'priority'
  | 'assignee'
  | 'labels'
  | 'milestone'
  | 'parent'
  | 'type'
  | 'blocked'
  | 'ready'
  | 'claimed'
  | 'due'
  | 'done'
  | 'archived'

export const FILTER_FIELDS: Array<{ field: FilterField; label: string; keys: Array<keyof FilterValues> }> = [
  { field: 'status', label: 'Status', keys: ['statusId'] },
  { field: 'category', label: 'Status group', keys: ['semanticCategory'] },
  { field: 'priority', label: 'Priority', keys: ['priority'] },
  { field: 'assignee', label: 'Assignee', keys: ['assigneeActorId', 'unassigned'] },
  { field: 'labels', label: 'Labels', keys: ['labelIds'] },
  { field: 'milestone', label: 'Milestone', keys: ['milestoneId'] },
  { field: 'parent', label: 'Parent', keys: ['parentId'] },
  { field: 'type', label: 'Type', keys: ['typeId'] },
  { field: 'blocked', label: 'Blocked', keys: ['blocked'] },
  { field: 'ready', label: 'Ready to start', keys: ['ready'] },
  { field: 'claimed', label: 'Claimed by an agent', keys: ['claimed'] },
  { field: 'due', label: 'Due date', keys: ['overdue', 'dueBefore', 'dueAfter'] },
  { field: 'done', label: 'Done hidden', keys: ['includeDone'] },
  { field: 'archived', label: 'Archived', keys: ['archived'] },
]

export const CATEGORY_LABELS: Record<string, string> = {
  backlog: 'Backlog',
  unstarted: 'Not started',
  started: 'In progress',
  review: 'In review',
  verification: 'Verifying',
  completed: 'Done',
  canceled: 'Canceled',
}

export type FilterLookups = {
  workflow?: WorkItemWorkflow
  labels?: ProjectLabel[]
  milestones?: Milestone[]
  assigneeName?: string | null
  parentKey?: string | null
}

function formatDay(value: string) {
  return formatDateLabel(value) || value
}

/** The words on a filter chip, such as "Status is In progress". Null when the field isn't set. */
export function chipText(field: FilterField, filters: FilterValues, lookups: FilterLookups): string | null {
  switch (field) {
    case 'status': {
      if (!filters.statusId) return null
      const status = lookups.workflow?.statuses.find((entry) => entry.id === filters.statusId)
      return `Status is ${status?.name ?? 'a removed status'}`
    }
    case 'category':
      return filters.semanticCategory ? `Status group is ${CATEGORY_LABELS[filters.semanticCategory] ?? filters.semanticCategory}` : null
    case 'priority':
      return filters.priority ? `Priority is ${priorityLabel(filters.priority as WorkItemPriority)}` : null
    case 'assignee':
      if (filters.unassigned) return 'Unassigned'
      return filters.assigneeActorId ? `Assigned to ${lookups.assigneeName ?? 'someone'}` : null
    case 'labels': {
      if (!filters.labelIds) return null
      if (filters.labelIds === 'none') return 'No labels'
      const names = filters.labelIds.split(',').map((id) => lookups.labels?.find((label) => label.id === id)?.name ?? 'a removed label')
      return `Labels include ${names.join(' and ')}`
    }
    case 'milestone': {
      if (!filters.milestoneId) return null
      if (filters.milestoneId === 'none') return 'No milestone'
      const milestone = lookups.milestones?.find((entry) => entry.id === filters.milestoneId)
      return `Milestone is ${milestone?.name ?? 'a removed milestone'}`
    }
    case 'parent':
      if (!filters.parentId) return null
      return filters.parentId === 'none' ? 'Top-level only' : `Subtasks of ${lookups.parentKey ?? 'a task'}`
    case 'type': {
      if (!filters.typeId) return null
      const type = lookups.workflow?.types.find((entry) => entry.id === filters.typeId)
      return `Type is ${type?.name ?? 'a removed type'}`
    }
    case 'blocked':
      return filters.blocked === 'true' ? 'Blocked' : filters.blocked === 'false' ? 'Not blocked' : null
    case 'ready':
      return filters.ready ? 'Ready to start' : null
    case 'claimed':
      return filters.claimed === 'true' ? 'Claimed' : filters.claimed === 'false' ? 'Not claimed' : null
    case 'due': {
      const parts: string[] = []
      if (filters.overdue) parts.push('Overdue')
      if (filters.dueAfter && filters.dueBefore) parts.push(`Due ${formatDay(filters.dueAfter)} to ${formatDay(filters.dueBefore)}`)
      else if (filters.dueAfter) parts.push(`Due on or after ${formatDay(filters.dueAfter)}`)
      else if (filters.dueBefore) parts.push(`Due by ${formatDay(filters.dueBefore)}`)
      return parts.length ? parts.join(', ') : null
    }
    case 'done':
      return filters.includeDone === 'false' ? 'Done hidden' : null
    case 'archived':
      return filters.archived ? 'Archived only' : null
  }
}

/** Filters with nothing to choose: picking them from the menu turns them on. */
export const TOGGLE_FIELDS: Partial<Record<FilterField, Partial<FilterValues>>> = {
  ready: { ready: true },
  done: { includeDone: 'false' },
  archived: { archived: true },
}
