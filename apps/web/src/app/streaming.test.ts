import { describe, expect, it } from 'vitest'

import { coalesceDelta, coalesceDeltaBatch } from './streaming'

describe('streaming delta coalescing', () => {
  it('appends deltas without mutating the current snapshot', () => {
    const current = { one: 'Hello' }
    expect(coalesceDelta(current, 'one', ' world')).toEqual({ one: 'Hello world' })
    expect(coalesceDelta(current, 'two', 'New')).toEqual({ one: 'Hello', two: 'New' })
    expect(current).toEqual({ one: 'Hello' })
  })

  it('rejects control characters and over-limit messages atomically', () => {
    const current = { safe: 'kept', full: 'a'.repeat(20_000) }
    expect(coalesceDelta(current, 'safe', '\u0000')).toBe(current)
    expect(coalesceDelta(current, 'full', 'b')).toBe(current)
  })

  it('accepts safe entries in a batch while isolating invalid entries', () => {
    const current = { one: 'A', bad: 'unchanged' }
    expect(coalesceDeltaBatch(current, { one: 'B', two: 'C', bad: '\ud800' })).toEqual({
      one: 'AB', two: 'C', bad: 'unchanged',
    })
    expect(current).toEqual({ one: 'A', bad: 'unchanged' })
  })
})
