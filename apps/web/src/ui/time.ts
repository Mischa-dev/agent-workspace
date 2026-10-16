/**
 * Every date and time the app shows is formatted here, so pages agree on how
 * "5 min ago", "Oct 15, 3:04 PM" and "Yesterday" read.
 */

function parse(iso: string | null | undefined): Date | null {
  if (!iso) return null
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? null : date
}

function startOfDay(value: Date) {
  return new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime()
}

/** Whole calendar days from `date` to `now`, in local time. */
function daysBetween(date: Date, now: Date) {
  return Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000)
}

/** "3:04 PM". Empty for a missing or invalid time. */
export function formatTime(iso: string | null | undefined) {
  const date = parse(iso)
  return date ? date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) : ''
}

/** "Thursday, October 15, 2026 at 3:04 PM", for tooltips. */
export function formatFullDate(iso: string | null | undefined) {
  const date = parse(iso)
  return date
    ? date.toLocaleString([], { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric', hour: 'numeric', minute: '2-digit' })
    : ''
}

/** "Oct 15, 3:04 PM", with the year when it isn't this year. Empty when invalid. */
export function formatDateTime(iso: string | null | undefined, now = new Date()) {
  const date = parse(iso)
  if (!date) return ''
  return date.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    ...(date.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
  })
}

/** "Oct 15", with the year when it isn't this year. Empty when invalid. */
export function formatShortDate(iso: string | null | undefined, now = new Date()) {
  const date = parse(iso)
  if (!date) return ''
  return date.toLocaleDateString([], {
    month: 'short',
    day: 'numeric',
    ...(date.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
  })
}

/** A stable key for grouping by calendar day. */
export function dayKey(iso: string | null | undefined) {
  const date = parse(iso)
  return date ? `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}` : ''
}

/** "Today", "Yesterday", or "Monday, October 12". */
export function dayLabel(iso: string | null | undefined, now = new Date()) {
  const date = parse(iso)
  if (!date) return ''
  const days = daysBetween(date, now)
  if (days === 0) return 'Today'
  if (days === 1) return 'Yesterday'
  return date.toLocaleDateString([], {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
    ...(date.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' }),
  })
}

/** "today", "yesterday" or "Oct 12": when something happened, by day. */
export function relativeDay(iso: string | null | undefined, now = new Date()) {
  const date = parse(iso)
  if (!date) return ''
  const days = daysBetween(date, now)
  if (days === 0) return 'today'
  if (days === 1) return 'yesterday'
  return formatShortDate(iso, now)
}

/**
 * "just now", "5 min ago", "3 hr ago", "2 days ago", then the date. Future
 * times read "in 5 min". Empty when invalid.
 */
export function formatRelative(iso: string | null | undefined, now = new Date()) {
  const date = parse(iso)
  if (!date) return ''
  const seconds = (now.getTime() - date.getTime()) / 1000
  const ahead = seconds < 0
  const magnitude = Math.abs(seconds)
  const phrase = (value: number, unit: string) => (ahead ? `in ${value} ${unit}` : `${value} ${unit} ago`)
  if (magnitude < 45) return ahead ? 'in a moment' : 'just now'
  if (magnitude < 3_600) return phrase(Math.max(1, Math.round(magnitude / 60)), 'min')
  if (magnitude < 86_400) return phrase(Math.round(magnitude / 3_600), 'hr')
  const days = Math.round(magnitude / 86_400)
  if (days < 7) return phrase(days, days === 1 ? 'day' : 'days')
  return formatShortDate(iso, now)
}

/** "run_started" → "Run started". */
export function eventLabel(kind: string) {
  const words = kind.split('_').join(' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}
