import { describe, expect, it } from 'vitest'

import type { SheetBody, SheetCell } from './api'
import { cards, cellLook, cellMap, columnName, columnNumber, display, formatTable, inkOn, parseRange, parseTable, patched, rangeName, viewRows } from './sheetModel'
import { replaced } from './sheetViews'

const body = (): SheetBody => ({
  sheet: {
    id: 's',
    name: 'Budget',
    description: '',
    tabs: ['Sheet1'],
    cells: 3,
    revision: 4,
    archived: false,
    updatedAt: '',
  },
  tabs: [
    {
      id: 't',
      name: 'Sheet1',
      position: 0,
      structureRevision: 1,
      frozenRows: 1,
      columnWidths: {},
      conditionalFormats: [],
      rows: 2,
      columns: 2,
    },
  ],
  tab: 't',
  cells: [
    {
      row: 1,
      column: 1,
      input: 'Item',
      kind: 'text',
      value: 'Item',
      revision: 1,
    },
    {
      row: 1,
      column: 2,
      input: 'Cost',
      kind: 'text',
      value: 'Cost',
      revision: 1,
    },
    { row: 2, column: 2, input: '=1+1', kind: 'number', value: 2, revision: 2 },
  ],
  canEdit: true,
  canSuggest: false,
  ranges: [],
})

describe('sheet model', () => {
  it('names columns as spreadsheets do', () => {
    expect([1, 26, 27, 52, 200].map(columnName)).toEqual(['A', 'Z', 'AA', 'AZ', 'GR'])
  })

  it('reads pasted cells, quoted ones too, and writes them back', () => {
    const rows = parseTable('a\tb\r\n"two\nlines"\t"say ""hi"""\n')
    expect(rows).toEqual([
      ['a', 'b'],
      ['two\nlines', 'say "hi"'],
    ])
    expect(parseTable(formatTable(rows))).toEqual(rows)
    expect(parseTable('one')).toEqual([['one']])
    // A column whose last cell is empty keeps that cell.
    expect(parseTable(formatTable([['new'], ['']]))).toEqual([['new'], ['']])
  })

  it('shows values, not formulas', () => {
    expect(display(body().cells[2])).toBe('2')
    expect(
      display({
        row: 1,
        column: 1,
        input: '=1/0',
        kind: 'error',
        value: '#DIV/0!',
        revision: 1,
      }),
    ).toBe('#DIV/0!')
    expect(
      display({
        row: 1,
        column: 1,
        input: 'TRUE',
        kind: 'boolean',
        value: true,
        revision: 1,
      }),
    ).toBe('TRUE')
  })

  it('shows a formatted value as its format writes it, and draws a style', () => {
    const rate = { row: 2, column: 1, input: '0.1', kind: 'number' as const, value: 0.1, revision: 1, format: '0%', display: '10%' }
    expect(display(rate)).toBe('10%')
    expect(cellLook({ ...rate, style: { bold: true, fill: '#FFF3B0', align: 'center' } })).toEqual({
      fontWeight: '600',
      textAlign: 'center',
      '--aw-cell-fill': '#FFF3B0',
      '--aw-cell-ink': '#1A1A1A',
    })
    expect(inkOn('#202020')).toBe('#FFFFFF')
    expect(cellLook(rate)).toBeUndefined()
    // A cleared cell that keeps its fill stays in the grid, empty.
    const next = patched(body(), {
      revision: 5,
      previousRevision: 4,
      tabsChanged: false,
      truncated: false,
      changed: [{ tab: 'Sheet1', row: 1, column: 1, input: null, kind: 'empty', value: null, style: { fill: '#FFF3B0' } }],
    })
    const kept = next?.cells.find((cell) => cell.row === 1 && cell.column === 1)
    expect(kept?.input).toBe('')
    expect(kept?.style?.fill).toBe('#FFF3B0')
  })

  it('puts a write’s cells in place, and asks for a refetch when rows moved', () => {
    const next = patched(body(), {
      revision: 5,
      previousRevision: 4,
      tabsChanged: false,
      truncated: false,
      changed: [
        {
          tab: 'Sheet1',
          row: 3,
          column: 1,
          input: 'Rent',
          kind: 'text',
          value: 'Rent',
        },
        {
          tab: 'Sheet1',
          row: 2,
          column: 2,
          input: null,
          kind: 'empty',
          value: null,
        },
      ],
    })
    expect(next?.sheet.revision).toBe(5)
    expect(next?.cells.map((cell) => `${cell.row}:${cell.column}`).sort()).toEqual(['1:1', '1:2', '3:1'])
    expect(next?.tabs[0].rows).toBe(3)
    // Revision 5 was someone else's: this screen reads the sheet again.
    expect(patched(body(), { revision: 6, previousRevision: 5, tabsChanged: false, truncated: false, changed: [] })).toBeNull()
    // A write that changed nothing, made after someone else's revision 5.
    expect(patched(body(), { revision: 5, previousRevision: 5, tabsChanged: false, truncated: false, changed: [] })).toBeNull()
    expect(
      patched(body(), {
        revision: 5,
        previousRevision: 4,
        tabsChanged: true,
        truncated: false,
        changed: [],
      }),
    ).toBeNull()
  })

  it('turns rows into cards named by the header row', () => {
    expect(cards(body(), 1)).toEqual([{ row: 2, fields: [{ column: 2, label: 'Cost', text: '2' }] }])
  })

  it('reads a range either way round, and refuses what is off the grid', () => {
    expect(parseRange('d9:b2')).toEqual({ top: 2, bottom: 9, left: 2, right: 4 })
    expect(rangeName(parseRange('C4')!)).toBe('C4')
    expect(rangeName(parseRange(' AA1:B3 ')!)).toBe('B1:AA3')
    expect(parseRange('GS1')).toBeNull()
    expect(parseRange('A0')).toBeNull()
    expect(parseRange('Sheet1!A1')).toBeNull()
  })
})

