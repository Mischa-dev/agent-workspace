import { describe, expect, it } from 'vitest'

import type { PageSummary } from './api'
import { pageTree, parentChoices } from './pageTree'

const page = (id: string, parentPageId: string | null, position: number): PageSummary => ({
  id,
  title: id,
  isGuide: false,
  isTemplate: false,
  skillDescription: null,
  version: 1,
  updatedAt: '',
  updatedBy: null,
  stale: false,
  excerpt: '',
  parentPageId,
  position,
})

describe('pageTree', () => {
  const pages = [page('c', 'b', 1), page('b', 'a', 1), page('a', null, 2), page('z', null, 1), page('d', 'gone', 1)]

  it('puts each parent before its children, with the titles above them', () => {
    expect(pageTree(pages).map((entry) => [entry.id, entry.depth, entry.path.join('/')])).toEqual([
      ['d', 0, ''],
      ['z', 0, ''],
      ['a', 0, ''],
      ['b', 1, 'a'],
      ['c', 2, 'a/b'],
    ])
  })

  it('offers every page but the page itself and those inside it', () => {
    expect(parentChoices(pages, 'b').map((entry) => entry.id)).toEqual(['d', 'z', 'a'])
  })
})
