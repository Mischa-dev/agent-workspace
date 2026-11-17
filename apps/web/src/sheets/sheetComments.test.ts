import { describe, expect, it } from 'vitest'

import type { CommentThreadView } from '../api/generated/operations'
import { commentedCells, selectionAnchor, threadAt } from './sheetComments'

const thread = (id: string, anchor: unknown, status = 'Open'): CommentThreadView => ({
  anchor,
  createdAt: '2026-11-16T00:00:00Z',
  id,
  messageCount: 1,
  name: id,
  status,
  unread: false,
})

describe('sheet comments', () => {
  it('marks the cell a thread is about, counted from 1, on its own tab only', () => {
    const marks = commentedCells(
      [
        thread('cell', { kind: 'cell', tabId: 'a', row: 4, column: 1 }),
        thread('range', { kind: 'range', tabId: 'a', from: [0, 0], to: [2, 2] }),
        thread('other tab', { kind: 'cell', tabId: 'b', row: 0, column: 0 }),
        thread('done', { kind: 'cell', tabId: 'a', row: 9, column: 9 }, 'Resolved'),
        thread('whole sheet', null),
      ],
      'a',
    )
    expect([...marks.keys()].sort()).toEqual(['1:1', '5:2'])
    expect(marks.get('5:2')?.id).toBe('cell')
  })

  it('finds a resolved thread when no open one starts there', () => {
    const threads = [thread('done', { kind: 'cell', tabId: 'a', row: 9, column: 9 }, 'Resolved')]
    expect(threadAt(threads, 'a', 10, 10)?.id).toBe('done')
    expect(threadAt(threads, 'a', 1, 1)).toBeNull()
  })

  it('anchors a new thread to the selection', () => {
    const one = { row: 3, column: 2 }
    expect(selectionAnchor({ anchor: one, focus: one }, 't')).toEqual({ kind: 'cell', tabId: 't', row: 2, column: 1 })
    expect(selectionAnchor({ anchor: { row: 4, column: 3 }, focus: { row: 1, column: 1 } }, 't')).toEqual({
      kind: 'range',
      tabId: 't',
      from: [0, 0],
      to: [3, 2],
    })
  })
})
