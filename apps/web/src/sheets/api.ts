import { keepPreviousData, useQuery } from '@tanstack/react-query'

import { apiUrl, requestJson } from '../api/client'
import { sheetKeys } from './keys'

/**
 * Sheets (AWD-54) on the web side: a project's spreadsheets. The server's
 * `sheets::http` works out every value; this side shows one tab's cells and
 * sends what was typed.
 */

export type SheetSummary = {
  id: string
  name: string
  description: string
  /** Tab names, in order. */
  tabs: string[]
  cells: number
  revision: number
  archived: boolean
  updatedAt: string
}

export type SheetTab = {
  id: string
  name: string
  position: number
  structureRevision: number
  frozenRows: number
  /** Widths in pixels by column number, for columns not at the default. */
  columnWidths: Record<number, number>
  /** Its conditional formats, the first that applies winning. */
  conditionalFormats: ConditionalRule[]
  /** The last row and column holding something (0 when empty). */
  rows: number
  columns: number
}

/** A conditional format: cells, when it applies, and how they look then (C1.17). */
export type ConditionalRule = {
  range: string
  when: string
  value?: string | number
  value2?: string | number
  text?: string
  formula?: string
  rank?: number
  percent?: boolean
  low?: string
  high?: string
  mid?: string
  style?: Pick<CellStyle, 'bold' | 'italic' | 'underline' | 'strike' | 'color' | 'fill'>
}

export type CellKind = 'empty' | 'number' | 'text' | 'boolean' | 'error'

/** How a cell's text is drawn; colors are `#RRGGBB`. */
export type CellStyle = {
  bold?: boolean
  italic?: boolean
  underline?: boolean
  strike?: boolean
  color?: string
  fill?: string
  align?: 'left' | 'center' | 'right'
  wrap?: boolean
}

export type SheetCell = {
  row: number
  column: number
  /** What was typed, such as `12` or `=SUM(B2:B9)`; empty for a cell that only has a format or style. */
  input: string
  kind: CellKind
  value: string | number | boolean | null
  revision: number
  /** Its number format, an Excel code such as `0.0%`. */
  format?: string
  style?: CellStyle
  /** The value as its number format shows it, such as `10%`. */
  display?: string
  /** How it looks once its tab's conditional formats apply, when that differs. */
  conditional?: CellStyle
}

/** A change to how cells look; text fields take '' to go back to plain. */
export type LookChange = {
  numberFormat?: string
  bold?: boolean
  italic?: boolean
  underline?: boolean
  strike?: boolean
  color?: string
  fill?: string
  align?: '' | 'left' | 'center' | 'right'
  wrap?: boolean
  clear?: boolean
}

export type SheetBody = {
  sheet: SheetSummary
  tabs: SheetTab[]
  /** The tab whose cells came back. */
  tab: string
  cells: SheetCell[]
  canEdit: boolean
  /** The role only suggests here: cell changes wait as a proposal (C1.11). */
  canSuggest: boolean
  /** The sheet's named ranges, by name (C1.20). */
  ranges: NamedRange[]
}

/** A block of one tab with a name, such as Totals for Costs!A1:B9 (C1.20). */
export type NamedRange = { name: string; tabId: string; tab: string; range: string }

/** One change from the grid, as the server takes it. */
export type SheetChange =
  | {
      type: 'cell'
      tab: string
      row: number
      column: number
      input: string
      ifRevision?: number
    }
  | {
      type: 'insertRows' | 'deleteRows'
      tab: string
      row: number
      count: number
    }
  | {
      type: 'insertColumns' | 'deleteColumns'
      tab: string
      column: number
      count: number
    }
  | { type: 'addTab'; name: string }
  | { type: 'renameTab'; tab: string; name: string }
  | { type: 'removeTab'; tab: string }
  | { type: 'moveTab'; tab: string; position: number }
  | { type: 'freezeRows'; tab: string; rows: number }
  | { type: 'columnWidth'; tab: string; column: number; width: number | null }
  | { type: 'format'; tab: string; rows: [number, number]; columns: [number, number]; look: LookChange }
  /** Sorts a block's rows by its columns (`[column, descending]`, most important first); formulas follow the rows. */
  | { type: 'sort'; tab: string; rows: [number, number]; columns: [number, number]; by: [number, boolean][] }
  /** Replaces a tab's conditional formats. */
  | { type: 'conditionalFormats'; tab: string; rules: ConditionalRule[] }

export type ChangedCell = {
  tab: string
  row: number
  column: number
  input: string | null
  kind: CellKind
  value: SheetCell['value']
  format?: string
  style?: CellStyle
  display?: string
  conditional?: CellStyle
}

export type ChangeResult = {
  revision: number
  /** The revision the write started from; the same as `revision` when it changed nothing. */
  previousRevision: number
  tabsChanged: boolean
  truncated: boolean
  changed: ChangedCell[]
}

