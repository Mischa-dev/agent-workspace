import type { OpOutput } from '../api/ops'
import { addDays, formatDateLabel, fromIsoDate, toIsoDate } from '../ui/dateModel'

export type Cycle = OpOutput<'cycle_list'>['cycles'][number]

export const weekdays = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'] as const

/** Cycle lengths offered, in days: one to eight weeks. */
export const cycleLengths = [7, 14, 21, 28, 35, 42, 49, 56] as const

export function lengthLabel(days: number) {
  return days % 7 === 0 ? `${days / 7} ${days === 7 ? 'week' : 'weeks'}` : `${days} days`
}

/** "Nov 16 – Nov 29": a cycle's first and last day (it ends the day before endsOn). */
export function cycleDates(cycle: Pick<Cycle, 'startsOn' | 'endsOn'>, now = new Date()) {
  const last = fromIsoDate(cycle.endsOn)
  return `${formatDateLabel(cycle.startsOn, undefined, now)} – ${last ? formatDateLabel(toIsoDate(addDays(last, -1)), undefined, now) : ''}`
}

/** "2 of 5 done", leaving out canceled tasks. */
export function progressLabel(cycle: Pick<Cycle, 'scope' | 'completed'>) {
  return cycle.scope === 0 ? 'No tasks' : `${cycle.completed} of ${cycle.scope} done`
}

export type BurnUpDay = { date: string; scope: number | null; completed: number | null }

/**
 * Each day of a cycle with its count, oldest first: a day the job didn't
 * count (one still to come, or a gap) has none.
 */
export function burnUp(cycle: Pick<Cycle, 'startsOn' | 'endsOn' | 'snapshots'>): BurnUpDay[] {
  const first = fromIsoDate(cycle.startsOn)
  const end = fromIsoDate(cycle.endsOn)
  if (!first || !end) return []
  const counted = new Map(cycle.snapshots.map((snapshot) => [snapshot.onDate, snapshot]))
  const days: BurnUpDay[] = []
  for (let day = first; day < end && days.length < 56; day = addDays(day, 1)) {
    const date = toIsoDate(day)
    const snapshot = counted.get(date)
    days.push({ date, scope: snapshot?.scope ?? null, completed: snapshot?.completed ?? null })
  }
  return days
}

/** An SVG path through the days that have a value, starting a new run after a gap. */
export function linePath(values: ReadonlyArray<number | null>, x: (index: number) => number, y: (value: number) => number) {
  let previous = false
  const parts: string[] = []
  values.forEach((value, index) => {
    if (value === null) {
      previous = false
      return
    }
    parts.push(`${previous ? 'L' : 'M'}${x(index)},${y(value)}`)
    previous = true
  })
  return parts.join(' ')
}
