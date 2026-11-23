import { describe, expect, it } from 'vitest'

import { proposedFields, shownValue } from './proposedChanges'

describe('proposedFields', () => {
  it('reads a record change as it read before and after', () => {
    const change = { collectionId: 'c', number: 3, changes: [{ field: 'f1', name: 'Stage', before: 'Lead', after: 'Won', was: 'o1', is: 'o2' }] }
    expect(proposedFields(change)).toEqual([{ field: 'f1', name: 'Stage', before: 'Lead', after: 'Won' }])
  })

  it('reads a task change by its shown names where ids would mean nothing', () => {
    const change = {
      changes: [
        { field: 'assignee', name: 'Assignee', was: 'a-1', is: 'a-2', wasShown: 'Sol', isShown: 'Mischa' },
        { field: 'labels', name: 'Labels', was: [], is: ['bug', 'web'] },
        { field: 'title', name: 'Title', was: 'Old', is: 'New' },
      ],
    }
    expect(proposedFields(change)).toEqual([
      { field: 'assignee', name: 'Assignee', before: 'Sol', after: 'Mischa' },
      { field: 'labels', name: 'Labels', before: 'empty', after: 'bug, web' },
      { field: 'title', name: 'Title', before: 'Old', after: 'New' },
    ])
  })

  it('leaves out what it cannot read', () => {
    expect(proposedFields(null)).toEqual([])
    expect(proposedFields({ cells: [] })).toEqual([])
    expect(proposedFields({ changes: [null, 4, {}] })).toEqual([])
  })
})

describe('shownValue', () => {
  it('reads empties, booleans and named things', () => {
    expect(shownValue(null)).toBe('empty')
    expect(shownValue(true)).toBe('Yes')
    expect(shownValue(2.5)).toBe('2.5')
    expect(shownValue({ name: 'Sol' })).toBe('Sol')
  })
})
