import { describe, expect, it } from 'vitest'

import type { HistoryEntry } from '../api/generated/operations'
import { changedSummary, madeBy } from './historyModel'

const entry = (patch: Partial<HistoryEntry>): HistoryEntry => ({ at: '2026-11-18T00:00:00.000Z', changed: [], summary: 'Edited', version: 2, ...patch })

describe('history entries', () => {
  it('say who made a version, through what and for whom', () => {
    expect(madeBy(entry({ actor: 'Rook', actorId: 'r', via: 'Phone', requestedBy: 'Lena', requestedById: 'l' }))).toBe('Rook through Phone, for Lena')
    expect(madeBy(entry({ actor: 'Lena', actorId: 'l', requestedBy: 'Lena', requestedById: 'l' }))).toBe('Lena')
    expect(madeBy(entry({}))).toBe('Someone')
  })

  it('sum up what changed', () => {
    expect(changedSummary(entry({ changed: ['title', 'body'] }))).toBe('title, body')
    expect(changedSummary(entry({ changed: ['A1', 'A2', 'A3', 'A4', 'A5'] }))).toBe('5 places')
    expect(changedSummary(entry({}))).toBeNull()
  })
})
