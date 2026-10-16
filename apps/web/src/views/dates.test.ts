import { describe, expect, it } from 'vitest'

import { addDays, dayFromNumber, dayNumber } from './dates'

describe('calendar days', () => {
  it('round-trips and crosses month and leap-day boundaries', () => {
    expect(dayFromNumber(dayNumber('2026-10-16'))).toBe('2026-10-16')
    expect(addDays('2026-10-31', 1)).toBe('2026-11-01')
    expect(addDays('2028-02-28', 1)).toBe('2028-02-29')
    expect(addDays('2026-01-01', -1)).toBe('2025-12-31')
  })
})
