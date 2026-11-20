import { describe, expect, it } from 'vitest'

import { burnUp, cycleDates, lengthLabel, linePath, progressLabel } from './cycleModel'

describe('cycles', () => {
  const now = new Date(2026, 10, 18)

  it('names a cycle by its first and last day', () => {
    expect(cycleDates({ startsOn: '2026-11-16', endsOn: '2026-11-30' }, now)).toBe('Nov 16 – Nov 29')
  })

  it('says lengths in weeks', () => {
    expect(lengthLabel(7)).toBe('1 week')
    expect(lengthLabel(14)).toBe('2 weeks')
  })

  it('counts progress without canceled tasks', () => {
    expect(progressLabel({ scope: 5, completed: 2 })).toBe('2 of 5 done')
    expect(progressLabel({ scope: 0, completed: 0 })).toBe('No tasks')
  })

  it('lays the counts out a day each, with gaps where none was taken', () => {
    const days = burnUp({
      startsOn: '2026-11-16',
      endsOn: '2026-11-20',
      snapshots: [
        { onDate: '2026-11-16', scope: 3, started: 0, completed: 0 },
        { onDate: '2026-11-18', scope: 4, started: 1, completed: 2 },
      ],
    })
    expect(days.map((day) => [day.date, day.scope, day.completed])).toEqual([
      ['2026-11-16', 3, 0],
      ['2026-11-17', null, null],
      ['2026-11-18', 4, 2],
      ['2026-11-19', null, null],
    ])
    expect(linePath([1, null, 2, 3], (index) => index, (value) => value)).toBe('M0,1 M2,2 L3,3')
  })
})
