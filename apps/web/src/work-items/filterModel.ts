import { workItemPriorities } from './constants'
import { SEMANTIC_CATEGORIES, canonicalValues, type FilterValues } from './listModel'
import type { Milestone, ProjectLabel, WorkItemWorkflow } from './types'

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
  { field: 'status', label: 'Status', keys: ['statusId', 'notStatusId'] },
  { field: 'category', label: 'Status group', keys: ['semanticCategory', 'notSemanticCategory'] },
  { field: 'priority', label: 'Priority', keys: ['priority', 'notPriority'] },
  { field: 'assignee', label: 'Assignee', keys: ['assigneeActorId', 'notAssigneeActorId'] },
  { field: 'labels', label: 'Labels', keys: ['anyLabelIds', 'labelIds', 'notLabelIds'] },
  { field: 'milestone', label: 'Milestone', keys: ['milestoneId', 'notMilestoneId'] },
  { field: 'parent', label: 'Parent', keys: ['parentId'] },
  { field: 'type', label: 'Type', keys: ['typeId', 'notTypeId'] },
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

/** Fields that take several values: any of them, or none of them. */
export type ListField = 'status' | 'category' | 'priority' | 'assignee' | 'labels' | 'milestone' | 'type'

/**
 * How a list filter matches: the task has any of the values, all of them
 * (labels only), or none of them.
 */
export type FilterOperator = 'any' | 'all' | 'not'

/** The URL key for each way a field can match. */
export const LIST_FIELDS: Record<ListField, { keys: Partial<Record<FilterOperator, keyof FilterValues>>; plural: string; none?: string }> = {
  status: { keys: { any: 'statusId', not: 'notStatusId' }, plural: 'statuses' },
  category: { keys: { any: 'semanticCategory', not: 'notSemanticCategory' }, plural: 'status groups' },
  priority: { keys: { any: 'priority', not: 'notPriority' }, plural: 'priorities' },
  assignee: { keys: { any: 'assigneeActorId', not: 'notAssigneeActorId' }, plural: 'assignees', none: 'Unassigned' },
  labels: { keys: { any: 'anyLabelIds', all: 'labelIds', not: 'notLabelIds' }, plural: 'labels', none: 'No labels' },
  milestone: { keys: { any: 'milestoneId', not: 'notMilestoneId' }, plural: 'milestones', none: 'No milestone' },
  type: { keys: { any: 'typeId', not: 'notTypeId' }, plural: 'types' },
}

export function isListField(field: FilterField): field is ListField {
  return field in LIST_FIELDS
}

/** The ways a field can match, in menu order. */
export function operatorsOf(field: ListField): FilterOperator[] {
  return (['any', 'all', 'not'] as const).filter((operator) => LIST_FIELDS[field].keys[operator])
}

/** The words between a field and its values, such as "is any of". */
export function operatorText(field: ListField, operator: FilterOperator, count: number): string {
  if (field === 'labels') {
    if (operator === 'not') return count > 1 ? 'include none of' : 'don’t include'
    if (count <= 1) return 'include'
    return operator === 'all' ? 'include all of' : 'include any of'
  }
  if (operator === 'not') return count > 1 ? 'is none of' : 'is not'
  return count > 1 ? 'is any of' : 'is'
}

/** The operator choices in a filter's value list: short, since the chip says the rest. */
export function operatorChoices(field: ListField): Array<{ value: FilterOperator; label: string }> {
  return field === 'labels'
    ? [
        { value: 'any', label: 'Any of' },
        { value: 'all', label: 'All of' },
        { value: 'not', label: 'None of' },
      ]
    : [
        { value: 'any', label: 'Is' },
        { value: 'not', label: 'Is not' },
      ]
}

/** The values a list filter names, "none" among them. */
export function valuesOf(filters: FilterValues, key: keyof FilterValues): string[] {
  const value = filters[key]
  return typeof value === 'string' && value ? value.split(',') : []
}

/** The filters with `key` naming `values`, or without it when there are none. */
export function withValues(filters: FilterValues, key: keyof FilterValues, values: readonly string[]): FilterValues {
  const next: Record<string, unknown> = { ...filters }
  const canonical = canonicalValues(values)
  if (canonical.length) next[key] = canonical.join(',')
  else delete next[key]
  return next as FilterValues
}

/**
 * Moves a filter's values to another way of matching, joining any values
 * already there. All of several labels can't include "No labels", so it
 * goes unless it's the only value.
 */
export function withOperator(filters: FilterValues, field: ListField, from: FilterOperator, to: FilterOperator): FilterValues {
  const keys = LIST_FIELDS[field].keys
  const fromKey = keys[from]
  const toKey = keys[to]
  if (!fromKey || !toKey || fromKey === toKey) return filters
  let values = [...valuesOf(filters, toKey), ...valuesOf(filters, fromKey)]
  if (to === 'all' && values.some((value) => value !== 'none')) values = values.filter((value) => value !== 'none')
  return withValues(withValues(filters, fromKey, []), toKey, values)
}

