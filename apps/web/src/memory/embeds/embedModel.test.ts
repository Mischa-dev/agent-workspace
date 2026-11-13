import { describe, expect, it } from 'vitest'

import { collectionEmbed, embedTarget, sheetEmbed } from './embedModel'

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

  it('reads a sheet, its tab and range from the path, and names them', () => {
    expect(embedTarget('/projects/p/sheets/s?tab=t&range=A1%3AC4')).toEqual({ kind: 'sheet', projectId: 'p', sheetId: 's', tabId: 't', range: 'A1:C4' })
    expect(embedTarget('/projects/p/sheets/s')).toEqual({ kind: 'sheet', projectId: 'p', sheetId: 's', tabId: null, range: null })
    expect(embedTarget('/projects/p/sheets')).toEqual({ kind: 'link' })
    const sheet = { id: 's', name: 'Budget' }
    expect(sheetEmbed('p', sheet, { id: 't', name: 'Q1' }, 2, 'A1:C4')).toEqual({ href: '/projects/p/sheets/s?tab=t&range=A1%3AC4', label: 'Budget · Q1 A1:C4' })
    expect(sheetEmbed('p', sheet, { id: 't', name: 'Q1' }, 1, null)).toEqual({ href: '/projects/p/sheets/s?tab=t', label: 'Budget' })
    expect(embedTarget(sheetEmbed('p', sheet, null, 1, 'B2').href)).toMatchObject({ kind: 'sheet', range: 'B2' })
  })
})
