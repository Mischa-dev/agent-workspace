/** How often a recurring record (T1.30) makes a record. */
export type Frequency = 'daily' | 'weekly' | 'monthly'

/** The repeat form while it's being filled in: numbers stay text until sent. */
export type ScheduleDraft = {
  frequency: Frequency
  every: string
  weekdays: number[]
  monthDay: string
  timeOfDay: string
  timezone: string
}

/** ISO weekdays, Monday first. */
export const WEEKDAYS: { value: number; label: string }[] = [
  { value: 1, label: 'Mon' },
  { value: 2, label: 'Tue' },
  { value: 3, label: 'Wed' },
  { value: 4, label: 'Thu' },
  { value: 5, label: 'Fri' },
  { value: 6, label: 'Sat' },
  { value: 7, label: 'Sun' },
]

export type ScheduleInput = {
  frequency: Frequency
  every: number
  weekdays?: number[]
  monthDay?: number
  timeOfDay: string
  timezone: string
}

/** The draft as the schedule the server takes; only the parts its frequency uses. */
export function scheduleInput(draft: ScheduleDraft): ScheduleInput {
  const every = Math.max(1, Math.trunc(Number(draft.every)) || 1)
  const input: ScheduleInput = { frequency: draft.frequency, every, timeOfDay: draft.timeOfDay || '09:00', timezone: draft.timezone.trim() || 'UTC' }
  if (draft.frequency === 'weekly') input.weekdays = [...draft.weekdays].sort((left, right) => left - right)
  if (draft.frequency === 'monthly') input.monthDay = Math.trunc(Number(draft.monthDay)) || 1
  return input
}

type Described = {
  frequency: string
  every: number
  weekdays?: readonly number[]
  monthDay?: number | null
  timeOfDay: string
  timezone: string
}

const UNITS: Record<string, [string, string]> = { daily: ['Daily', 'days'], weekly: ['Weekly', 'weeks'], monthly: ['Monthly', 'months'] }

/** A schedule in a few words, such as "Weekly on Mon, Thu at 09:00 (Europe/Amsterdam)". */
export function describeSchedule(schedule: Described): string {
  const [single, plural] = UNITS[schedule.frequency] ?? ['Daily', 'days']
  let text = schedule.every > 1 ? `Every ${schedule.every} ${plural}` : single
  if (schedule.frequency === 'weekly' && schedule.weekdays?.length) {
    const days = WEEKDAYS.filter((day) => schedule.weekdays?.includes(day.value)).map((day) => day.label)
    text += ` on ${days.join(', ')}`
  }
  if (schedule.frequency === 'monthly' && schedule.monthDay) text += ` on day ${schedule.monthDay}`
  return `${text} at ${schedule.timeOfDay} (${schedule.timezone})`
}