export type FilterLookups = {
  workflow?: WorkItemWorkflow
  labels?: ProjectLabel[]
  milestones?: Milestone[]
  /** Names of people and agents, by actor ID. */
  actorNames?: ReadonlyMap<string, string>
  parentKey?: string | null
}

/** A value's name, and where it sorts among the field's values. */
function named(field: ListField, value: string, lookups: FilterLookups): { name: string; rank: number } {
  const none = LIST_FIELDS[field].none
  if (value === 'none' && none) return { name: none, rank: -1 }
  const at = <T extends { id: string }>(entries: readonly T[] | undefined, fallback: string, name: (entry: T) => string) => {
    const index = entries?.findIndex((entry) => entry.id === value) ?? -1
    return index >= 0 && entries ? { name: name(entries[index]), rank: index } : { name: fallback, rank: Number.MAX_SAFE_INTEGER }
  }
  switch (field) {
    case 'status':
      return at(lookups.workflow?.statuses, 'a removed status', (status) => status.name)
    case 'type':
      return at(lookups.workflow?.types, 'a removed type', (type) => type.name)
    case 'labels':
      return at(lookups.labels, 'a removed label', (label) => label.name)
    case 'milestone':
      return at(lookups.milestones, 'a removed milestone', (milestone) => milestone.name)
    case 'category':
      return { name: CATEGORY_LABELS[value] ?? value, rank: SEMANTIC_CATEGORIES.indexOf(value) }
    case 'priority': {
      const rank = workItemPriorities.findIndex((priority) => priority.value === value)
      return { name: workItemPriorities[rank]?.label ?? value, rank }
    }
    case 'assignee':
      return { name: lookups.actorNames?.get(value) ?? 'someone', rank: 0 }
  }
}

/** Names of the values, in the order the field lists them. */
export function valueNames(field: ListField, values: readonly string[], lookups: FilterLookups): string[] {
  return values
    .map((value) => named(field, value, lookups))
    .sort((a, b) => a.rank - b.rank || a.name.localeCompare(b.name))
    .map((entry) => entry.name)
}

/** What a field with only "none" reads as, for each operator. */
const NONE_ONLY: Partial<Record<ListField, { any: string; not: string }>> = {
  assignee: { any: 'Unassigned', not: 'Assigned' },
  labels: { any: 'No labels', not: 'Has labels' },
  milestone: { any: 'No milestone', not: 'In a milestone' },
}

export type FilterChip =
  /** A filter with values to choose: "Status", "is any of", "Ready, In Progress". */
  | {
      kind: 'list'
      field: ListField
      operator: FilterOperator
      key: keyof FilterValues
      label: string
      /** The operator's words; empty when `values` already says it all, such as "Unassigned". */
      verb: string
      values: string
      /** The whole filter in words, every value named. */
      text: string
    }
  | { kind: 'text'; field: FilterField; text: string }

/** Up to two names, then a count. */
function shortNames(names: readonly string[], plural: string) {
  return names.length > 2 ? `${names.length} ${plural}` : names.join(', ')
}

/** The chips for these filters, in the Filter menu's order. */
export function filterChips(filters: FilterValues, lookups: FilterLookups): FilterChip[] {
  return FILTER_FIELDS.flatMap((entry): FilterChip[] => {
    const field = entry.field
    if (!isListField(field)) {
      const text = chipText(field, filters, lookups)
      return text ? [{ kind: 'text', field, text }] : []
    }
    return operatorsOf(field).flatMap((operator): FilterChip[] => {
      const key = LIST_FIELDS[field].keys[operator] as keyof FilterValues
      const values = valuesOf(filters, key)
      if (!values.length) return []
      const noneOnly = NONE_ONLY[field]
      if (noneOnly && values.length === 1 && values[0] === 'none') {
        const words = operator === 'not' ? noneOnly.not : noneOnly.any
        return [{ kind: 'list', field, operator, key, label: entry.label, verb: '', values: words, text: words }]
      }
      const names = valueNames(field, values, lookups)
      const verb = operatorText(field, operator, values.length)
      return [
        {
          kind: 'list',
          field,
          operator,
          key,
          label: entry.label,
          verb,
          values: shortNames(names, LIST_FIELDS[field].plural),
          text: `${entry.label} ${verb} ${names.join(', ')}`,
        },
      ]
    })
  })
}

function formatDay(value: string) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return value
  return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3])).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
}

/** The words on a chip for a filter with nothing to pick from a list, such as "Blocked". Null when it isn't set. */
function chipText(field: FilterField, filters: FilterValues, lookups: FilterLookups): string | null {
  switch (field) {
    case 'parent':
      if (!filters.parentId) return null
      return filters.parentId === 'none' ? 'Top-level only' : `Subtasks of ${lookups.parentKey ?? 'a task'}`
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
    default:
      return null
  }
}

/** Filters with nothing to choose: picking them from the menu turns them on. */
export const TOGGLE_FIELDS: Partial<Record<FilterField, Partial<FilterValues>>> = {
  ready: { ready: true },
  done: { includeDone: 'false' },
  archived: { archived: true },
}
