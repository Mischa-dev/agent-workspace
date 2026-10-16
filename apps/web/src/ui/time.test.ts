import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { dayKey, dayLabel, eventLabel, formatFullDate, formatRelative, formatTime } from './time'

describe('timeline time formatting', () => {
  beforeEach(() => vi.stubEnv('TZ', 'UTC'))
  afterEach(() => vi.unstubAllEnvs())

  it('returns empty display values for invalid timestamps', () => {
    expect(formatTime('not-a-date')).toBe('')
    expect(formatFullDate('not-a-date')).toBe('')
    expect(dayLabel('not-a-date')).toBe('')
    expect(dayKey('not-a-date')).toBe('')
  })

  it('uses calendar dates for stable grouping keys', () => {
    expect(dayKey('2026-10-15T12:30:00Z')).toBe('2026-9-15')
    expect(dayKey('2026-10-15T23:59:59Z')).toBe(dayKey('2026-10-15T00:00:00Z'))
  })

  it('labels today, yesterday, older dates, and dates from prior years', () => {
    const now = new Date('2026-10-15T12:00:00Z')
    expect(dayLabel('2026-10-15T01:00:00Z', now)).toBe('Today')
    expect(dayLabel('2026-10-14T23:00:00Z', now)).toBe('Yesterday')
    expect(dayLabel('2026-10-10T00:00:00Z', now)).not.toContain('2026')
    expect(dayLabel('2025-10-10T00:00:00Z', now)).toContain('2025')
  })

  it('humanizes event identifiers', () => {
    expect(eventLabel('run_started')).toBe('Run started')
    expect(eventLabel('')).toBe('')
  })

  it('says how long ago something happened, then falls back to the date', () => {
    const now = new Date('2026-10-15T12:00:00Z')
    expect(formatRelative('2026-10-15T11:59:40Z', now)).toBe('just now')
    expect(formatRelative('2026-10-15T11:55:00Z', now)).toBe('5 min ago')
    expect(formatRelative('2026-10-15T09:00:00Z', now)).toBe('3 hr ago')
    expect(formatRelative('2026-10-14T12:00:00Z', now)).toBe('1 day ago')
    expect(formatRelative('2026-10-15T12:10:00Z', now)).toBe('in 10 min')
    expect(formatRelative('2026-09-01T12:00:00Z', now)).not.toContain('ago')
    expect(formatRelative('nope', now)).toBe('')
  })
})
