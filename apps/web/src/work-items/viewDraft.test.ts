import { describe, expect, it } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { clausesFit, fieldClauses, withClauses } from './fieldFilterModel'
import { readListState, writeListState } from './listModel'
import { draftFrom } from './viewDraft'

const me = '01a1b5c0-0000-7000-8000-000000000001'
const field = (id: string, type: FieldDefinition['type']): FieldDefinition => ({
  id,
  name: id,
  description: '',
  type,
  options: [],
  target: null,
  position: 1,
  archived: false,
  version: 1,
})
const owner = field('01a1b5c0-0000-7000-8000-0000000000aa', 'person')
const note = field('01a1b5c0-0000-7000-8000-0000000000bb', 'text')

describe('draftFrom', () => {
  it('saves "me" for the person only in assignee and person-field filters', () => {
    const state = readListState(
      new URLSearchParams({
        assigneeActorId: me,
        fieldFilters: JSON.stringify([
          { field: owner.id, op: 'is', values: [me] },
          { field: note.id, op: 'is', values: [me] },
        ]),
      }),
    )
    const draft = draftFrom(state, me, [owner, note])
    expect(draft.query.assigneeActorId).toBe('me')
    // A text field's value that happens to be the person's ID stays as written.
    expect(JSON.parse(draft.query.fieldFilters)).toEqual([
      { field: owner.id, op: 'is', values: ['me'] },
      { field: note.id, op: 'is', values: [me] },
    ])
  })
})

describe('field filter budget', () => {
  const ids = (from: number) => Array.from({ length: 12 }, (_, at) => `01a1b5c0-0000-7000-8000-${String(from + at).padStart(12, '0')}`)
  const first = { field: owner.id, op: 'is' as const, values: ids(0) }
  const second = { field: note.id, op: 'is' as const, values: ids(100) }

  it('keeps any filters it accepts through the URL', () => {
    expect(clausesFit([first])).toBe(true)
    const state = withClauses(readListState(new URLSearchParams()), [first])
    expect(fieldClauses(readListState(writeListState(state)))).toEqual([first])
  })

  it('refuses filters too long for the URL and a saved view, rather than writing ones the list drops', () => {
    // Two filters of twelve IDs each: about 1,077 characters, over the 1,000 a saved view keeps.
    expect(clausesFit([first, second])).toBe(false)
  })
})
