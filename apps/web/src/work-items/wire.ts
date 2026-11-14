import type {
  ActivityChanges,
  ActorRef,
  EstimateUnit,
  Milestone,
  MilestoneRef,
  ProjectLabel,
  TaskClaim,
  TaskHandoff,
  TaskRef,
  TaskRelation,
  WorkItemPriority,
} from './types'

/** Defensive readers for server JSON: a missing or odd field never crashes a view. */
export type WireRecord = Record<string, unknown>

export function record(value: unknown): WireRecord {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as WireRecord) : {}
}

export function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}

export function string(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

export function nullableString(value: unknown) {
  return typeof value === 'string' ? value : null
}

export function number(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

export function nullableNumber(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

export function boolean(value: unknown) {
  return value === true
}

const DATE = /^\d{4}-\d{2}-\d{2}$/

/** A `YYYY-MM-DD` date, or null. */
export function date(value: unknown) {
  return typeof value === 'string' && DATE.test(value) ? value : null
}

export function priority(value: unknown): WorkItemPriority {
  return value === 'low' || value === 'medium' || value === 'high' || value === 'urgent' ? value : 'no_priority'
}

export function estimateUnit(value: unknown): EstimateUnit {
  return value === 'hours' ? 'hours' : 'points'
}

const LABEL_COLOR = /^#[0-9a-fA-F]{6}$/
export const FALLBACK_LABEL_COLOR = '#8b8d98'

/** Colors go into inline styles, so only a plain hex color is used. */
export function color(value: unknown) {
  const text = string(value)
  return LABEL_COLOR.test(text) ? text.toLowerCase() : FALLBACK_LABEL_COLOR
}

/** Only same-app paths are followed, so a hostile href can't leave the app. */
export function appHref(value: unknown) {
  const href = string(value)
  return href.startsWith('/projects/') ? href : ''
}

export function actorRef(value: unknown): ActorRef | null {
  if (!value || typeof value !== 'object') return null
  const actor = record(value)
  const id = string(actor.id) || string(actor.actorId)
  if (!id) return null
  return {
    id,
    displayName: string(actor.displayName),
    handle: string(actor.handle),
    kind: string(actor.kind),
    avatarUrl: nullableString(actor.avatarUrl),
  }
}

export function actorRefs(value: unknown): ActorRef[] {
  return list(value).flatMap((entry) => {
    const actor = actorRef(entry)
    return actor ? [actor] : []
  })
}

export function taskRef(value: unknown): TaskRef | null {
  if (!value || typeof value !== 'object') return null
  const task = record(value)
  const id = string(task.id)
  if (!id) return null
  const statusCategory = string(task.statusCategory)
  return {
    id,
    projectId: string(task.projectId),
    key: string(task.key),
    title: string(task.title),
    priority: priority(task.priority),
    statusName: string(task.statusName),
    statusCategory,
    finished: typeof task.finished === 'boolean' ? task.finished : statusCategory === 'completed' || statusCategory === 'canceled',
    archived: boolean(task.archived),
    href: appHref(task.href),
  }
}

export function taskRefs(value: unknown): TaskRef[] {
  return list(value).flatMap((entry) => {
    const task = taskRef(entry)
    return task ? [task] : []
  })
}

export function milestoneRef(value: unknown): MilestoneRef | null {
  if (!value || typeof value !== 'object') return null
  const milestone = record(value)
  const id = string(milestone.id)
  return id ? { id, name: string(milestone.name), targetDate: date(milestone.targetDate) } : null
}

export function relations(value: unknown): TaskRelation[] {
  return list(value).flatMap((entry) => {
    const relation = record(entry)
    const task = taskRef(relation.task)
    const kind = relation.kind
    if (!task || (kind !== 'relates_to' && kind !== 'duplicates' && kind !== 'duplicated_by')) return []
    return [{ kind, task }]
  })
}

export function claim(value: unknown): TaskClaim | null {
  if (!value || typeof value !== 'object') return null
  const entry = record(value)
  const actor = actorRef(entry.actor)
  if (!actor) return null
  return {
    actor,
    note: nullableString(entry.note),
    claimedAt: string(entry.claimedAt),
    expiresAt: string(entry.expiresAt),
  }
}

export function handoff(value: unknown): TaskHandoff | null {
  if (!value || typeof value !== 'object') return null
  const entry = record(value)
  const actor = actorRef(entry.actor)
  const note = string(entry.note)
  if (!actor || !note) return null
  return { actor, note, at: string(entry.at), via: nullableString(entry.via) }
}

export function projectLabel(value: unknown): ProjectLabel {
  const label = record(value)
  return {
    id: string(label.id),
    name: string(label.name),
    color: color(label.color),
    description: string(label.description),
    archived: boolean(label.archived),
    taskCount: number(label.taskCount),
    version: number(label.version),
  }
}

export function milestone(value: unknown): Milestone {
  const entry = record(value)
  const total = number(entry.total)
  const finished = number(entry.finished)
  const progress = typeof entry.progress === 'number' ? entry.progress : total ? finished / total : 0
  return {
    id: string(entry.id),
    name: string(entry.name),
    description: string(entry.description),
    targetDate: date(entry.targetDate),
    archived: boolean(entry.archived),
    total,
    finished,
    open: typeof entry.open === 'number' ? entry.open : Math.max(0, total - finished),
    canceled: typeof entry.canceled === 'number' ? entry.canceled : 0,
    progress: Math.max(0, Math.min(1, progress)),
    overdue: boolean(entry.overdue),
    version: number(entry.version),
  }
}

function changeValue(value: unknown): string | number | null {
  if (typeof value === 'string') return value.slice(0, 500)
  if (typeof value === 'number' && Number.isFinite(value)) return value
  return null
}

/** `payload.changes` on a history entry: field → { from, to }, or the description's diff. */
export function activityChanges(value: unknown): ActivityChanges | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const changes: ActivityChanges = {}
  for (const [field, raw] of Object.entries(value as WireRecord)) {
    const change = record(raw)
    if (field === 'description') {
      if (typeof change.diff === 'string') changes.description = { diff: change.diff, truncated: boolean(change.truncated) }
      continue
    }
    if (!('from' in change) && !('to' in change)) continue
    if (field.startsWith('field:')) {
      // A custom field: values are display text, numbers, yes/no or lists of option names.
      const shown = (raw: unknown) =>
        Array.isArray(raw)
          ? raw.filter((item) => typeof item === 'string').join(', ') || null
          : typeof raw === 'boolean'
            ? raw
              ? 'Yes'
              : null
            : changeValue(raw)
      changes[field] = { from: shown(change.from), to: shown(change.to), label: string(change.field, 'A field') }
      continue
    }
    changes[field] = { from: changeValue(change.from), to: changeValue(change.to) }
  }
  return Object.keys(changes).length ? changes : null
}
