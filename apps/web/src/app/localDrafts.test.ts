import { afterEach, describe, expect, it } from 'vitest'

import { readLocal, setLocalDraftOwner, writeLocal } from './localDrafts'

afterEach(() => {
  setLocalDraftOwner(null)
  window.localStorage.clear()
})

describe('local drafts', () => {
  it('never reads one person’s queued message for another on the same device', () => {
    setLocalDraftOwner('actor-a')
    writeLocal('unsent:channel-1', [{ text: 'From A' }])
    expect(readLocal('unsent:channel-1')).toEqual([{ text: 'From A' }])

    // A's session lapses and B signs in.
    setLocalDraftOwner(null)
    expect(readLocal('unsent:channel-1')).toBeNull()
    setLocalDraftOwner('actor-b')
    expect(readLocal('unsent:channel-1')).toBeNull()
    // And A's is gone from the device, not just hidden.
    setLocalDraftOwner('actor-a')
    expect(readLocal('unsent:channel-1')).toBeNull()
  })

  it('keeps a person’s drafts across a reload', () => {
    setLocalDraftOwner('actor-a')
    writeLocal('draft:channel-1', { text: 'Half written' })
    setLocalDraftOwner(null)
    setLocalDraftOwner('actor-a')
    expect(readLocal('draft:channel-1')).toEqual({ text: 'Half written' })
  })
})
