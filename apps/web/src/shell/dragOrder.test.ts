import { describe, expect, it } from 'vitest'

import { orderAfterDrop } from './dragOrder'

const items = ['a', 'b', 'c'].map((id) => ({ id }))
const ids = (order: { id: string }[] | null) => order?.map((item) => item.id) ?? null

describe('dropping a dragged item', () => {
  it('puts it before or after the one it lands on', () => {
    expect(ids(orderAfterDrop(items, 'c', { id: 'a', after: false }))).toEqual(['c', 'a', 'b'])
    expect(ids(orderAfterDrop(items, 'a', { id: 'c', after: true }))).toEqual(['b', 'c', 'a'])
  })

  it('reports nothing moved when it lands where it was', () => {
    expect(orderAfterDrop(items, 'b', { id: 'b', after: false })).toBeNull()
    expect(orderAfterDrop(items, 'b', { id: 'a', after: true })).toBeNull()
    expect(orderAfterDrop(items, 'b', { id: 'gone', after: true })).toBeNull()
  })
})
