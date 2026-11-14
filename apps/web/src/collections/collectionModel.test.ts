import { describe, expect, it } from 'vitest'

import type { FieldDefinition } from '../fields/fieldModel'
import { formatDateLabel } from '../ui/dateModel'
import type { CollectionRecord, ViewConfig } from './api'
import {
  activeConfig,
  boardColumns,
  boardKey,
  boardFields,
  boardValue,
  calendarMonth,
  groupableFields,
  columnOrder,
  historyValue,
  recordsByDay,
  sameConfig,
  sortText,
  tableItems,
  viewColumns,
  withColumnHidden,
  withValues,
} from './collectionModel'

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

const config = (change: Partial<ViewConfig> = {}): ViewConfig => ({ filters: [], sort: [], groupBy: null, columns: [], titleWidth: null, dateField: null, ...change })

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

describe('a board’s columns', () => {
  const stage: FieldDefinition = {
    ...field('stage', 'single_choice', 1),
    options: [
      { id: 'queued', name: 'Queued', color: '#888', archived: false },
      { id: 'old', name: 'Old', color: '#888', archived: true },
      { id: 'done', name: 'Done', color: '#0a0', archived: false },
    ],
  }

  it('shows every option in order, empty ones too, archived ones only while records have them, and No value last', () => {
    const columns = boardColumns(stage, [{ key: 'done', label: 'Done', count: 2, color: '#0a0' }])
    expect(columns.map((column) => [column.key, column.count])).toEqual([
      ['queued', 0],
      ['done', 2],
      ['none', 0],
    ])
    expect(boardColumns(stage, [{ key: 'old', label: 'Old', count: 1, color: null }]).map((column) => column.key)).toEqual(['queued', 'old', 'done', 'none'])
  })

  it('puts a record in the column its value says, and a value no column has under No value', () => {
    const columns = boardColumns(stage, [])
    expect(boardKey({ values: { stage: 'done' } }, stage, columns)).toBe('done')
    expect(boardKey({ values: { stage: 'old' } }, stage, columns)).toBe('none')
    expect(boardKey({ values: {} }, stage, columns)).toBe('none')
  })

  it('reads a checkbox as checked or not, and moving a card sets or clears the value', () => {
    const approved = field('ok', 'checkbox', 2)
    const columns = boardColumns(approved, [{ key: 'true', label: 'Checked', count: 3, color: null }])
    expect(columns.map((column) => column.key)).toEqual(['none', 'true'])
    expect(boardKey({ values: { ok: true } }, approved, columns)).toBe('true')
    expect(boardValue(approved, 'true')).toBe(true)
    expect(boardValue(approved, 'none')).toBe(false)
    expect(boardValue(stage, 'none')).toBeNull()
    expect(boardValue(stage, 'done')).toBe('done')
  })

  it('lists the people the server found for a person field', () => {
    const owner = field('owner', 'person', 3)
    const columns = boardColumns(owner, [
      { key: 'p1', label: 'Ada', count: 2, color: null },
      { key: 'none', label: 'No one', count: 1, color: null },
    ])
    expect(columns.map((column) => [column.key, column.label, column.count])).toEqual([
      ['p1', 'Ada', 2],
      ['none', 'No one', 1],
    ])
  })
})

describe('a calendar', () => {
  it('reads its month from the address, or shows this month', () => {
    const today = new Date(2026, 10, 13)
    expect(calendarMonth('2027-02', today)).toEqual(new Date(2027, 1, 1))
    expect(calendarMonth('2027-13', today)).toEqual(new Date(2026, 10, 1))
    expect(calendarMonth(null, today)).toEqual(new Date(2026, 10, 1))
  })

  it('puts each record on its date’s day and leaves undated ones out', () => {
    const dated = (id: string, day?: string): CollectionRecord => ({ ...record(id), values: day ? { due: day } : {} })
    const days = recordsByDay([dated('a', '2026-11-02'), dated('b'), dated('c', '2026-11-02'), dated('d', '2026-11-05')], 'due')
    expect([...days.entries()].map(([day, records]) => [day, records.map((entry) => entry.id)])).toEqual([
      ['2026-11-02', ['a', 'c']],
      ['2026-11-05', ['d']],
    ])
  })
})

describe('statuses, dates and system fields (T1.10)', () => {
  it('puts a status board’s columns in category order, and leaves out what a card can’t move between', () => {
    const stage: FieldDefinition = {
      ...field('stage', 'status', 1),
      options: [
        { id: 'won', name: 'Won', color: 'green', archived: false, category: 'completed' },
        { id: 'lead', name: 'Lead', color: 'gray', archived: false, category: 'backlog' },
        { id: 'talk', name: 'Talking', color: 'blue', archived: false, category: 'started' },
      ],
    }
    expect(boardColumns(stage, []).map((column) => column.key)).toEqual(['lead', 'talk', 'won', 'none'])
    const fields = [stage, field('met', 'date', 2), field('tags', 'multiple_choice', 3), field('maker', 'created_by', 4), field('notes', 'long_text', 5)]
    expect(groupableFields(fields).map((entry) => entry.id)).toEqual(['stage', 'met', 'tags', 'maker'])
    expect(boardFields(fields).map((entry) => entry.id)).toEqual(['stage'])
  })
})
