import type { OpOutput } from '../api/ops'
import { fromIsoDate } from '../ui/dateModel'

export type Initiatives = OpOutput<'initiative_list'>
export type Initiative = Initiatives['initiatives'][number]
export type InitiativeMilestone = Initiative['milestones'][number]

/** "3 of 5 done" across its milestones, leaving out canceled tasks. */
export function initiativeProgress(item: { scope: number; completed: number }) {
  return item.scope === 0 ? 'No tasks yet' : `${item.completed} of ${item.scope} done`
}

/** Completed over scope, from 0 to 1. */
export function share(item: { scope: number; completed: number }) {
  return item.scope > 0 ? Math.min(1, Math.max(0, item.completed / item.scope)) : 0
}

/** A field's value as one line: a list joined, a yes or no. */
export function fieldText(value: unknown): string {
  if (Array.isArray(value)) return value.map(fieldText).filter(Boolean).join(', ')
  if (value === true) return 'Yes'
  if (value === false) return 'No'
  if (typeof value === 'number') return String(value)
  return typeof value === 'string' ? value : ''
}

export type TimelinePoint = { milestone: InitiativeMilestone; at: number }

/**
 * The milestones with a target date along one line, each at its share of
 * the way from the earliest date to the latest (0 to 1), and today's place
 * on it when it falls between; those without a date are listed apart.
 */
export function timeline(milestones: readonly InitiativeMilestone[], now = new Date()) {
  const dated = milestones.flatMap((milestone) => {
    const date = fromIsoDate(milestone.targetDate)
    return date ? [{ milestone, time: date.getTime() }] : []
  })
  const undated = milestones.filter((milestone) => !fromIsoDate(milestone.targetDate))
  if (dated.length === 0) return { points: [] as TimelinePoint[], undated, today: null as number | null }
  const first = Math.min(...dated.map((entry) => entry.time))
  const last = Math.max(...dated.map((entry) => entry.time))
  const span = last - first
  const at = (time: number) => (span === 0 ? 0.5 : (time - first) / span)
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  return {
    points: dated.sort((a, b) => a.time - b.time).map((entry) => ({ milestone: entry.milestone, at: at(entry.time) })),
    undated,
    today: span > 0 && today >= first && today <= last ? at(today) : null,
  }
}
