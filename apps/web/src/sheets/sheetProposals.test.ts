import { describe, expect, it } from 'vitest'

import { proposedCells, proposedOn } from './sheetProposals'

describe('sheet proposals', () => {
  it('reads the cells a proposal sets and marks them on their tab only', () => {
    const cells = proposedCells({
      cells: [
        { tabId: 't1', tab: 'Sheet1', row: 2, column: 3, before: '', after: '5' },
        { tabId: 't2', tab: 'Other', row: 1, column: 1, before: 'a', after: '' },
        { tabId: 't1', tab: 'Sheet1', row: 'x', column: 1 },
      ],
    })
    expect(cells).toHaveLength(2)
    const marks = proposedOn([{ id: 'p', author: null, authorId: 'a', note: null, createdAt: '', cells }], 't1')
    expect([...marks]).toEqual([['2:3', '5']])
    expect(proposedCells(null)).toEqual([])
  })
})
