import { describe, expect, it } from 'vitest'

import { collectionEmbed, embedTarget } from './embedModel'

describe('what an embed shows', () => {
  it('reads a collection and its view from the path, and anything else as a link', () => {
    expect(embedTarget('/projects/p%201/collections/c?view=v')).toEqual({ kind: 'collection', projectId: 'p 1', collectionId: 'c', viewId: 'v' })
    expect(embedTarget('/projects/p/collections/c/')).toEqual({ kind: 'collection', projectId: 'p', collectionId: 'c', viewId: null })
    expect(embedTarget('/projects/p/collections')).toEqual({ kind: 'link' })
    expect(embedTarget('/projects/p/collections/c/records')).toEqual({ kind: 'link' })
    expect(embedTarget('//elsewhere.example/projects/p/collections/c')).toEqual({ kind: 'link' })
    expect(embedTarget('/projects/%E0%A4%A/collections/c')).toEqual({ kind: 'link' })
  })

  it('names a view after its collection, and the view when there are others', () => {
    const collection = { id: 'c/1', name: 'Bugs' }
    expect(collectionEmbed('p', collection, { id: 'v', name: 'Open' }, 1)).toEqual({ href: '/projects/p/collections/c%2F1?view=v', label: 'Bugs' })
    expect(collectionEmbed('p', collection, { id: 'v', name: 'Open' }, 2)).toEqual({ href: '/projects/p/collections/c%2F1?view=v', label: 'Bugs · Open' })
    expect(collectionEmbed('p', collection, null, 0)).toEqual({ href: '/projects/p/collections/c%2F1', label: 'Bugs' })
  })
})
