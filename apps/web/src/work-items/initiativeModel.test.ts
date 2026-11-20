import { describe, expect, it } from 'vitest'

import { fieldText, initiativeProgress, share, timeline, type InitiativeMilestone } from './initiativeModel'

const milestone = (name: string, targetDate: string | null): InitiativeMilestone => ({
  id: name,
  name,
  spaceId: 'space',
  spaceKey: 'WORK',
  spaceName: 'Work',
  targetDate,
  archived: false,
  scope: 2,
  completed: 1,
})

describe('initiatives', () => {
  it('counts progress across milestones', () => {
    expect(initiativeProgress({ scope: 5, completed: 3 })).toBe('3 of 5 done')
    expect(initiativeProgress({ scope: 0, completed: 0 })).toBe('No tasks yet')
    expect(share({ scope: 4, completed: 1 })).toBe(0.25)
    expect(share({ scope: 0, completed: 0 })).toBe(0)
  })

  it('reads field values as one line', () => {
    expect(fieldText(['Web', 'Docs'])).toBe('Web, Docs')
    expect(fieldText(true)).toBe('Yes')
    expect(fieldText(null)).toBe('')
  })

  it('lays milestones out by target date, with today between them', () => {
    const laid = timeline(
      [milestone('Late', '2026-12-31'), milestone('Someday', null), milestone('Early', '2026-11-01'), milestone('Middle', '2026-12-01')],
      new Date(2026, 10, 16),
    )
    expect(laid.points.map((point) => point.milestone.name)).toEqual(['Early', 'Middle', 'Late'])
    expect(laid.points[0].at).toBe(0)
    expect(laid.points[2].at).toBe(1)
    expect(laid.undated.map((entry) => entry.name)).toEqual(['Someday'])
    expect(laid.today).toBeGreaterThan(0)
    expect(laid.today).toBeLessThan(laid.points[1].at)
    expect(timeline([milestone('Only', '2026-11-01')]).points[0].at).toBe(0.5)
  })
})
