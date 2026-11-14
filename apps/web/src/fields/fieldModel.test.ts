import { describe, expect, it } from 'vitest'

import {
  behavesAs,
  canGroup,
  canGroupRecords,
  clausesParam,
  clauseText,
  displayText,
  fieldIdOf,
  isEmpty,
  normalizeField,
  normalizeValues,
  opsFor,
  parseClauses,
  type FieldDefinition,
} from './fieldModel'

const risk: FieldDefinition = normalizeField({
  id: 'f1',
  name: 'Risk',
  type: 'single_choice',
  options: [
    { id: 'o1', name: 'Low', color: '#30a46c' },
    { id: 'o2', name: 'High', color: '#e5484d', archived: true },
  ],
  position: 1,
  version: 2,
})

describe('custom fields', () => {
  it('reads definitions and values defensively', () => {
    expect(risk.options[1].archived).toBe(true)
    expect(normalizeField({ type: 'formula' }).type).toBe('text')
    expect(normalizeValues({ a: 'x', b: 3, c: true, d: ['o1', 2], e: null, f: { nested: 1 } })).toEqual({ a: 'x', b: 3, c: true, d: ['o1'] })
  })

  it('shows values as people read them', () => {
    expect(displayText(risk, 'o2')).toBe('High')
    expect(displayText({ ...risk, type: 'multiple_choice' }, ['o2', 'o1'])).toBe('Low, High')
    expect(displayText({ ...risk, type: 'checkbox' }, true)).toBe('Yes')
    expect(displayText({ ...risk, type: 'person' }, 'a1', [{ id: 'a1', kind: 'person', label: 'Ana', key: '@ana' }])).toBe('Ana')
    expect(displayText({ ...risk, type: 'link', target: 'task' }, 't1', [{ id: 't1', kind: 'task', label: 'Ship it', key: 'AW-3' }])).toBe(
      'AW-3 Ship it',
    )
    expect(isEmpty(false)).toBe(true)
    expect(isEmpty([])).toBe(true)
    expect(isEmpty(0)).toBe(false)
  })

  it('shows small numbers as nonzero', () => {
    const points = { ...risk, type: 'number' as const }
    expect(displayText(points, 1e-7)).toBe('1e-7')
    expect(displayText(points, -1e-7)).toBe('-1e-7')
    expect(displayText(points, 1.234567e-300)).toBe('1.23457e-300')
    expect(displayText(points, 0.0000015)).toBe('0.0000015')
    expect(displayText(points, 2.5)).toBe('2.5')
    expect(displayText(points, 3)).toBe('3')
  })

  it('keeps filters as the server reads them', () => {
    const clauses = [
      { field: 'f1', op: 'is' as const, values: ['o1'] },
      { field: 'f2', op: 'empty' as const },
      { field: 'f3', op: 'contains' as const, values: [] },
    ]
    const param = clausesParam(clauses)
    expect(parseClauses(param)).toEqual(clauses.slice(0, 2))
    expect(parseClauses('nonsense')).toEqual([])
    expect(clausesParam([])).toBeUndefined()
    expect(clauseText(clauses[0], risk)).toBe('Risk is Low')
    expect(opsFor('checkbox')).toEqual(['is', 'empty'])
  })

  it('names a field in a column, sort or group', () => {
    expect(fieldIdOf('field:abc')).toBe('abc')
    expect(fieldIdOf('field:abc:desc')).toBe('abc')
    expect(fieldIdOf('due')).toBeNull()
  })

  it('reads the easy types and the record’s own columns (T1.10)', () => {
    const stage = normalizeField({
      id: 's',
      name: 'Stage',
      type: 'status',
      options: [
        { id: 'w', name: 'Won', color: 'green', category: 'completed' },
        { id: 'x', name: 'Odd', color: 'gray', category: 'someday' },
      ],
    })
    expect(stage.type).toBe('status')
    expect(stage.options.map((option) => option.category)).toEqual(['completed', null])
    expect(displayText(stage, 'w')).toBe('Won')
    expect(opsFor('status')).toContain('categoryIs')
    expect(opsFor('long_text')).toEqual(['contains', 'empty', 'notEmpty'])
    expect(opsFor('created_by')).toEqual(opsFor('person'))
    expect(behavesAs('created_time')).toBe('date')
    expect(displayText({ ...risk, type: 'auto_id' }, 12)).toBe('#12')
    expect(displayText({ ...risk, type: 'created_by' }, 'a1', [{ id: 'a1', kind: 'person', label: 'Ana', key: '@ana' }])).toBe('Ana')
    expect(displayText({ ...risk, type: 'email' }, 'ada@example.com')).toBe('ada@example.com')
    // Task lists group one group per task; collections also by several choices and dates.
    expect(canGroup('multiple_choice')).toBe(false)
    expect(canGroupRecords('multiple_choice')).toBe(true)
    expect(canGroupRecords('created_time')).toBe(true)
    expect(canGroupRecords('long_text')).toBe(false)
  })
})
