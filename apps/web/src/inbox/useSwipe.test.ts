import { describe, expect, it } from 'vitest'

import { SWIPE_THRESHOLD, swipeAxis, swipeOutcome } from './useSwipe'

describe('swiping an inbox row', () => {
  it('tells a sideways swipe from a scroll', () => {
    expect(swipeAxis(4, 2)).toBeNull()
    expect(swipeAxis(30, 5)).toBe('x')
    expect(swipeAxis(12, 20)).toBe('y')
  })

  it('acts only past the threshold', () => {
    expect(swipeOutcome(-SWIPE_THRESHOLD + 1)).toBeNull()
    expect(swipeOutcome(-SWIPE_THRESHOLD)).toBe('left')
    expect(swipeOutcome(SWIPE_THRESHOLD + 10)).toBe('right')
  })
})
