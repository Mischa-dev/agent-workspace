import { describe, expect, it } from 'vitest'

import type { DocEntry } from '../api/generated/operations'
import { docRows, folderChoices } from './docsModel'

const entry = (id: string, kind: DocEntry['kind'], parentId?: string): DocEntry => ({
  id,
  kind,
  title: id,
  restricted: false,
  restrictedHere: false,
  updatedAt: '2026-11-27T00:00:00Z',
  ...(parentId ? { parentId } : {}),
})

const entries = [
  entry('Notes', 'page'),
  entry('Design', 'folder'),
  entry('Sketches', 'folder', 'Design'),
  entry('Board', 'whiteboard', 'Sketches'),
  entry('Budget', 'sheet', 'Design'),
  entry('Orphan', 'page', 'gone'),
]

describe('the Docs tree', () => {
  it('lists folders first and shows what an open folder holds under it', () => {
    expect(docRows(entries, new Set()).map((row) => row.entry.id)).toEqual(['Design', 'Notes', 'Orphan'])
    const open = docRows(entries, new Set(['Design', 'Sketches']))
    expect(open.map((row) => `${row.depth}:${row.entry.id}`)).toEqual(['0:Design', '1:Sketches', '2:Board', '1:Budget', '0:Notes', '0:Orphan'])
    expect(open[0].holds).toBe(2)
  })

  it('never draws a loop twice', () => {
    const loop = [entry('a', 'folder', 'b'), entry('b', 'folder', 'a')]
    expect(docRows(loop, new Set(['a', 'b']))).toEqual([])
  })

  it('offers every folder by its path, except the one being moved and what it holds', () => {
    expect(folderChoices(entries).map((choice) => choice.label)).toEqual(['Design', 'Design / Sketches'])
    expect(folderChoices(entries, 'Design')).toEqual([])
    expect(folderChoices(entries, 'Sketches').map((choice) => choice.id)).toEqual(['Design'])
  })
})
