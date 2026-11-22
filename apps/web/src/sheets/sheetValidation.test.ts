import { describe, expect, it } from 'vitest'

import type { ValidationRule } from './api'
import { allows, describeRule, markTitle, ruleBook } from './sheetValidation'

describe('sheet validation', () => {
  const status: ValidationRule = { range: 'B2:B50', allow: 'list', choices: ['Yes', 'No'], hint: 'Pick one' }
  const done: ValidationRule = { range: 'C2:C50', allow: 'checkbox' }
  const score: ValidationRule = { range: 'D2:D50', allow: 'number', min: 0, max: 10, whole: true, strict: false }

  it('reads inputs as the server does', () => {
    expect(allows(status, 'yes')).toBe(true)
    expect(allows(status, 'Maybe')).toBe(false)
    expect(allows(status, '')).toBe(true)
    expect(allows(status, '=A1')).toBe(true)
    expect(allows(done, 'TRUE')).toBe(true)
    expect(allows(done, 'x')).toBe(false)
    expect(allows(score, '7')).toBe(true)
    expect(allows(score, '7.5')).toBe(false)
    expect(allows(score, '11')).toBe(false)
    expect(allows({ range: 'A1', allow: 'number', min: 0, max: 1 }, '50%')).toBe(true)
    expect(allows({ range: 'A1', allow: 'number' }, '$1,200')).toBe(true)
    expect(allows({ range: 'A1', allow: 'date', from: '2026-01-01' }, '2026-02-30')).toBe(false)
    expect(allows({ range: 'A1', allow: 'date', from: '2026-01-01' }, '2025-12-31')).toBe(false)
    expect(allows({ range: 'A1', allow: 'textLength', max: 3 }, 'abcd')).toBe(false)
  })

  it('finds the control and the broken rule for a cell', () => {
    const book = ruleBook([status, done, score])
    expect(book.control(3, 2)?.allow).toBe('list')
    expect(book.control(3, 3)?.allow).toBe('checkbox')
    expect(book.control(1, 2)).toBeNull()
    expect(book.broken(4, 4, '12')).toBe(score)
    expect(book.broken(4, 4, '3')).toBeNull()
  })

  it('says what a rule allows', () => {
    expect(describeRule(score)).toBe('a whole number from 0 to 10')
    expect(markTitle(status)).toBe('Should be one of Yes, No (Pick one)')
    expect(describeRule({ range: 'A1', allow: 'textLength', min: 2 })).toBe('text of at least 2 characters')
  })
})
