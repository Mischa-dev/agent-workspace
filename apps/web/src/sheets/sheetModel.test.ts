import { describe, expect, it } from 'vitest'

import type { SheetBody } from './api'
import { cards, columnName, display, formatTable, parseTable, patched } from './sheetModel'

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

  it('puts a write’s cells in place, and asks for a refetch when rows moved', () => {
    const next = patched(body(), {
      revision: 5,
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
    expect(patched(body(), { revision: 6, tabsChanged: false, truncated: false, changed: [] })).toBeNull()
    expect(
      patched(body(), {
        revision: 5,
        tabsChanged: true,
        truncated: false,
        changed: [],
      }),
    ).toBeNull()
  })

  it('turns rows into cards named by the header row', () => {
    expect(cards(body(), 1)).toEqual([{ row: 2, fields: [{ column: 2, label: 'Cost', text: '2' }] }])
  })
})
