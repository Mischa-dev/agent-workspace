import { describe, expect, it } from 'vitest'

import { anchorFor, elementsAnchor, selectionBounds, toScreen } from './pins'

describe('board pins', () => {
  it('boxes the selected live elements, lines running backwards included', () => {
    const box = selectionBounds(
      [
        { id: 'a', x: 10, y: 20, width: 100, height: 50 },
        { id: 'line', x: 300, y: 200, width: -80, height: -150 },
        { id: 'gone', x: -500, y: -500, width: 10, height: 10, isDeleted: true },
        { id: 'other', x: 900, y: 900, width: 10, height: 10 },
      ],
      ['line', 'a', 'gone'],
    )
    expect(box).toEqual({ ids: ['a', 'line'], x: 10, y: 20, width: 290, height: 180 })
    expect(selectionBounds([], ['a'])).toBeNull()
    expect(anchorFor(box!)).toEqual({ kind: 'elements', elementIds: ['a', 'line'], at: { x: 300, y: 20 } })
  })

  it('reads only element anchors and places them on screen', () => {
    expect(elementsAnchor({ kind: 'cell', tabId: 't', row: 0, column: 0 })).toBeNull()
    const anchor = elementsAnchor({ kind: 'elements', elementIds: ['a'], at: { x: 10, y: 20 } })
    expect(anchor?.elementIds).toEqual(['a'])
    expect(toScreen(anchor!.at, { scrollX: 5, scrollY: -10, zoom: 2 })).toEqual({ left: 30, top: 20 })
  })
})
