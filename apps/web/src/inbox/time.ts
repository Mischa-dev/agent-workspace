import { formatRelativeShort } from '../ui/time'

/** "now", "5m", "3h", "2d", or a short date, in either direction. */
export function relativeTime(iso: string, now = Date.now()) {
  return formatRelativeShort(iso, new Date(now))
}
