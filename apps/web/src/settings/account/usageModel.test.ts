import { describe, expect, it } from 'vitest'

import { describeSchedule, formatCountdown, resetText, timeToMinute, windowTone } from './usageModel'
import type { UsageWindow } from './usageApi'

const weekly: UsageWindow = {
  key: 'codex.secondary',
  label: 'Weekly',
  source: 'reported',
  scheduleId: null,
  durationMinutes: 10080,
  usedPercent: 62,
  unusedPercent: 38,
  resetsAt: '2026-10-17T12:00:00Z',
  limitReached: false,
  readAt: null,
  rolledOver: false,
}

describe('usage countdowns', () => {
  it('counts down in the largest useful units', () => {
    expect(formatCountdown(3 * 86_400_000 + 4 * 3_600_000)).toBe('3 d 4 h')
    expect(formatCountdown(2 * 3_600_000 + 14 * 60_000 + 30_000)).toBe('2 h 14 min')
    expect(formatCountdown(4 * 60_000 + 5_000)).toBe('4 min 05 s')
    expect(formatCountdown(9_000)).toBe('9 s')
    expect(formatCountdown(-1)).toBe('now')
  })

  it('reads a window, including one that already reset', () => {
    const now = Date.parse('2026-10-17T09:30:00Z')
    expect(resetText(weekly, now)).toBe('Resets in 2 h 30 min')
    expect(resetText(weekly, Date.parse('2026-10-17T12:00:01Z'))).toBe('Resetting now')
    expect(resetText({ ...weekly, resetsAt: null }, now)).toBe('Reset time not reported')
    expect(windowTone(weekly)).toBe('ok')
    expect(windowTone({ ...weekly, usedPercent: 95 })).toBe('danger')
    expect(windowTone({ ...weekly, usedPercent: null })).toBe('unknown')
  })

  it('describes schedules and parses times', () => {
    expect(
      describeSchedule({ id: 's', label: 'Team plan', cadence: 'weekly', weekday: 1, monthDay: null, minuteOfDay: 540, timeZone: 'Europe/Berlin' }),
    ).toBe('Every Monday at 09:00 (Europe/Berlin)')
    expect(timeToMinute('22:05')).toBe(1325)
    expect(timeToMinute('24:00')).toBeNull()
  })
})
