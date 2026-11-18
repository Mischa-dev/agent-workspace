import { describe, expect, it } from 'vitest'

import type { TileEntry } from '../api/generated/operations'
import { additive, figurePoints, measureWords, moveTile, stepHeight, stepWidth, tileHref, tileInputs } from './dashboardModel'

const view: TileEntry = { id: 't1', kind: 'view', title: '', text: '', view: 'v1', collection: 'c1', width: 6, height: 2, figures: null }
const note: TileEntry = { id: 't2', kind: 'text', title: 'Read me', text: 'Ship Friday', width: 12, height: 1 }

describe('dashboard tiles', () => {
  it('writes tiles back as they were, a view by its view and a note by its text', () => {
    expect(tileInputs([view, note])).toEqual([
      { id: 't1', kind: 'view', title: null, view: 'v1', text: null, width: 6, height: 2 },
      { id: 't2', kind: 'text', title: 'Read me', view: null, text: 'Ship Friday', width: 12, height: 1 },
    ])
  })

  it('moves a tile within the ends', () => {
    expect(moveTile(['a', 'b', 'c'], 0, 1)).toEqual(['b', 'a', 'c'])
    expect(moveTile(['a', 'b', 'c'], 2, 1)).toEqual(['a', 'b', 'c'])
    expect(moveTile(['a', 'b', 'c'], 2, -2)).toEqual(['c', 'a', 'b'])
  })

  it('steps widths through the ones there are and heights from 1 to 4', () => {
    expect(stepWidth(6, 1)).toBe(8)
    expect(stepWidth(12, 1)).toBe(12)
    expect(stepWidth(3, -1)).toBe(3)
    expect(stepWidth(5, -1)).toBe(4)
    expect(stepHeight(4, 1)).toBe(4)
    expect(stepHeight(1, -1)).toBe(1)
  })

  it('leads a view tile to its collection, opened on its view', () => {
    expect(tileHref('p1', view)).toBe('/projects/p1/collections/c1?view=v1')
    expect(tileHref('p1', { collection: null, view: null })).toBeNull()
  })

  it('reads a chart’s figures as points, counting or measuring', () => {
    const groups = [{ key: 'a', name: '', count: 3, value: 7.5, color: 'blue' }, { key: 'b', name: 'B', count: 1 }]
    const base = { collectionName: 'Bugs', viewName: 'By area', viewType: 'chart', total: 4, records: [] }
    expect(figurePoints({ ...base, groups, measure: 'count' })).toEqual([
      { key: 'a', label: 'None', color: 'blue', count: 3, value: 3 },
      { key: 'b', label: 'B', color: null, count: 1, value: 1 },
    ])
    expect(figurePoints({ ...base, groups, measure: 'sum of Hours' }).map((point) => point.value)).toEqual([7.5, null])
    expect(measureWords('average of Hours')).toBe('Average Hours')
    expect(measureWords('count')).toBe('Records')
    expect(additive('sum of Hours')).toBe(true)
    expect(additive('max of Hours')).toBe(false)
  })
})