describe('filter views', () => {
  const cell = (row: number, column: number, value: string | number): SheetCell => ({
    row,
    column,
    input: String(value),
    kind: typeof value === 'number' ? 'number' : 'text',
    value,
    revision: 1,
  })
  const cells = cellMap([
    cell(1, 1, 'Name'),
    cell(1, 2, 'Score'),
    cell(2, 1, 'Cy'),
    cell(2, 2, 3),
    cell(3, 1, 'Ana'),
    cell(3, 2, 7),
    cell(4, 1, 'Di'),
    cell(5, 1, 'Bo'),
    cell(5, 2, 11),
  ])

  it('filters and sorts rows without touching the data', () => {
    expect(viewRows(cells, { filters: [{ column: 'B', op: 'atLeast', value: 5 }], sort: [{ column: 'B', descending: true }], hiddenColumns: [] }, 2, 5)).toEqual([5, 3])
    expect(viewRows(cells, { filters: [], sort: [{ column: 'B' }], hiddenColumns: [] }, 2, 5)).toEqual([2, 3, 5, 4])
    expect(viewRows(cells, { filters: [{ column: 'A', op: 'contains', value: 'a' }], sort: [], hiddenColumns: [] }, 2, 5)).toEqual([3])
    expect(viewRows(cells, { filters: [{ column: 'B', op: 'empty' }], sort: [], hiddenColumns: [] }, 2, 5)).toEqual([4])
    expect(columnNumber('AA')).toBe(27)
  })
})

describe('replacing what was found', () => {
  it('replaces every match, ignoring case unless asked', () => {
    expect(replaced('Rent and rent', 'rent', 'Lease', { matchCase: false, wholeCell: false })).toBe('Lease and Lease')
    expect(replaced('Rent and rent', 'rent', 'lease', { matchCase: true, wholeCell: false })).toBe('Rent and lease')
    expect(replaced('a.b', '.', '$&', { matchCase: false, wholeCell: false })).toBe('a$&b')
    expect(replaced('Rent', 'rent', 'Lease', { matchCase: false, wholeCell: true })).toBe('Lease')
  })
})
