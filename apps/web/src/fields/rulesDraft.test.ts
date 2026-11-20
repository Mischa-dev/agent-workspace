import { describe, expect, it } from 'vitest'

import { normalizeField } from './fieldModel'
import { rulesDraft, rulesInput } from './rulesDraft'

describe('field rules', () => {
  it('reads stored rules back as typed, a choice default by its name', () => {
    const field = normalizeField({
      id: 'f',
      name: 'Kind',
      type: 'single_choice',
      options: [{ id: 'o1', name: 'Nut' }],
      rules: { required: true, default: 'o1' },
    })
    expect(rulesDraft(field)).toMatchObject({ required: true, default: 'Nut', unique: false })
  })

  it('saves only what applies to the type, and refuses a number that isn’t one', () => {
    const draft = { required: false, unique: true, buildersOnly: false, default: '3', min: '1', max: 'x', pattern: '[a-z]+' }
    expect(rulesInput(draft, 'number')).toEqual({ problem: 'The largest value is a number.' })
    expect(rulesInput({ ...draft, max: '' }, 'number')).toEqual({ input: { unique: true, min: 1, default: 3 } })
    expect(rulesInput(draft, 'date')).toEqual({ input: { default: '3' } })
  })
})
