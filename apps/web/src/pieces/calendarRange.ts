import { addDays, monthGrid, toIsoDate } from '../ui/dateModel'

/** The first day a month's calendar shows and the day after its last, as `YYYY-MM-DD`, for loading its items. */
export function calendarRange(month: Date): { from: string; to: string } {
  const days = monthGrid(month)
  return { from: toIsoDate(days[0]), to: toIsoDate(addDays(days[days.length - 1], 1)) }
}
