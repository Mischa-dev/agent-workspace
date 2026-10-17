import { describe, expect, it } from 'vitest'

import { layoutGraph } from './graphLayout'

describe('layoutGraph', () => {
  it('puts each task one column after its last blocker', () => {
    const layout = layoutGraph({
      nodes: [{ id: 'a' }, { id: 'b' }, { id: 'c' }, { id: 'd' }],
      edges: [
        { from: 'a', to: 'b' },
        { from: 'b', to: 'c' },
        { from: 'a', to: 'c' },
      ],
    })
    expect(layout.placed.get('a')?.column).toBe(0)
    expect(layout.placed.get('b')?.column).toBe(1)
    expect(layout.placed.get('c')?.column).toBe(2)
    expect(layout.placed.get('d')?.column).toBe(0)
    expect(layout.width).toBeGreaterThan(layout.placed.get('c')?.x ?? 0)
  })

  it('survives a cycle a suggested edge might close', () => {
    const layout = layoutGraph({
      nodes: [{ id: 'a' }, { id: 'b' }],
      edges: [
        { from: 'a', to: 'b' },
        { from: 'b', to: 'a' },
      ],
    })
    expect(layout.placed.size).toBe(2)
  })
})
