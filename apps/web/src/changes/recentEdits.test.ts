import { describe, expect, it } from 'vitest'

import type { ChangeSetView } from '../api/generated/operations'
import { cellKey, describeTouched, newestOutlined, outlines } from './recentEdits'

const NOW = Date.parse('2026-11-18T12:00:00Z')

function change(id: string, actorId: string, minutesAgo: number, touched: unknown, extra: Partial<ChangeSetView> = {}): ChangeSetView {
  const at = new Date(NOW - minutesAgo * 60_000).toISOString()
  return {
    id,
    actorId,
    actor: actorId === 'me' ? 'Me' : 'Wren',
    createdAt: at,
    updatedAt: at,
    thingId: 'sheet',
    thingKind: 'sheet',
    touched,
    undoable: true,
    versionFrom: 1,
    versionTo: 2,
    ...extra,
  }
}

describe('recent-edit outlines', () => {
  const cells = (row: number) => ({ cells: [{ tabId: 't', row, column: 1 }], more: 0 })

  it('outlines only what others changed just now, unless asked for all', () => {
    const changes = [change('a', 'wren', 1, cells(1)), change('b', 'me', 1, cells(2)), change('c', 'wren', 30, cells(3))]
    const landed = outlines('sheet', changes, { viewerId: 'me', all: false, now: NOW })
    expect([...landed.keys()]).toEqual([cellKey('t', 1, 1)])
    const all = outlines('sheet', changes, { viewerId: 'me', all: true, now: NOW })
    expect(all.size).toBe(3)
  })

  it('names the newest change for an item and skips undone ones', () => {
    const changes = [
      change('old', 'wren', 5, { blocks: ['x', 'y'] }),
      change('new', 'kite', 1, { blocks: ['x'] }),
      change('gone', 'wren', 0, { blocks: ['y'] }, { undoneBy: 'undo' }),
    ]
    const shown = outlines('page', changes, { viewerId: 'me', all: true, now: NOW })
    expect(shown.get('x')?.changeId).toBe('new')
    expect(shown.get('y')?.changeId).toBe('old')
    expect(newestOutlined(changes, shown)?.id).toBe('new')
  })

  it('counts what a change touched, with the ones not listed', () => {
    expect(describeTouched('whiteboard', change('a', 'wren', 0, { elements: ['p', 'q'], more: 3 }))).toBe('5 elements')
    expect(describeTouched('sheet', change('a', 'wren', 0, cells(1)))).toBe('1 cell')
  })
})
