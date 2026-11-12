import type { ChangeResult, SheetBody, SheetCell } from './api'

/** The grid's limits, as the server keeps them. */
export const MAX_ROWS = 10_000
export const MAX_COLUMNS = 200
export const DEFAULT_WIDTH = 112
export const ROW_HEIGHT = 28

export type Position = { row: number; column: number }

/** 1 → A, 27 → AA. */
export function columnName(column: number) {
  let name = ''
  let rest = column
  while (rest > 0) {
    const digit = (rest - 1) % 26
    name = String.fromCharCode(65 + digit) + name
    rest = Math.floor((rest - 1) / 26)
  }
  return name
}

export const cellName = ({ row, column }: Position) => `${columnName(column)}${row}`

export const cellKey = (row: number, column: number) => `${row}:${column}`

export function cellMap(cells: SheetCell[]) {
  return new Map(cells.map((cell) => [cellKey(cell.row, cell.column), cell]))
}

const numberFormat = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 10,
})

/** What a cell shows: its value, written for people. */
export function display(cell: SheetCell | undefined) {
  if (!cell) return ''
  switch (cell.kind) {
    case 'number':
      return typeof cell.value === 'number' ? numberFormat.format(cell.value) : ''
    case 'boolean':
      return cell.value === true ? 'TRUE' : 'FALSE'
    case 'text':
    case 'error':
      return typeof cell.value === 'string' ? cell.value : ''
    default:
      return ''
  }
}

/** How many rows and columns the grid draws: what's used, and room to grow. */
export function extent(used: { rows: number; columns: number }) {
  return {
    rows: Math.min(MAX_ROWS, Math.max(50, used.rows + 20)),
    columns: Math.min(MAX_COLUMNS, Math.max(26, used.columns + 4)),
  }
}

export function clamp(position: Position, size: { rows: number; columns: number }): Position {
  return {
    row: Math.min(Math.max(position.row, 1), size.rows),
    column: Math.min(Math.max(position.column, 1), size.columns),
  }
}

/**
 * Text copied from a spreadsheet: rows on lines, cells split by tabs, and
 * quoted cells that hold tabs, newlines or quotes.
 */
export function parseTable(text: string): string[][] {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let quoted = false
  const source = text.replace(/\r\n?/g, '\n')
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index]
    if (quoted) {
      if (character === '"' && source[index + 1] === '"') {
        cell += '"'
        index += 1
      } else if (character === '"') {
        quoted = false
      } else {
        cell += character
      }
    } else if (character === '"' && cell === '') {
      quoted = true
    } else if (character === '\t') {
      row.push(cell)
      cell = ''
    } else if (character === '\n') {
      row.push(cell)
      rows.push(row)
      row = []
      cell = ''
    } else {
      cell += character
    }
  }
  if (cell !== '' || row.length) {
    row.push(cell)
    rows.push(row)
  }
  return rows
}

/** Cells as text another spreadsheet pastes. */
export function formatTable(rows: string[][]) {
  return rows.map((row) => row.map((cell) => (/[\t\n"]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell)).join('\t')).join('\n')
}

export type Selection = { anchor: Position; focus: Position }

export function bounds({ anchor, focus }: Selection) {
  return {
    top: Math.min(anchor.row, focus.row),
    bottom: Math.max(anchor.row, focus.row),
    left: Math.min(anchor.column, focus.column),
    right: Math.max(anchor.column, focus.column),
  }
}

export function contains(selection: Selection, row: number, column: number) {
  const box = bounds(selection)
  return row >= box.top && row <= box.bottom && column >= box.left && column <= box.right
}

/**
 * A body with a write's changed cells in place, or `null` when the screen
 * should read the sheet again (tabs or rows moved, or too many changed).
 */
export function patched(body: SheetBody, result: ChangeResult): SheetBody | null {
  if (result.tabsChanged || result.truncated) return null
  const tab = body.tabs.find((entry) => entry.id === body.tab)
  if (!tab) return null
  const cells = cellMap(body.cells)
  let rows = tab.rows
  let columns = tab.columns
  for (const change of result.changed) {
    if (change.tab !== tab.name) continue
    const key = cellKey(change.row, change.column)
    const previous = cells.get(key)
    // A cleared cell comes back with no input; one whose value alone changed keeps its own.
    const input = change.input ?? ''
    if (!input) {
      cells.delete(key)
      continue
    }
    cells.set(key, {
      row: change.row,
      column: change.column,
      input,
      kind: change.kind,
      value: change.value,
      revision: previous && previous.input === input ? previous.revision : result.revision,
    })
    rows = Math.max(rows, change.row)
    columns = Math.max(columns, change.column)
  }
  return {
    ...body,
    sheet: { ...body.sheet, revision: result.revision },
    tabs: body.tabs.map((entry) => (entry.id === tab.id ? { ...entry, rows, columns } : entry)),
    cells: [...cells.values()],
  }
}

/** The rows of a tab as cards on a phone: row 1 names the fields. */
export function cards(body: SheetBody, frozenRows: number) {
  const cells = cellMap(body.cells)
  const tab = body.tabs.find((entry) => entry.id === body.tab)
  const columns = tab?.columns ?? 0
  const rows = tab?.rows ?? 0
  const headerRow = frozenRows > 0 ? 1 : 0
  const headers = Array.from({ length: columns }, (_, index) => {
    const header = headerRow ? display(cells.get(cellKey(1, index + 1))) : ''
    return header || columnName(index + 1)
  })
  const result: {
    row: number
    fields: { column: number; label: string; text: string }[]
  }[] = []
  for (let row = headerRow + 1; row <= rows; row += 1) {
    const fields = headers.flatMap((label, index) => {
      const text = display(cells.get(cellKey(row, index + 1)))
      return text ? [{ column: index + 1, label, text }] : []
    })
    if (fields.length) result.push({ row, fields })
  }
  return result
}
