import { describe, expect, it } from 'vitest'

import { describeSchedule, scheduleInput } from './templateModel'

describe('record schedules', () => {
  it('sends only the parts its frequency uses', () => {
    const draft = { frequency: 'weekly' as const, every: '2', weekdays: [4, 1], monthDay: '15', timeOfDay: '09:00', timezone: 'Europe/Amsterdam' }
    expect(scheduleInput(draft)).toEqual({ frequency: 'weekly', every: 2, weekdays: [1, 4], timeOfDay: '09:00', timezone: 'Europe/Amsterdam' })
    expect(scheduleInput({ ...draft, frequency: 'monthly', every: '' })).toEqual({ frequency: 'monthly', every: 1, monthDay: 15, timeOfDay: '09:00', timezone: 'Europe/Amsterdam' })
  })

  it('reads as a sentence', () => {
    expect(describeSchedule({ frequency: 'weekly', every: 1, weekdays: [1, 4], timeOfDay: '09:00', timezone: 'UTC' })).toBe('Weekly on Mon, Thu at 09:00 (UTC)')
    expect(describeSchedule({ frequency: 'monthly', every: 3, monthDay: 31, timeOfDay: '18:30', timezone: 'UTC' })).toBe('Every 3 months on day 31 at 18:30 (UTC)')
  })
})
