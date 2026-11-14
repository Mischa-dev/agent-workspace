import { keepPreviousData, useQuery } from '@tanstack/react-query'

import { apiUrl, requestJson } from '../api/client'

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
  /** The last row and column holding something (0 when empty). */
  rows: number
  columns: number
}

export type CellKind = 'empty' | 'number' | 'text' | 'boolean' | 'error'

export type SheetCell = {
  row: number
  column: number
  /** What was typed, such as `12` or `=SUM(B2:B9)`. */
  input: string
  kind: CellKind
  value: string | number | boolean | null
  revision: number
}

export type SheetBody = {
  sheet: SheetSummary
  tabs: SheetTab[]
  /** The tab whose cells came back. */
  tab: string
  cells: SheetCell[]
  canEdit: boolean
}

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

export type ChangedCell = {
  tab: string
  row: number
  column: number
  input: string | null
  kind: CellKind
  value: SheetCell['value']
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
      }
    }),
  }
}

// ---------------------------------------------------------------------------
// Calls

const listPath = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/sheets`
const sheetPath = (projectId: string, sheetId: string) => `${listPath(projectId)}/${encodeURIComponent(sheetId)}`

export const sheetKeys = {
  all: (projectId: string) => ['project', projectId, 'sheets'] as const,
  list: (projectId: string) => ['project', projectId, 'sheets', 'list'] as const,
  one: (projectId: string, sheetId: string) => ['project', projectId, 'sheets', sheetId] as const,
  tab: (projectId: string, sheetId: string, tab: string) => ['project', projectId, 'sheets', sheetId, tab] as const,
}

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

export async function changeSheet(projectId: string, sheetId: string, changes: SheetChange[]) {
  return normalizeResult(
    await requestJson<unknown>(`${sheetPath(projectId, sheetId)}/changes`, {
      method: 'POST',
      idempotencyKey: crypto.randomUUID(),
      json: { changes },
    }),
  )
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
