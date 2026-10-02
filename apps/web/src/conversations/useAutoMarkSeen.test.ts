import { describe, expect, it } from 'vitest'

import { decideMarkSeen, type AutoMarkInput, type ShownSequence } from './useAutoMarkSeen'

const input = (overrides: Partial<AutoMarkInput> = {}): AutoMarkInput => ({
  conversationId: 'group',
  highestSequence: 5,
  highestSeenSequence: 5,
  ready: true,
  ...overrides,
})

describe('decideMarkSeen', () => {
  it('leaves a mark moved back elsewhere until something new arrives', () => {
    // Opened already read: nothing to mark, but the page has shown 5.
    let shown: ShownSequence | null = decideMarkSeen(null, input()).shown
    expect(shown).toEqual({ conversationId: 'group', sequence: 5 })

    // Marked unread from message 3 in another tab or through MCP.
    const unread = decideMarkSeen(shown, input({ highestSeenSequence: 2 }))
    expect(unread.mark).toBeNull()
    shown = unread.shown

    // A new message arrives: now it is marked seen.
    const arrived = decideMarkSeen(shown, input({ highestSequence: 6, highestSeenSequence: 2 }))
    expect(arrived.mark).toBe(6)
    expect(arrived.shown).toEqual({ conversationId: 'group', sequence: 6 })
  })

  it('marks seen when a conversation is opened, and waits until the page is ready', () => {
    expect(decideMarkSeen(null, input({ highestSeenSequence: 2, ready: false }))).toEqual({
      mark: null,
      shown: null,
    })
    expect(decideMarkSeen(null, input({ highestSeenSequence: 2 })).mark).toBe(5)
    const other = { conversationId: 'other', sequence: 9 }
    expect(decideMarkSeen(other, input({ highestSeenSequence: 2 })).mark).toBe(5)
  })
})
