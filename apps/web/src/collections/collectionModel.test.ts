import { describe, expect, it } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { formatDateLabel } from '../ui/dateModel'
import type { CollectionRecord, ViewConfig } from './api'
import { activeConfig, columnOrder, historyValue, sameConfig, sortText, tableItems, viewColumns, withColumnHidden, withValues } from './collectionModel'

const field = (id: string, type: FieldDefinition['type'], position: number, archived = false): FieldDefinition => ({
  id,
  name: id.toUpperCase(),
  type,
  description: '',
  options: [],
  target: null,
  position,
  archived,
  version: 1,
})

const config = (change: Partial<ViewConfig> = {}): ViewConfig => ({ filters: [], sort: [], groupBy: null, columns: [], titleWidth: null, ...change })

const record = (id: string, group: string | null = null): CollectionRecord => ({
  id,
  number: 1,
  title: id,
  values: {},
  createdBy: { id: 'a', name: 'Ada' },
  updatedBy: { id: 'a', name: 'Ada' },
  archived: false,
  version: 1,
  createdAt: '',
  updatedAt: '',
  group: group === null ? null : { key: group, label: group.toUpperCase() },
})

describe('a view’s columns', () => {
  const fields = [field('b', 'number', 2), field('a', 'text', 1), field('gone', 'text', 3, true), field('c', 'checkbox', 4)]

  it('lists the view’s columns first, then fields it doesn’t name in the collection’s order, never archived ones', () => {
    const shown = viewColumns(fields, config({ columns: [{ field: 'c', width: 300 }, { field: 'gone' }, { field: 'b', hidden: true }] }))
    expect(shown.map(({ field: { id }, width }) => [id, width])).toEqual([
      ['c', 300],
      ['a', 240],
    ])
  })

  it('hides and shows a column without moving the others', () => {
    const hidden = withColumnHidden(fields, config(), 'a', true)
    expect(hidden.columns).toEqual([{ field: 'a', hidden: true }, { field: 'b' }, { field: 'c' }])
    const shownAgain = withColumnHidden(fields, hidden, 'a', false)
    expect(columnOrder(fields, shownAgain).map(({ field: { id }, hidden: isHidden }) => [id, isHidden])).toEqual([
      ['a', false],
      ['b', false],
      ['c', false],
    ])
    // It now lists its columns, so it reads as changed, though it shows the same ones.
    expect(sameConfig(shownAgain, config())).toBe(false)
    expect(viewColumns(fields, shownAgain)).toEqual(viewColumns(fields, config()))
  })

  it('tells a changed view from the saved one, whatever order a clause’s parts came in', () => {
    const saved = config({ filters: [{ field: 'a', op: 'contains', values: ['x'] }], sort: [{ field: 'title', direction: 'asc' }] })
    expect(sameConfig(saved, { ...saved, filters: [{ values: ['x'], op: 'contains', field: 'a' }] })).toBe(true)
    expect(sameConfig(saved, { ...saved, sort: [{ field: 'title', direction: 'desc' }] })).toBe(false)
    expect(sameConfig(saved, { ...saved, groupBy: 'c' })).toBe(false)
  })

  it('leaves out filters, sorts and a grouping on an archived field, which the server refuses in a query', () => {
    const changed = config({
      filters: [
        { field: 'gone', op: 'notEmpty' },
        { field: 'a', op: 'contains', values: ['x'] },
      ],
      sort: [
        { field: 'gone', direction: 'asc' },
        { field: 'updated', direction: 'desc' },
        { field: 'b', direction: 'asc' },
      ],
      groupBy: 'gone',
      columns: [{ field: 'gone', width: 300 }],
    })
    expect(activeConfig(changed, fields)).toEqual(
      config({
        filters: [{ field: 'a', op: 'contains', values: ['x'] }],
        sort: [
          { field: 'updated', direction: 'desc' },
          { field: 'b', direction: 'asc' },
        ],
        columns: [{ field: 'gone', width: 300 }],
      }),
    )
    const fine = config({ groupBy: 'c', sort: [{ field: 'title', direction: 'asc' }] })
    expect(activeConfig(fine, fields)).toBe(fine)
  })
})

describe('the table’s rows', () => {
  const groups = [
    { key: 'x', label: 'X', count: 5, color: null },
    { key: 'y', label: 'Y', count: 1, color: '#fff' },
  ]

  it('heads each group with its full count, folds collapsed groups and ends with a loading row', () => {
    const rows = tableItems([record('1', 'x'), record('2', 'x'), record('3', 'y')], groups, true, new Set(['x']), true)
    expect(rows.map((row) => (row.kind === 'record' ? row.record.id : row.kind === 'group' ? `${row.key}:${row.count}:${row.collapsed}` : row.kind))).toEqual([
      'x:5:true',
      'y:1:false',
      '3',
      'loading',
    ])
  })

  it('lists records alone when the view isn’t grouped', () => {
    expect(tableItems([record('1'), record('2')], [], false, new Set(), false).map((row) => row.kind)).toEqual(['record', 'record'])
  })
})

describe('values', () => {
  it('applies a change, where null or an empty value clears a field', () => {
    expect(withValues({ a: 'x', b: 2, c: true }, { a: null, b: 3, c: [], d: 'new' })).toEqual({ b: 3, d: 'new' })
  })

  it('reads history values as people would say them', () => {
    expect([historyValue(null), historyValue(''), historyValue(true), historyValue(['a', 'b']), historyValue({ label: 'Ada' }), historyValue(4)]).toEqual([
      'empty',
      'empty',
      'Yes',
      'a, b',
      'Ada',
      '4',
    ])
    // A field's type reads a cleared checkbox as No and a date as people write it.
    expect([historyValue(null, 'checkbox'), historyValue(false, 'checkbox'), historyValue(true, 'checkbox')]).toEqual(['No', 'No', 'Yes'])
    expect(historyValue('2026-11-02', 'date')).toBe(formatDateLabel('2026-11-02'))
    expect(historyValue('2026-11-02', 'date')).not.toBe('2026-11-02')
  })

  it('names a sort for its type, as the server orders it', () => {
    const fields = [field('n', 'number', 1), field('c', 'checkbox', 2), field('s', 'single_choice', 3)]
    expect(sortText({ field: 'title', direction: 'asc' }, fields, 'Name')).toBe('Name, A to Z')
    expect(sortText({ field: 'updated', direction: 'desc' }, fields, 'Name')).toBe('Last changed, newest first')
    expect(sortText({ field: 'n', direction: 'desc' }, fields, 'Name')).toBe('N, 9 to 1')
    expect(sortText({ field: 'c', direction: 'asc' }, fields, 'Name')).toBe('C, checked first')
    expect(sortText({ field: 's', direction: 'asc' }, fields, 'Name')).toBe('S, in option order')
  })
})
