import { describe, expect, it } from 'vitest'

import { incomingWins, missingFiles, remember, unsaved, type Known } from './sync'

const element = (id: string, version: number, versionNonce: number, extra: Record<string, unknown> = {}) => ({
  id,
  version,
  versionNonce,
  type: 'rectangle',
  ...extra,
})

describe('whiteboard sync', () => {
  it('uses the same version rule as the server', () => {
    expect(incomingWins({ version: 2, versionNonce: 9 }, { version: 1, versionNonce: 1 })).toBe(true)
    expect(incomingWins({ version: 1, versionNonce: 1 }, { version: 2, versionNonce: 9 })).toBe(false)
    expect(incomingWins({ version: 2, versionNonce: 3 }, { version: 2, versionNonce: 5 })).toBe(true)
    expect(incomingWins({ version: 2, versionNonce: 5 }, { version: 2, versionNonce: 5 })).toBe(false)
  })

  it('sends only what the server lacks, and never a new element that is already deleted', () => {
    const known: Known = new Map()
    remember(known, [element('a', 1, 10), element('b', 3, 30)])
    const local = [element('a', 1, 10), element('b', 4, 31), element('c', 1, 5), element('d', 2, 6, { isDeleted: true })]
    expect(unsaved(local, known).map((item) => item.id)).toEqual(['b', 'c'])
    remember(known, unsaved(local, known))
    expect(unsaved(local, known)).toEqual([])
  })

  it('finds images still to load', () => {
    const elements = [
      element('i1', 1, 1, { type: 'image', fileId: 'f1' }),
      element('i2', 1, 1, { type: 'image', fileId: 'f2' }),
      element('i3', 1, 1, { type: 'image', fileId: 'f1' }),
      element('i4', 1, 1, { type: 'image', fileId: 'f3', isDeleted: true }),
    ]
    expect(missingFiles(elements, new Set(['f2']))).toEqual(['f1'])
  })
})
