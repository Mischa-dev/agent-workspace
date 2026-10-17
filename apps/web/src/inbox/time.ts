import { formatShortDate } from '../ui/time'

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/** "now", "5m", "3h", "2d", or a short date, in either direction. */
export function relativeTime(iso: string, now = Date.now()) {
  const at = Date.parse(iso)
  if (Number.isNaN(at)) return ''
  const distance = Math.abs(now - at)
  if (distance < MINUTE) return 'now'
  if (distance < HOUR) return `${Math.round(distance / MINUTE)}m`
  if (distance < DAY) return `${Math.round(distance / HOUR)}h`
  if (distance < 7 * DAY) return `${Math.round(distance / DAY)}d`
  return formatShortDate(iso, new Date(now))
}
