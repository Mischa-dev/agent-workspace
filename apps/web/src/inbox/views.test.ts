import { describe, expect, it } from 'vitest'

import { currentView, filterParams, spacesParam, suggestedName, type InboxSavedView } from './views'

const saved = (id: string, filter: InboxSavedView['filter']): InboxSavedView => ({ id, name: id, position: 0, filter, density: 'detailed' })

describe('saved inbox views', () => {
  it('reads spaces from the address in a stable order', () => {
    expect(spacesParam(null)).toEqual([])
    expect(spacesParam('b, a,,b')).toEqual(['a', 'b'])
  })

  it('leaves defaults out of the address', () => {
    expect(filterParams({ view: 'inbox', category: 'all', spaces: [] }).toString()).toBe('')
    expect(filterParams({ view: 'unread', category: 'mentions', spaces: ['b', 'a'] }).toString()).toBe('view=unread&category=mentions&spaces=a%2Cb')
  })

  it('finds the view whose filter is on screen', () => {
    const views = [saved('mentions', { view: 'unread', category: 'mentions' }), saved('alpha', { view: 'inbox', category: 'all', spaces: ['a'] })]
    expect(currentView(views, { view: 'inbox', category: 'all', spaces: ['a'] })?.id).toBe('alpha')
    expect(currentView(views, { view: 'unread', category: 'mentions', spaces: [] })?.id).toBe('mentions')
    expect(currentView(views, { view: 'archived', category: 'all', spaces: [] })).toBeNull()
  })

  it('suggests a name from the filter', () => {
    expect(suggestedName({ view: 'inbox', category: 'all', spaces: [] }, 'All', 'Inbox')).toBe('My inbox')
    expect(suggestedName({ view: 'unread', category: 'mentions', spaces: [] }, 'Mentions', 'Unread')).toBe('Mentions, Unread')
  })
})