// ---------------------------------------------------------------------------
// Reading the server's JSON

type Wire = Record<string, unknown>
const asRecord = (value: unknown): Wire => (value && typeof value === 'object' && !Array.isArray(value) ? (value as Wire) : {})
const asString = (value: unknown) => (typeof value === 'string' ? value : '')
const asNumber = (value: unknown, fallback = 0) => (typeof value === 'number' && Number.isFinite(value) ? value : fallback)
const asArray = (value: unknown) => (Array.isArray(value) ? value : [])
const KINDS: readonly CellKind[] = ['empty', 'number', 'text', 'boolean', 'error']
const asKind = (value: unknown): CellKind => (KINDS.includes(value as CellKind) ? (value as CellKind) : 'empty')
const COLOR = /^#[0-9A-Fa-f]{6}$/
const ALIGNS = ['left', 'center', 'right'] as const
/** Only the style fields the grid knows, in the shapes it expects. */
export function asStyle(value: unknown): CellStyle | undefined {
  const raw = asRecord(value)
  const style: CellStyle = {}
  for (const flag of ['bold', 'italic', 'underline', 'strike', 'wrap'] as const) if (raw[flag] === true) style[flag] = true
  for (const paint of ['color', 'fill'] as const) if (typeof raw[paint] === 'string' && COLOR.test(raw[paint] as string)) style[paint] = raw[paint] as string
  if (ALIGNS.includes(raw.align as (typeof ALIGNS)[number])) style.align = raw.align as CellStyle['align']
  return Object.keys(style).length ? style : undefined
}
const asLook = (raw: Wire) => ({
  ...(typeof raw.format === 'string' && raw.format ? { format: raw.format } : {}),
  ...(asStyle(raw.style) ? { style: asStyle(raw.style) } : {}),
  ...(typeof raw.display === 'string' ? { display: raw.display } : {}),
  ...(raw.conditional && typeof raw.conditional === 'object' ? { conditional: asStyle(raw.conditional) ?? {} } : {}),
})
const asValue = (value: unknown): SheetCell['value'] =>
  typeof value === 'string' || typeof value === 'boolean' || (typeof value === 'number' && Number.isFinite(value)) ? value : null

export function normalizeSheet(value: unknown): SheetSummary {
  const raw = asRecord(value)
  return {
    id: asString(raw.id),
    name: asString(raw.name),
    description: asString(raw.description),
    tabs: asArray(raw.tabs).filter((tab): tab is string => typeof tab === 'string'),
    cells: asNumber(raw.cells),
    revision: asNumber(raw.revision, 1),
    archived: raw.archived === true,
    updatedAt: asString(raw.updatedAt),
  }
}

function normalizeTab(value: unknown): SheetTab {
  const raw = asRecord(value)
  const widths: Record<number, number> = {}
  for (const [column, width] of Object.entries(asRecord(raw.columnWidths))) {
    const number = Number(column)
    if (Number.isInteger(number) && number > 0 && typeof width === 'number') widths[number] = width
  }
  return {
    id: asString(raw.id),
    name: asString(raw.name),
    position: asNumber(raw.position),
    structureRevision: asNumber(raw.structureRevision, 1),
    frozenRows: asNumber(raw.frozenRows, 1),
    columnWidths: widths,
    conditionalFormats: asArray(raw.conditionalFormats).filter((rule): rule is ConditionalRule => typeof asRecord(rule).range === 'string' && typeof asRecord(rule).when === 'string'),
    rows: asNumber(raw.rows),
    columns: asNumber(raw.columns),
  }
}

function normalizeCell(value: unknown): SheetCell {
  const raw = asRecord(value)
  return {
    row: asNumber(raw.row, 1),
    column: asNumber(raw.column, 1),
    input: asString(raw.input),
    kind: asKind(raw.kind),
    value: asValue(raw.value),
    revision: asNumber(raw.revision, 1),
    ...asLook(raw),
  }
}

export function normalizeBody(value: unknown): SheetBody {
  const raw = asRecord(value)
  return {
    sheet: normalizeSheet(raw.sheet),
    tabs: asArray(raw.tabs).map(normalizeTab),
    tab: asString(raw.tab),
    cells: asArray(raw.cells).map(normalizeCell),
    canEdit: raw.canEdit === true,
    canSuggest: raw.canSuggest === true,
    ranges: asArray(raw.ranges).flatMap((entry) => {
      const range = asRecord(entry)
      return typeof range.name === 'string' && typeof range.tabId === 'string' && typeof range.range === 'string'
        ? [{ name: range.name, tabId: range.tabId, tab: asString(range.tab), range: range.range }]
        : []
    }),
  }
}

