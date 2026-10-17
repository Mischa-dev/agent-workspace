import type { TimelineLink, TimelineMilestone, TimelineTask } from './api'
import { dayNumber } from './dates'

/** A task's bar in days: a task with one date is one day long. */
export type Span = { start: number; end: number }

export function spanOf(task: Pick<TimelineTask, 'startDate' | 'dueDate'>): Span | null {
  const start = task.startDate ?? task.dueDate
  const end = task.dueDate ?? task.startDate
  if (!start || !end) return null
  const first = dayNumber(start)
  return { start: first, end: Math.max(first, dayNumber(end)) }
}

/** A change being dragged, before it's saved. */
export type PendingChange =
  | { kind: 'milestone'; id: string; days: number }
  | { kind: 'task'; id: string; days: number; edge: 'move' | 'end' }

export type Slip = {
  /** Tasks that would end after their milestone, or start before what blocks them finishes. */
  tasks: Map<string, string>
  /** Milestones whose tasks would end after them. */
  milestones: Set<string>
}

function shifted(span: Span, change: PendingChange | null, id: string): Span {
  if (!change || change.kind !== 'task' || change.id !== id) return span
  return change.edge === 'end'
    ? { start: span.start, end: Math.max(span.start, span.end + change.days) }
    : { start: span.start + change.days, end: span.end + change.days }
}

/** Where every task's bar sits with the pending change applied. */
export function spansWith(tasks: TimelineTask[], change: PendingChange | null) {
  const spans = new Map<string, Span>()
  for (const task of tasks) {
    const span = spanOf(task)
    if (span) spans.set(task.id, shifted(span, change, task.id))
  }
  return spans
}

export function targetWith(milestone: TimelineMilestone, change: PendingChange | null) {
  if (!milestone.targetDate) return null
  const day = dayNumber(milestone.targetDate)
  return change?.kind === 'milestone' && change.id === milestone.id ? day + change.days : day
}

const finished = (task: TimelineTask) => task.statusCategory === 'completed' || task.statusCategory === 'canceled'

/**
 * What slips with the pending change applied: open tasks that would end after
 * their milestone's target date, and open tasks that would start before a task
 * blocking them ends. Only what the change causes counts, so a timeline that
 * already has late tasks doesn't light up before anything moves.
 */
export function slips(
  tasks: TimelineTask[],
  milestones: TimelineMilestone[],
  dependencies: TimelineLink[],
  change: PendingChange | null,
): Slip {
  const result: Slip = { tasks: new Map(), milestones: new Set() }
  if (!change || change.days === 0) return result
  const before = problems(tasks, milestones, dependencies, null)
  const after = problems(tasks, milestones, dependencies, change)
  for (const [id, reason] of after.tasks) if (!before.tasks.has(id)) result.tasks.set(id, reason)
  for (const id of after.milestones) if (!before.milestones.has(id)) result.milestones.add(id)
  return result
}

function problems(tasks: TimelineTask[], milestones: TimelineMilestone[], dependencies: TimelineLink[], change: PendingChange | null): Slip {
  const result: Slip = { tasks: new Map(), milestones: new Set() }
  const spans = spansWith(tasks, change)
  const byId = new Map(tasks.map((task) => [task.id, task]))
  const targets = new Map(milestones.map((milestone) => [milestone.id, { milestone, day: targetWith(milestone, change) }]))
  for (const task of tasks) {
    const span = spans.get(task.id)
    if (!span || finished(task) || !task.milestoneId) continue
    const target = targets.get(task.milestoneId)
    if (target?.day != null && span.end > target.day) {
      result.tasks.set(task.id, `Ends after ${target.milestone.name}`)
      result.milestones.add(target.milestone.id)
    }
  }
  for (const link of dependencies) {
    const blocking = spans.get(link.blockingId)
    const blocked = spans.get(link.blockedId)
    const blockedTask = byId.get(link.blockedId)
    const blockingTask = byId.get(link.blockingId)
    if (!blocking || !blocked || !blockedTask || !blockingTask || finished(blockedTask) || finished(blockingTask)) continue
    if (blocked.start <= blocking.end && !result.tasks.has(link.blockedId)) {
      result.tasks.set(link.blockedId, `Starts before ${blockingTask.key} is done`)
    }
  }
  return result
}

/** The days the chart shows: the request's range, widened to fit what's dragged. */
export function chartRange(from: string, to: string) {
  return { first: dayNumber(from), last: dayNumber(to) }
}
