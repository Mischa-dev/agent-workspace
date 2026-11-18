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
  if (cell.display !== undefined) return cell.display
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

/** Text that reads on a fill: dark on light colors, light on dark ones. */
export function inkOn(fill: string) {
  const channel = (start: number) => {
    const value = parseInt(fill.slice(start, start + 2), 16) / 255
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }
  const luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
  return luminance > 0.4 ? '#1A1A1A' : '#FFFFFF'
}

/** A cell's style as the grid draws it: inline properties and the custom properties its class reads. */
export function cellLook(cell: SheetCell | undefined): Record<string, string> | undefined {
  // Conditional formats, when they apply, decide how the cell looks.
  const style = cell?.conditional ?? cell?.style
  if (!style) return undefined
  const look: Record<string, string> = {}
  if (style.bold) look.fontWeight = '600'
  if (style.italic) look.fontStyle = 'italic'
  const lines = [style.underline ? 'underline' : '', style.strike ? 'line-through' : ''].filter(Boolean)
  if (lines.length) look.textDecoration = lines.join(' ')
  if (style.align) look.textAlign = style.align
  if (style.wrap) look.whiteSpace = 'normal'
  if (style.fill) {
    look['--aw-cell-fill'] = style.fill
    look['--aw-cell-ink'] = style.color ?? inkOn(style.fill)
  } else if (style.color) {
    look.color = style.color
  }
  return look
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
  const text = rows.map((row) => row.map((cell) => (/[\t\n"]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell)).join('\t')).join('\n')
  // A last line with nothing on it would read back as no line at all, so end the text with a line break.
  return rows.length && rows[rows.length - 1].every((cell) => cell === '') ? `${text}\n` : text
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
 * should read the sheet again: tabs or rows moved, too many changed, or
 * someone else wrote in between (each write moves the revision on by one),
 * whose cells this screen hasn't seen.
 */
export function patched(body: SheetBody, result: ChangeResult): SheetBody | null {
  if (result.tabsChanged || result.truncated) return null
  // The write must start from what this screen holds (or end where it already is):
  // a write that changed nothing can still report a revision someone else made.
  if (result.previousRevision !== body.sheet.revision && result.revision !== body.sheet.revision) return null
  const tab = body.tabs.find((entry) => entry.id === body.tab)
  if (!tab) return null
  const cells = cellMap(body.cells)
  let rows = tab.rows
  let columns = tab.columns
  for (const change of result.changed) {
    if (change.tab !== tab.name) continue
    const key = cellKey(change.row, change.column)
    const previous = cells.get(key)
    // A cleared cell comes back with no input (keeping any format or style); one whose value alone changed keeps its own.
    const input = change.input ?? ''
    if (!input && !change.format && !change.style) {
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
      ...(change.format ? { format: change.format } : {}),
      ...(change.style ? { style: change.style } : {}),
      ...(change.display !== undefined ? { display: change.display } : {}),
      ...(change.conditional ? { conditional: change.conditional } : {}),
    })
    if (!input) continue
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

export type Range = { top: number; left: number; bottom: number; right: number }

/** `B2:D9` (either corner first) or one cell such as `C4`, or `null` when it isn't a range of the grid. */
export function parseRange(text: string): Range | null {
  const match = /^\s*([A-Za-z]{1,3})(\d{1,5})(?::([A-Za-z]{1,3})(\d{1,5}))?\s*$/.exec(text)
  if (!match) return null
  const first = { column: columnNumber(match[1].toUpperCase()), row: Number(match[2]) }
  const second = match[3] ? { column: columnNumber(match[3].toUpperCase()), row: Number(match[4]) } : first
  const range = {
    top: Math.min(first.row, second.row),
    bottom: Math.max(first.row, second.row),
    left: Math.min(first.column, second.column),
    right: Math.max(first.column, second.column),
  }
  if (range.top < 1 || range.left < 1 || range.bottom > MAX_ROWS || range.right > MAX_COLUMNS) return null
  return range
}

export const rangeName = ({ top, left, bottom, right }: Range) =>
  top === bottom && left === right ? cellName({ row: top, column: left }) : `${cellName({ row: top, column: left })}:${cellName({ row: bottom, column: right })}`

/** A → 1, AA → 27; 0 when it isn't a column name. */
export function columnNumber(name: string) {
  const letters = name.trim().toUpperCase()
  if (!/^[A-Z]{1,3}$/.test(letters)) return 0
  let column = 0
  for (const letter of letters) column = column * 26 + (letter.charCodeAt(0) - 64)
  return column
}

/** A saved filter view as the grid applies it. */
export type ViewSpec = {
  filters: { column: string; op: string; value?: unknown; values?: unknown[] }[]
  sort: { column: string; descending?: boolean }[]
  hiddenColumns: string[]
}

const asNumber = (value: unknown) => {
  if (typeof value === 'number') return value
  if (typeof value === 'string' && value.trim() !== '' && Number.isFinite(Number(value))) return Number(value)
  return null
}

/** -1, 0 or 1: numbers by size when both are numbers, otherwise text ignoring case. */
function compareTo(cell: SheetCell | undefined, wanted: unknown) {
  const left = cell?.kind === 'number' ? asNumber(cell.value) : asNumber(display(cell))
  const right = asNumber(wanted)
  if (left !== null && right !== null) return Math.sign(left - right)
  return Math.sign(display(cell).toLowerCase().localeCompare(String(wanted ?? '').toLowerCase()))
}

function passes(cell: SheetCell | undefined, filter: ViewSpec['filters'][number]) {
  const shown = display(cell)
  switch (filter.op) {
    case 'empty':
      return shown === ''
    case 'notEmpty':
      return shown !== ''
    case 'equals':
      return compareTo(cell, filter.value) === 0
    case 'notEquals':
      return compareTo(cell, filter.value) !== 0
    case 'contains':
      return shown.toLowerCase().includes(String(filter.value ?? '').toLowerCase())
    case 'notContains':
      return !shown.toLowerCase().includes(String(filter.value ?? '').toLowerCase())
    case 'greaterThan':
      return shown !== '' && compareTo(cell, filter.value) > 0
    case 'lessThan':
      return shown !== '' && compareTo(cell, filter.value) < 0
    case 'atLeast':
      return shown !== '' && compareTo(cell, filter.value) >= 0
    case 'atMost':
      return shown !== '' && compareTo(cell, filter.value) <= 0
    case 'oneOf':
      return (filter.values ?? []).some((value) => compareTo(cell, value) === 0)
    default:
      return true
  }
}

/** Numbers, then text, then TRUE/FALSE, then errors; empty cells always last. */
function rank(cell: SheetCell | undefined) {
  if (!cell || display(cell) === '') return 4
  return { number: 0, text: 1, boolean: 2, error: 3 }[cell.kind as string] ?? 1
}

function sortCompare(left: SheetCell | undefined, right: SheetCell | undefined, descending: boolean) {
  const [a, b] = [rank(left), rank(right)]
  if (a !== b || a === 4) return a - b
  const order = a === 0 ? Math.sign(Number(left?.value) - Number(right?.value)) : display(left).toLowerCase().localeCompare(display(right).toLowerCase())
  return descending ? -order : order
}

/**
 * The rows a view shows, in its order: rows `from` to `to` that pass every
 * filter, sorted by its columns (ties keep sheet order). The data stays as it is.
 */
export function viewRows(cells: Map<string, SheetCell>, view: ViewSpec, from: number, to: number) {
  const filters = view.filters.map((filter) => ({ ...filter, at: columnNumber(filter.column) })).filter((filter) => filter.at > 0)
  const rows: number[] = []
  for (let row = from; row <= to; row += 1) {
    if (filters.every((filter) => passes(cells.get(cellKey(row, filter.at)), filter))) rows.push(row)
  }
  const keys = view.sort.map((key) => ({ at: columnNumber(key.column), descending: Boolean(key.descending) })).filter((key) => key.at > 0)
  if (keys.length) {
    rows.sort((left, right) => {
      for (const key of keys) {
        const order = sortCompare(cells.get(cellKey(left, key.at)), cells.get(cellKey(right, key.at)), key.descending)
        if (order) return order
      }
      return left - right
    })
  }
  return rows
}

/**
 * The cells a block of `height` by `breadth` covers from `start` as the grid
 * shows them: in a view, its rows in its order after the frozen ones and only
 * its shown columns, so a paste lands where it is seen. Past the end, undefined.
 */
export function shownBlock(
  start: Position,
  height: number,
  breadth: number,
  shown: { frozen: number; order: readonly number[] | null; columns: readonly number[] | null },
): { rows: (number | undefined)[]; columns: (number | undefined)[] } {
  const shownRows = shown.order ? [...Array.from({ length: shown.frozen }, (_, index) => index + 1), ...shown.order] : null
  const rowIndex = shownRows ? shownRows.indexOf(start.row) : -1
  const columnIndex = shown.columns ? shown.columns.indexOf(start.column) : -1
  return {
    rows: Array.from({ length: height }, (_, offset) => (shownRows && rowIndex >= 0 ? shownRows[rowIndex + offset] : start.row + offset)),
    columns: Array.from({ length: breadth }, (_, offset) => (shown.columns && columnIndex >= 0 ? shown.columns[columnIndex + offset] : start.column + offset)),
  }
}

/**
 * The rows and columns a selection covers as shown: in a view that filters
 * or sorts, the shown rows between its corners, never the hidden ones
 * between them on the sheet; past hidden columns too.
 */
export function shownSelection(
  anchor: Position,
  focus: Position,
  shown: { frozen: number; order: readonly number[] | null; columns: readonly number[] | null },
): { rows: number[]; columns: number[] } {
  const span = (from: number, to: number, list: readonly number[] | null) => {
    const a = list ? list.indexOf(from) : -1
    const b = list ? list.indexOf(to) : -1
    if (list && a >= 0 && b >= 0) return list.slice(Math.min(a, b), Math.max(a, b) + 1)
    const low = Math.min(from, to)
    return Array.from({ length: Math.max(from, to) - low + 1 }, (_, offset) => low + offset)
  }
  const shownRows = shown.order ? [...Array.from({ length: shown.frozen }, (_, index) => index + 1), ...shown.order] : null
  return { rows: span(anchor.row, focus.row, shownRows), columns: span(anchor.column, focus.column, shown.columns) }
}
