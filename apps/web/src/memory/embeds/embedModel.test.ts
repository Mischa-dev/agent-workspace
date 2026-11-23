import { describe, expect, it } from 'vitest'

import { registeredEmbed } from '../../modules/embeds'
import { collectionEmbed, embedPath, fileEmbed, sheetChartEmbed, sheetEmbed } from './embedModel'

/** What the module that reads a path read from it (M1.5), or a link. */
function read(href: string) {
  const found = registeredEmbed(href)
  return found ? { embed: `${found.module}:${found.registration.id}`, ...found.params } : { kind: 'link' }
}

describe('what an embed shows', () => {
  it('reads a collection and its view from the path, and anything else as a link', () => {
    const view = 'collections:collection-view'
    expect(read('/projects/p%201/collections/c?view=v')).toEqual({ embed: view, projectId: 'p 1', collectionId: 'c', viewId: 'v' })
    expect(read('/projects/p/collections/c/')).toEqual({ embed: view, projectId: 'p', collectionId: 'c', viewId: null })
    expect(read('/w/-/s/p/collections/c')).toEqual({ embed: view, projectId: 'p', collectionId: 'c', viewId: null })
    expect(read('/projects/p/collections')).toEqual({ kind: 'link' })
    expect(read('/projects/p/collections/c/records')).toEqual({ kind: 'link' })
    expect(read('//elsewhere.example/projects/p/collections/c')).toEqual({ kind: 'link' })
    expect(read('/projects/%E0%A4%A/collections/c')).toEqual({ kind: 'link' })
  })

  it('names a view after its collection, and the view when there are others', () => {
    const collection = { id: 'c/1', name: 'Bugs' }
    expect(collectionEmbed('p', collection, { id: 'v', name: 'Open' }, 1)).toEqual({ href: '/w/-/s/p/collections/c%2F1?view=v', label: 'Bugs' })
    expect(collectionEmbed('p', collection, { id: 'v', name: 'Open' }, 2)).toEqual({ href: '/w/-/s/p/collections/c%2F1?view=v', label: 'Bugs · Open' })
    expect(collectionEmbed('p', collection, null, 0)).toEqual({ href: '/w/-/s/p/collections/c%2F1', label: 'Bugs' })
  })

  it('reads a sheet, its tab and range from the path, and names them', () => {
    const range = 'sheets:sheet-range'
    expect(read('/w/-/s/p/sheets/s?tab=t&range=A1%3AC4')).toEqual({ embed: range, projectId: 'p', sheetId: 's', tabId: 't', range: 'A1:C4', name: null, chart: null })
    expect(read('/projects/p/sheets/s')).toEqual({ embed: range, projectId: 'p', sheetId: 's', tabId: null, range: null, name: null, chart: null })
    expect(read('/projects/p/sheets')).toEqual({ kind: 'link' })
    const sheet = { id: 's', name: 'Budget' }
    expect(sheetEmbed('p', sheet, { id: 't', name: 'Q1' }, 2, 'A1:C4')).toEqual({ href: '/w/-/s/p/sheets/s?tab=t&range=A1%3AC4', label: 'Budget · Q1 A1:C4' })
    expect(sheetEmbed('p', sheet, { id: 't', name: 'Q1' }, 1, null)).toEqual({ href: '/w/-/s/p/sheets/s?tab=t', label: 'Budget' })
    expect(read(sheetEmbed('p', sheet, null, 1, 'B2').href)).toMatchObject({ embed: range, range: 'B2' })
    // A named range is kept by its name alone, so the embed follows it.
    expect(sheetEmbed('p', sheet, { id: 't', name: 'Q1' }, 2, 'A1:C4', 'Totals')).toEqual({ href: '/w/-/s/p/sheets/s?name=Totals', label: 'Budget · Totals' })
    expect(read('/projects/p/sheets/s?name=Totals')).toMatchObject({ embed: range, name: 'Totals', tabId: null })
    expect(read(sheetChartEmbed('p', { id: 's', name: 'Plan' }, { id: 'c1', title: 'Costs' }).href)).toMatchObject({ embed: range, chart: 'c1' })
  })

  it('reads a file of the page from its path', () => {
    const id = '01a1eae3-c9cd-7980-a241-4f1e716a8b07'
    expect(fileEmbed({ id, name: 'plan.png' })).toEqual({ href: `/files/${id}`, label: 'plan.png' })
    expect(read(`/files/${id.toUpperCase()}`)).toEqual({ embed: 'pages:file', fileId: id })
    expect(read('/files/not-an-id')).toEqual({ kind: 'link' })
  })

  it('reads one task, page, whiteboard or thread from its address', () => {
    expect(read('/projects/p1/work-items/t1')).toEqual({ embed: 'tasks:task', kind: 'task', id: 't1' })
    expect(read('/w/acme/s/p1/work-items/t1')).toEqual({ embed: 'tasks:task', kind: 'task', id: 't1' })
    expect(read('/whiteboards/w1')).toEqual({ embed: 'whiteboards:whiteboard', kind: 'whiteboard', id: 'w1' })
    expect(read('/projects/p1/pages/x')).toEqual({ embed: 'pages:page', kind: 'page', id: 'x' })
    expect(read('/projects/p1/threads/c1')).toEqual({ embed: 'channels:thread', kind: 'thread', id: 'c1' })
    expect(read('/projects/p1/work-items/t1/extra')).toEqual({ kind: 'link' })
    expect(read('https://example.com/projects/p1/work-items/t1')).toEqual({ kind: 'link' })
    expect(read('/runs/r1')).toEqual({ kind: 'link' })
  })

  it('reads a space’s task list with its filters and view', () => {
    expect(read('/projects/p1/work-items?view=board&priority=high')).toEqual({ embed: 'tasks:task-list', projectId: 'p1', query: 'view=board&priority=high' })
    expect(read('/projects/p1/work-items')).toEqual({ embed: 'tasks:task-list', projectId: 'p1', query: '' })
  })
})

describe('pasted addresses', () => {
  it('keeps the path of an address in this app and nothing else', () => {
    expect(embedPath('https://app.example/projects/p1/work-items/t1', 'https://app.example')).toBe('/projects/p1/work-items/t1')
    expect(embedPath(' /projects/p1/work-items?status=s ', 'https://app.example')).toBe('/projects/p1/work-items?status=s')
    expect(embedPath('https://elsewhere.example/projects/p1', 'https://app.example')).toBeNull()
    expect(embedPath('', 'https://app.example')).toBeNull()
  })
})
