import { describe, expect, it } from 'vitest'

import { describeElement, previewSource, proposedElements } from './proposalModel'

describe('board proposals', () => {
  it('reads the elements a proposal sets, with words for each', () => {
    const elements = proposedElements({
      elements: [
        { id: 'a', type: 'rectangle', action: 'add', after: { text: '' } },
        { id: 'b', type: 'text', action: 'change', before: { text: 'Old' }, after: { originalText: 'New  plan' } },
        { id: 'c', type: 'arrow', action: 'remove', before: {}, after: {} },
        { id: 'd', type: 'ellipse', action: 'paint', after: {} },
      ],
    })
    expect(elements.map(describeElement)).toEqual(['Add rectangle', 'Change text “New plan”', 'Remove arrow'])
  })

  it('shows the server’s drawing as an image, and nothing when there is none', () => {
    expect(previewSource('<svg/>')).toBe('data:image/svg+xml;charset=utf-8,%3Csvg%2F%3E')
    expect(previewSource(null)).toBeNull()
  })
})
