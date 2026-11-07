import { priorityLabel } from './constants'
import type { ActivityChanges, DescriptionChange, EstimateUnit, FieldChange, WorkItemPriority } from './types'
import { formatDateLabel } from '../ui/dateModel'

/**
 * History entries carry `payload.changes`, field → { from, to }, with ids
 * already turned into names by the server. This turns them into lines such as
 * "Priority High → Urgent" and a readable description diff.
 */

export type FormattedChange =
  | { kind: 'field'; field: string; label: string; from: string; to: string }
  | { kind: 'description'; field: 'description'; label: string; lines: DiffLine[]; truncated: boolean }

export type DiffLine = { kind: 'added' | 'removed' | 'same'; text: string }

const FIELD_ORDER = [
  'title',
  'status',
  'priority',
  'assignee',
  'type',
  'milestone',
  'parent',
  'startDate',
  'dueDate',
  'estimate',
  'blockedReason',
  'description',
]

const FIELD_LABELS: Record<string, string> = {
  title: 'Title',
  status: 'Status',
  priority: 'Priority',
  assignee: 'Assignee',
  type: 'Type',
  milestone: 'Milestone',
  parent: 'Parent',
  startDate: 'Start date',
  dueDate: 'Due date',
  estimate: 'Estimate',
  blockedReason: 'Blocked',
  description: 'Description',
}

const EMPTY: Record<string, string> = {
  assignee: 'Unassigned',
  priority: 'No priority',
  milestone: 'No milestone',
  parent: 'No parent',
  startDate: 'No date',
  dueDate: 'No date',
  estimate: 'No estimate',
  blockedReason: 'Not blocked',
}

/** "startDate" → "Start date" for fields this version doesn't know yet. */
function fieldLabel(field: string) {
  if (FIELD_LABELS[field]) return FIELD_LABELS[field]
  const words = field.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/_/g, ' ').toLowerCase()
  return words.charAt(0).toUpperCase() + words.slice(1)
}

/** "Oct 15", with the year when it isn't this year. */
export function formatIsoDate(value: string) {
  return formatDateLabel(value) || value
}

export function formatEstimate(value: number, unit: EstimateUnit) {
  const amount = Number.isInteger(value) ? String(value) : value.toFixed(1)
  if (unit === 'hours') return `${amount} ${value === 1 ? 'hour' : 'hours'}`
  return `${amount} ${value === 1 ? 'point' : 'points'}`
}

function formatValue(field: string, value: string | number | null, unit: EstimateUnit) {
  if (value === null || value === '') return EMPTY[field] ?? 'None'
  if (field === 'priority' && typeof value === 'string') return priorityLabel(value as WorkItemPriority)
  if ((field === 'startDate' || field === 'dueDate') && typeof value === 'string') return formatIsoDate(value)
  if (field === 'estimate' && typeof value === 'number') return formatEstimate(value, unit)
  if (field === 'blockedReason') return `Blocked: ${value}`
  return String(value)
}

function isDescription(change: FieldChange | DescriptionChange): change is DescriptionChange {
  return 'diff' in change
}

export function parseDiff(diff: string): DiffLine[] {
  return diff
    .split('\n')
    .filter((line, index, lines) => !(line === '' && index === lines.length - 1))
    .map((line) => {
      if (line.startsWith('+')) return { kind: 'added', text: line.slice(1) }
      if (line.startsWith('-')) return { kind: 'removed', text: line.slice(1) }
      return { kind: 'same', text: line.startsWith(' ') ? line.slice(1) : line }
    })
}

export function formatChanges(changes: ActivityChanges | null | undefined, unit: EstimateUnit = 'points'): FormattedChange[] {
  if (!changes) return []
  const fields = Object.keys(changes).sort((a, b) => {
    const left = FIELD_ORDER.indexOf(a)
    const right = FIELD_ORDER.indexOf(b)
    return (left < 0 ? FIELD_ORDER.length : left) - (right < 0 ? FIELD_ORDER.length : right)
  })
  return fields.flatMap<FormattedChange>((field) => {
    const change = changes[field]
    if (!change) return []
    if (isDescription(change)) {
      return [{ kind: 'description', field: 'description', label: 'Description', lines: parseDiff(change.diff), truncated: change.truncated }]
    }
    const from = formatValue(field, change.from, unit)
    const to = formatValue(field, change.to, unit)
    if (from === to) return []
    return [{ kind: 'field', field, label: change.label ?? fieldLabel(field), from, to }]
  })
}

/** One line of plain text per change, for screen readers and tests. */
export function changeSentence(change: FormattedChange) {
  if (change.kind === 'description') return 'Description changed'
  return `${change.label} ${change.from} → ${change.to}`
}
