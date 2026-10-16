import { describe, expect, it } from 'vitest'

import { coalesceDelta, coalesceDeltaBatch } from './streaming'

describe('streamed worker message coalescing', () => {
  it('keeps one in-memory message while appending deltas', () => {
    const first = coalesceDelta({}, 'message-1', 'hello ')
    const second = coalesceDelta(first, 'message-1', 'world')
    expect(second).toEqual({ 'message-1': 'hello world' })
  })

  it('coalesces a frame of deltas into one state update', () => {
    expect(
      coalesceDeltaBatch(
        { 'message-1': 'hello ' },
        { 'message-1': 'world', 'message-2': 'second' },
      ),
    ).toEqual({
      'message-1': 'hello world',
      'message-2': 'second',
    })
  })

  it('keeps the consolidated provider stream inside the Message boundary', () => {
    const exact = { 'message-1': '🙂'.repeat(20_000) }
    expect(coalesceDelta(exact, 'message-1', '🙂')).toBe(exact)
    expect(coalesceDelta(exact, 'message-1', '\u0000')).toBe(exact)
    expect(
      coalesceDeltaBatch(exact, { 'message-1': '🙂', 'message-2': 'safe' }),
    ).toEqual({ ...exact, 'message-2': 'safe' })
  })
})