function normalizeResult(value: unknown): ChangeResult {
  const raw = asRecord(value)
  return {
    revision: asNumber(raw.revision, 1),
    previousRevision: asNumber(raw.previousRevision, asNumber(raw.revision, 1)),
    tabsChanged: raw.tabsChanged === true,
    truncated: raw.truncated === true,
    changed: asArray(raw.changed).map((entry) => {
      const cell = asRecord(entry)
      return {
        tab: asString(cell.tab),
        row: asNumber(cell.row, 1),
        column: asNumber(cell.column, 1),
        input: typeof cell.input === 'string' ? cell.input : null,
        kind: asKind(cell.kind),
        value: asValue(cell.value),
        ...asLook(cell),
      }
    }),
  }
}

// ---------------------------------------------------------------------------
// Calls

const listPath = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/sheets`
const sheetPath = (projectId: string, sheetId: string) => `${listPath(projectId)}/${encodeURIComponent(sheetId)}`

export { sheetKeys } from './keys'

export async function listSheets(projectId: string) {
  const payload = asRecord(await requestJson<unknown>(`${listPath(projectId)}?includeArchived=true`))
  return {
    sheets: asArray(payload.sheets).map(normalizeSheet),
    canEdit: payload.canEdit === true,
  }
}

export async function getSheet(projectId: string, sheetId: string, tab: string) {
  const query = tab ? `?tab=${encodeURIComponent(tab)}` : ''
  return normalizeBody(await requestJson<unknown>(`${sheetPath(projectId, sheetId)}${query}`))
}

export async function createSheet(projectId: string, input: { name: string; description: string; idempotencyKey: string }) {
  const { idempotencyKey, ...json } = input
  return normalizeBody(
    await requestJson<unknown>(listPath(projectId), {
      method: 'POST',
      idempotencyKey,
      json,
    }),
  )
}

export async function updateSheet(
  projectId: string,
  sheetId: string,
  input: {
    name?: string
    description?: string
    archived?: boolean
    tab?: string
  },
) {
  return normalizeBody(
    await requestJson<unknown>(sheetPath(projectId, sheetId), {
      method: 'PATCH',
      idempotencyKey: crypto.randomUUID(),
      json: input,
    }),
  )
}

/** Sends cell changes; the server writes them, or keeps them as a proposal. */
function postChanges(projectId: string, sheetId: string, changes: SheetChange[]) {
  return requestJson<unknown>(`${sheetPath(projectId, sheetId)}/changes`, {
    method: 'POST',
    idempotencyKey: crypto.randomUUID(),
    json: { changes },
  })
}

export async function changeSheet(projectId: string, sheetId: string, changes: SheetChange[]) {
  return normalizeResult(await postChanges(projectId, sheetId, changes))
}

/** Cell changes from someone whose role only suggests: kept as one proposal. */
export async function proposeCells(projectId: string, sheetId: string, changes: SheetChange[]) {
  const raw = asRecord(await postChanges(projectId, sheetId, changes))
  return { proposalId: typeof raw.proposalId === 'string' ? raw.proposalId : null, cells: asArray(raw.proposed).length }
}

/** The file types a sheet can be made from. */
export const SHEET_IMPORT_ACCEPT = '.xlsx,.csv,.tsv,.txt'

/** Makes a sheet from an Excel (.xlsx) or CSV file, named after it. */
export async function importSheet(projectId: string, file: File) {
  const format = /\.xlsx$/i.test(file.name) ? 'xlsx' : 'csv'
  const name = file.name.replace(/\.(xlsx|csv|tsv|txt)$/i, '').trim().slice(0, 80) || 'Imported sheet'
  return normalizeBody(
    await requestJson<unknown>(`${listPath(projectId)}/import?name=${encodeURIComponent(name)}&format=${format}`, {
      method: 'POST',
      idempotencyKey: crypto.randomUUID(),
      body: file,
      headers: { 'content-type': 'application/octet-stream' },
    }),
  )
}

/** Where a sheet downloads: every tab as Excel, or one tab's values as CSV. */
export function sheetExportHref(projectId: string, sheetId: string, format: 'xlsx' | 'csv', tab?: string) {
  const query = format === 'csv' && tab ? `&tab=${encodeURIComponent(tab)}` : ''
  return apiUrl(`${sheetPath(projectId, sheetId)}/export?format=${format}${query}`)
}

export function useSheets(projectId: string) {
  return useQuery({
    queryKey: sheetKeys.list(projectId),
    queryFn: () => listSheets(projectId),
    enabled: Boolean(projectId),
  })
}

/** One tab of a sheet; `tab` is a tab ID, or empty for the first. */
export function useSheet(projectId: string, sheetId: string, tab: string) {
  return useQuery({
    queryKey: sheetKeys.tab(projectId, sheetId, tab),
    queryFn: () => getSheet(projectId, sheetId, tab),
    enabled: Boolean(sheetId),
    placeholderData: keepPreviousData,
  })
}
