import { formatDateLabel } from '../ui/dateModel'

/** Calendar days as YYYY-MM-DD, worked out in UTC so no time zone shifts a day. */

const DAY_MS = 86_400_000

export function dayNumber(iso: string) {
  const [year, month, day] = iso.split('-').map(Number)
  return Math.floor(Date.UTC(year ?? 1970, (month ?? 1) - 1, day ?? 1) / DAY_MS)
}

export function dayFromNumber(value: number) {
  return new Date(value * DAY_MS).toISOString().slice(0, 10)
}

export function addDays(iso: string, days: number) {
  return dayFromNumber(dayNumber(iso) + days)
}

export function today() {
  const now = new Date()
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`
}

/** "Oct 20", with the year only when it isn't this year: the kit's date label. */
export function dayLabel(iso: string) {
  return formatDateLabel(iso) || iso
}
